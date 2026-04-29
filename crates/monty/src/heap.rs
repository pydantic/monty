use std::{
    cell::{Cell, UnsafeCell},
    fmt,
    marker::PhantomData,
    mem::{ManuallyDrop, size_of},
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    vec,
};

use bytemuck::TransparentWrapper;
use serde::ser::SerializeStruct;
use smallvec::SmallVec;

// Re-export items moved to `heap_traits` so that `crate::heap::HeapGuard` etc. continue
// to resolve (used by the `defer_drop!` macros and throughout the codebase).
pub(crate) use crate::heap_data::HeapData;
pub(crate) use crate::heap_traits::{ContainsHeap, DropWithHeap, HeapGuard, HeapItem, ImmutableHeapGuard};
use crate::{
    asyncio::{Coroutine, GatherFuture, GatherItem},
    bytecode::VM,
    exception_private::{ExcType, RunResult, SimpleException},
    heap_data::{CellValue, Closure, FunctionDefaults},
    resource::{ResourceError, ResourceTracker, check_mult_size, check_repeat_size},
    types::{
        Bytes, Dataclass, Dict, DictItemsView, DictKeysView, DictValuesView, FrozenSet, List, LongInt, Module,
        MontyIter, NamedTuple, Path, PyTrait, Range, ReMatch, RePattern, Set, Slice, Str, TimeZone, Tuple,
        allocate_tuple, date, datetime, timedelta, timezone,
    },
    value::Value,
};

mod heap_entries;
use heap_entries::HeapEntries;

/// Unique identifier for values stored inside the heap arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct HeapId(usize);

impl HeapId {
    /// Creates a `HeapId` from a raw index.
    #[inline]
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index)
    }

    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0
    }
}

/// The empty tuple is a singleton which is allocated at startup.
const EMPTY_TUPLE_ID: HeapId = HeapId(0);

/// Color tag used by the trial-deletion cycle collector (Bacon–Rajan, ECOOP 2001).
///
/// Each [`HeapEntry`] carries a color that represents what the collector currently
/// believes about the entry. Outside of a running collection, every reachable
/// entry is either [`Black`](Self::Black) (live, not part of any suspected cycle)
/// or [`Purple`](Self::Purple) (a candidate cycle root discovered by `dec_ref`,
/// awaiting investigation). [`Gray`](Self::Gray) and [`White`](Self::White) are
/// transient states used only during a [`Heap::collect_cycles`] call.
///
/// The encoding fits in a single byte and is serialized as part of every
/// [`HeapEntry`]: a snapshot taken with cycles pending must round-trip through
/// serde so the entries stay enrolled as candidates after restore (otherwise a
/// graph that becomes garbage just before snapshot would leak permanently).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum CcColor {
    /// Live and not currently a cycle candidate. Default state for every newly
    /// allocated entry.
    #[default]
    Black,
    /// Visited by `MarkGray` during a collection cycle. Children's refcounts
    /// have been provisionally decremented; a later `Scan` pass decides whether
    /// to resurrect (back to [`Black`](Self::Black)) or condemn
    /// ([`White`](Self::White)) the entry.
    Gray,
    /// Confirmed unreachable by the current collection: every reference into
    /// the entry comes from another condemned entry. `CollectWhite` will free
    /// it. Only seen mid-collection.
    White,
    /// Candidate cycle root. Set by `dec_ref` whenever a GC-tracked entry's
    /// refcount drops to a non-zero value — the only situation in which a new
    /// reference cycle can become unreachable. The collector seeds its work
    /// from every entry currently flagged Purple.
    Purple,
}

/// Hash caching state stored alongside each heap entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum HashState {
    /// Hash has not yet been computed but the value might be hashable.
    Unknown,
    /// Cached hash value for immutable types that have been hashed at least once.
    Cached(u64),
    /// Value is unhashable (mutable types or tuples containing unhashables).
    Unhashable,
}

impl HashState {
    fn for_data(data: &HeapData) -> Self {
        match data {
            // Cells are hashable by identity (like all Python objects without __hash__ override)
            // FrozenSet is immutable and hashable
            // Range is immutable and hashable
            // Slice is immutable and hashable (like in CPython)
            // LongInt is immutable and hashable
            // NamedTuple is immutable and hashable (like Tuple)
            HeapData::Str(_)
            | HeapData::Bytes(_)
            | HeapData::Tuple(_)
            | HeapData::NamedTuple(_)
            | HeapData::FrozenSet(_)
            | HeapData::Cell(_)
            | HeapData::Closure(_)
            | HeapData::FunctionDefaults(_)
            | HeapData::Range(_)
            | HeapData::Slice(_)
            | HeapData::LongInt(_)
            | HeapData::Date(_)
            | HeapData::DateTime(_)
            | HeapData::TimeDelta(_)
            | HeapData::TimeZone(_) => Self::Unknown,
            // Dataclass hashability depends on the mutable flag
            HeapData::Dataclass(dc) => {
                if dc.is_frozen() {
                    Self::Unknown
                } else {
                    Self::Unhashable
                }
            }
            // Path is immutable and hashable
            HeapData::Path(_) => Self::Unknown,
            // ExtFunction is hashable (by identity, like closures)
            HeapData::ExtFunction(_) => Self::Unknown,
            // other types are unhashable
            _ => Self::Unhashable,
        }
    }
}

/// This structure allows for reading into the heap more efficiently than repeated calls to `Heap::get` and
/// `Heap::get_mut` by performing the indexing and type lookup once, and then using the borrow checker to
/// safely deference the resulting pointers for short-lived borrows.
///
/// The safety boundary is primarily that `HeapRead` pointers generated by the `HeapReader::read` API must remain valid
/// for their lifetime, see the safety notes in `HeapRead::get` for how that is guaranteed.
pub(crate) struct HeapReader<'a, T: ResourceTracker> {
    pub(crate) heap: &'a mut Heap<T>,
    /// Makes the lifetime `'a` invariant.
    phantom: PhantomData<fn(&'a T) -> &'a T>,
}

impl<T: ResourceTracker> HeapReader<'_, T> {
    /// The ONLY way to get a `HeapReader`. By only providing an API which takes a closure which
    /// must be satisfied for all `'a`, it's impossible to create other `HeapReader` with the
    /// exact same lifetime `'a`.
    pub fn with<R>(heap: &mut Heap<T>, f: impl for<'a> FnOnce(&'a mut HeapReader<'a, T>) -> R) -> R {
        f(&mut HeapReader {
            heap: &mut *heap,
            phantom: PhantomData,
        })
    }
}

impl<'a, T: ResourceTracker> HeapReader<'a, T> {
    /// Indexes into the heap
    pub fn read(&self, id: HeapId) -> HeapReadOutput<'a> {
        /// Computes a `HeapRead` from the raw `UnsafeCell` pointer and a shared reference
        /// to the variant field. The `&T` is only used to compute the field's byte offset
        /// within the `HeapData` enum; the returned `NonNull` is derived from the original
        /// `*mut HeapData` pointer so it inherits the `SharedReadWrite` permission from
        /// the `UnsafeCell`, allowing both reads and writes.
        #[inline]
        fn heap_read<'a, T>(base: *mut HeapData, field: &T, readers: NonNull<Cell<usize>>) -> HeapRead<'a, T> {
            let base_addr = base as usize;
            let field_addr = ptr::from_ref(field) as usize;
            let offset = field_addr - base_addr;
            HeapRead {
                // SAFETY: The pointer is derived from the UnsafeCell's `*mut` via byte
                // offset, preserving the `SharedReadWrite` permission. No reference retag
                // occurs — we only use the `&T` for its address, not to derive the pointer.
                value: unsafe { NonNull::new_unchecked(base.byte_add(offset).cast::<T>()) },
                readers,
                borrow: PhantomData,
            }
        }

        /// Like `heap_read` but for `Box<T>` fields inside `HeapData` variants.
        ///
        /// For boxed variants, the `Box`'s heap allocation lives at a separate
        /// address from the `HeapData` enum, so the offset-from-base trick used
        /// by `heap_read` doesn't work. Instead we derive the pointer directly
        /// from the `Box`'s inner allocation. The pointer remains valid for the
        /// `HeapReader`'s lifetime because the `HeapData` (and its `Box`) stay
        /// alive as long as the reader exists.
        #[expect(
            clippy::borrowed_box,
            reason = "We intentionally take &Box<T> to signal this is for boxed HeapData variants; &T would lose that context"
        )]
        fn heap_read_boxed<'a, T>(boxed: &Box<T>, readers: NonNull<Cell<usize>>) -> HeapRead<'a, T> {
            HeapRead {
                // SAFETY: The Box's allocation is valid for reads/writes as long as the
                // HeapData containing it is alive. The HeapReader guarantees the entry
                // won't be deallocated. We cast away the shared reference to get a mutable
                // pointer — this is sound because all mutation goes through `get_mut` which
                // requires `&mut HeapReader`, ensuring exclusive access.
                value: unsafe { NonNull::new_unchecked(ptr::from_ref(boxed.as_ref()).cast_mut()) },
                readers,
                borrow: PhantomData,
            }
        }

        let heap = self.heap.heap();
        let entry = heap.entries.get(id.index());

        // Increment the reader count for this entry. The corresponding decrement
        // happens in `HeapRead::drop`.
        entry.readers.set(entry.readers.get() + 1);
        let readers = NonNull::from(&entry.readers);

        // Get the raw pointer from the UnsafeCell — this has SharedReadWrite permission.
        let base: *mut HeapData = entry.data.0.get();

        // SAFETY: Match on a shared reference (`&*base`) to read the discriminant without
        // creating a Unique retag. The shared retag is compatible with existing
        // SharedReadWrite permissions from prior `read()` calls into the same UnsafeCell.
        // The `heap_read` helper then derives the NonNull from `base` (not from `&T`),
        // so the returned pointer retains full SharedReadWrite permission.
        match unsafe { &*base } {
            HeapData::Str(s) => HeapReadOutput::Str(heap_read(base, s, readers)),
            HeapData::Bytes(bytes) => HeapReadOutput::Bytes(heap_read(base, bytes, readers)),
            HeapData::List(list) => HeapReadOutput::List(heap_read(base, list, readers)),
            HeapData::Tuple(tuple) => HeapReadOutput::Tuple(heap_read(base, tuple, readers)),
            HeapData::NamedTuple(named_tuple) => HeapReadOutput::NamedTuple(heap_read(base, named_tuple, readers)),
            HeapData::Dict(dict) => HeapReadOutput::Dict(heap_read(base, dict, readers)),
            HeapData::DictItemsView(v) => HeapReadOutput::DictItemsView(heap_read(base, v, readers)),
            HeapData::DictKeysView(v) => HeapReadOutput::DictKeysView(heap_read(base, v, readers)),
            HeapData::DictValuesView(v) => HeapReadOutput::DictValuesView(heap_read(base, v, readers)),
            HeapData::Set(set) => HeapReadOutput::Set(heap_read(base, set, readers)),
            HeapData::FrozenSet(frozen_set) => HeapReadOutput::FrozenSet(heap_read(base, frozen_set, readers)),
            HeapData::Closure(closure) => HeapReadOutput::Closure(heap_read(base, closure, readers)),
            HeapData::FunctionDefaults(function_defaults) => {
                HeapReadOutput::FunctionDefaults(heap_read(base, function_defaults, readers))
            }
            HeapData::ExtFunction(name) => HeapReadOutput::ExtFunction(heap_read(base, name, readers)),
            HeapData::Cell(cell_value) => HeapReadOutput::Cell(heap_read(base, cell_value, readers)),
            HeapData::Range(range) => HeapReadOutput::Range(heap_read(base, range, readers)),
            HeapData::Slice(slice) => HeapReadOutput::Slice(heap_read(base, slice, readers)),
            HeapData::Exception(simple_exception) => {
                HeapReadOutput::Exception(heap_read(base, simple_exception, readers))
            }
            HeapData::Dataclass(dataclass) => HeapReadOutput::Dataclass(heap_read(base, dataclass, readers)),
            HeapData::Iter(monty_iter) => HeapReadOutput::Iter(heap_read(base, monty_iter, readers)),
            HeapData::LongInt(l) => HeapReadOutput::LongInt(heap_read(base, l, readers)),
            HeapData::Module(module) => HeapReadOutput::Module(heap_read(base, module, readers)),
            HeapData::Coroutine(coroutine) => HeapReadOutput::Coroutine(heap_read(base, coroutine, readers)),
            HeapData::GatherFuture(gather_future) => {
                HeapReadOutput::GatherFuture(heap_read(base, gather_future, readers))
            }
            HeapData::Path(path) => HeapReadOutput::Path(heap_read(base, path, readers)),
            HeapData::RePattern(re_pattern) => HeapReadOutput::RePattern(heap_read_boxed(re_pattern, readers)),
            HeapData::ReMatch(re_match) => HeapReadOutput::ReMatch(heap_read(base, re_match, readers)),
            HeapData::Date(d) => HeapReadOutput::Date(heap_read(base, d, readers)),
            HeapData::DateTime(d) => HeapReadOutput::DateTime(heap_read(base, d, readers)),
            HeapData::TimeDelta(d) => HeapReadOutput::TimeDelta(heap_read(base, d, readers)),
            HeapData::TimeZone(d) => HeapReadOutput::TimeZone(heap_read(base, d, readers)),
        }
    }

    #[expect(clippy::unused_self, reason = "'a lifetime is used to create the safety guarantees")]
    pub fn protect<'t, U: ?Sized>(&mut self, value: &'t U) -> BorrowedHeapRead<'t, 'a, U> {
        BorrowedHeapRead {
            inner: ManuallyDrop::new(HeapRead {
                value: NonNull::from(value),
                readers: NonNull::dangling(),
                borrow: PhantomData,
            }),
            original: PhantomData,
        }
    }

    #[expect(clippy::unused_self, reason = "'a lifetime is used to create the safety guarantees")]
    pub fn protect_mut<'t, U: ?Sized>(&mut self, value: &'t mut U) -> BorrowedHeapReadMut<'t, 'a, U> {
        BorrowedHeapReadMut {
            inner: ManuallyDrop::new(HeapRead {
                value: NonNull::from(value),
                readers: NonNull::dangling(),
                borrow: PhantomData,
            }),
            original: PhantomData,
        }
    }
}

impl<T: ResourceTracker> ContainsHeap for HeapReader<'_, T> {
    type ResourceTracker = T;

    fn heap(&self) -> &Heap<T> {
        self.heap.heap()
    }
    fn heap_mut(&mut self) -> &mut Heap<T> {
        self.heap.heap_mut()
    }
}

impl<T: ResourceTracker> Deref for HeapReader<'_, T> {
    type Target = Heap<T>;

    fn deref(&self) -> &Self::Target {
        self.heap
    }
}

impl<T: ResourceTracker> DerefMut for HeapReader<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.heap
    }
}

pub enum HeapReadOutput<'a> {
    Str(HeapRead<'a, Str>),
    Bytes(HeapRead<'a, Bytes>),
    List(HeapRead<'a, List>),
    Tuple(HeapRead<'a, Tuple>),
    NamedTuple(HeapRead<'a, NamedTuple>),
    Dict(HeapRead<'a, Dict>),
    DictItemsView(HeapRead<'a, DictItemsView>),
    DictKeysView(HeapRead<'a, DictKeysView>),
    DictValuesView(HeapRead<'a, DictValuesView>),
    Set(HeapRead<'a, Set>),
    FrozenSet(HeapRead<'a, FrozenSet>),
    Closure(HeapRead<'a, Closure>),
    FunctionDefaults(HeapRead<'a, FunctionDefaults>),
    ExtFunction(HeapRead<'a, String>),
    Cell(HeapRead<'a, CellValue>),
    Range(HeapRead<'a, Range>),
    Slice(HeapRead<'a, Slice>),
    Exception(HeapRead<'a, SimpleException>),
    Dataclass(HeapRead<'a, Dataclass>),
    Iter(HeapRead<'a, MontyIter>),
    LongInt(HeapRead<'a, LongInt>),
    Module(HeapRead<'a, Module>),
    Coroutine(HeapRead<'a, Coroutine>),
    GatherFuture(HeapRead<'a, GatherFuture>),
    Path(HeapRead<'a, Path>),
    RePattern(HeapRead<'a, RePattern>),
    ReMatch(HeapRead<'a, ReMatch>),
    Date(HeapRead<'a, date::Date>),
    DateTime(HeapRead<'a, datetime::DateTime>),
    TimeDelta(HeapRead<'a, timedelta::TimeDelta>),
    TimeZone(HeapRead<'a, timezone::TimeZone>),
}

pub struct HeapRead<'a, T: ?Sized> {
    value: NonNull<T>,
    /// Pointer to the `readers` counter in the owning `HeapValue`.
    ///
    /// Incremented on creation, decremented on drop. This ensures `dec_ref`
    /// cannot free the entry while any `HeapRead` pointing into it exists.
    readers: NonNull<Cell<usize>>,
    /// Makes the lifetime `'a` invariant. In combination with the invariant lifetime
    /// on `HeapReader` and the `HeapReader::with` API, this guarantees that this
    /// `HeapRead` originated from that matching `HeapReader` (there is no way to
    /// construct another `HeapReader` with the same lifetime).
    borrow: PhantomData<fn(&'a T) -> &'a T>,
}

impl<T: ?Sized> Drop for HeapRead<'_, T> {
    fn drop(&mut self) {
        // SAFETY: (DH) the readers pointer is valid for the lifetime of the HeapValue,
        // which is guaranteed by the paged storage (addresses never move) and the
        // reader count itself (dec_ref cannot free an entry with active readers).
        let cell = unsafe { self.readers.as_ref() };
        cell.set(cell.get() - 1);
    }
}

impl<'a, T: ?Sized> HeapRead<'a, T> {
    /// Accesses the value contained in this reference.
    pub fn get<'r, RT: ResourceTracker>(&self, _: &'r HeapReader<'a, RT>) -> &'r T {
        // SAFETY: (DH)
        //  - The HeapReader has an invariant lifetime 'a which guarantees that this HeapRead
        //    came from the heap borrowed by this HeapReader.
        //  - The address of the `HeapValue` never changes because entries are stored in
        //    paged storage (`HeapEntries`) where each page is never reallocated or moved.
        //  - The HeapRead holds a strong reader reference (via the `readers` counter in
        //    `HeapValue`) which guarantees the entry will never be freed by `dec_ref`
        //    or `collect_cycles` while this `HeapRead` exists. The cycle collector's
        //    `Scan` phase treats `readers > 0` as an external reference and resurrects
        //    the entry to Black instead of condemning it as White.
        //  - The type of the `HeapValue` can never change once allocated. This is
        //    guaranteed by never exposing `&mut HeapData` outside of this module.
        //  - The borrow on `HeapReader` guarantees that there are no mutable borrows on any heap
        //    data while the return value of this function is alive.
        unsafe { self.value.as_ref() }
    }

    /// Mutably accesses the value contained in this reference.
    pub fn get_mut<'r>(&mut self, _: &'r mut HeapReader<'a, impl ResourceTracker>) -> &'r mut T {
        // SAFETY: see same constraints as in get() above.
        unsafe { self.value.as_mut() }
    }

    /// Cast this reader around some type T which is a transparent wrapper around U
    /// to its inner type. Name peel comes from `TransparentWrapper::peel` method.
    pub fn peel_ref<U>(&self) -> &HeapRead<'a, U>
    where
        T: TransparentWrapper<U>,
    {
        // SAFETY: (DH) all `HeapRead` have the same layout, T and U pointers are
        // equivalent due to the `#[repr(transparent)] struct T(U)`
        unsafe { NonNull::from(self).cast().as_ref() }
    }

    /// Cast this reader around some type T which is a transparent wrapper around U
    /// to its inner type. Name peel comes from `TransparentWrapper::peel` method.
    pub fn peel_mut<U>(&mut self) -> &mut HeapRead<'a, U>
    where
        T: TransparentWrapper<U>,
    {
        // SAFETY: (DH) all `HeapRead` have the same layout, T and U pointers are
        // equivalent due to the `#[repr(transparent)] struct T(U)`
        unsafe { NonNull::from(self).cast().as_mut() }
    }

    /// Casts this reader to a field of type `U` at some `offset` within the struct.
    ///
    /// Transfers ownership of the reader count from `self` to the returned `HeapRead`.
    ///
    /// # Safety
    ///   - The field of type `U` must ALWAYS exist at `offset` within `T` (i.e. `T` cannot be an enum, union etc)
    unsafe fn cast_as_member_ref<U>(&self, offset: usize) -> BorrowedHeapRead<'_, 'a, U> {
        BorrowedHeapRead {
            // SAFETY: (DH) - caller of this function guarantees the offset & cast is valid
            inner: ManuallyDrop::new(HeapRead {
                // SAFETY: caller guarantees offset points to a valid field of type U within T
                value: unsafe { self.value.byte_add(offset) }.cast(),
                // dangling is fine because this heapread will never be dropped, and it is
                // also not `Clone` so there's no risk of this value ever being used
                readers: NonNull::dangling(),
                borrow: PhantomData,
            }),
            original: PhantomData,
        }
    }

    /// Casts this reader to a field of type `U` at some `offset` within the struct.
    ///
    /// Transfers ownership of the reader count from `self` to the returned `HeapRead`.
    ///
    /// # Safety
    ///   - The field of type `U` must ALWAYS exist at `offset` within `T` (i.e. `T` cannot be an enum, union etc)
    unsafe fn cast_as_member_ref_mut<U>(&mut self, offset: usize) -> BorrowedHeapReadMut<'_, 'a, U> {
        BorrowedHeapReadMut {
            // SAFETY: (DH) - caller of this function guarantees the offset & cast is valid
            inner: ManuallyDrop::new(HeapRead {
                // SAFETY: caller guarantees offset points to a valid field of type U within T
                value: unsafe { self.value.byte_add(offset) }.cast(),
                // dangling is fine because this heapread will never be dropped, and it is
                // also not `Clone` so there's no risk of this value ever being used
                readers: NonNull::dangling(),
                borrow: PhantomData,
            }),
            original: PhantomData,
        }
    }
}

impl<'a, T> HeapRead<'a, Vec<T>> {
    pub fn as_slice(&self, reader: &HeapReader<'a, impl ResourceTracker>) -> BorrowedHeapRead<'_, 'a, [T]> {
        BorrowedHeapRead {
            inner: ManuallyDrop::new(HeapRead {
                value: NonNull::from(self.get(reader).as_slice()),
                readers: NonNull::dangling(),
                borrow: PhantomData,
            }),
            original: PhantomData,
        }
    }
}

impl<'a, T: ?Sized> HeapRead<'a, Box<T>> {
    pub fn as_box_value(&self, reader: &HeapReader<'a, impl ResourceTracker>) -> BorrowedHeapRead<'_, 'a, T> {
        BorrowedHeapRead {
            inner: ManuallyDrop::new(HeapRead {
                value: NonNull::from(self.get(reader).as_ref()),
                readers: NonNull::dangling(),
                borrow: PhantomData,
            }),
            original: PhantomData,
        }
    }
}

/// Represents the reborrow of a `HeapRead` as a reference to a field of the original type.
pub struct BorrowedHeapRead<'original, 'a, U: ?Sized> {
    // inner is a projected HeapRead which will never be dropped
    inner: ManuallyDrop<HeapRead<'a, U>>,
    original: PhantomData<&'original U>,
}

// NB no DerefMut - would need to have a `BorrowedHeapReadMut`
impl<'a, U: ?Sized> Deref for BorrowedHeapRead<'_, 'a, U> {
    type Target = HeapRead<'a, U>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Unsafe helper for `heap_read_as_field`, do not use. Same safety invariants as `HeapRead::cast_as_member`.
pub(crate) unsafe fn cast_as_member_ref_type_hinted<'r, 'a, T, U>(
    heap_read: &'r HeapRead<'a, T>,
    offset: usize,
    _type_hint: impl for<'s> Fn(&'s HeapRead<'a, T>) -> *const U,
) -> BorrowedHeapRead<'r, 'a, U> {
    // SAFETY: (DH) - caller upholds `cast_as_member` contract
    unsafe { heap_read.cast_as_member_ref(offset) }
}

macro_rules! heap_read_ref_as_field {
    ($heap_read:ident, $ty:ty, $field:tt) => {{
        let offset = std::mem::offset_of!($ty, $field);
        #[expect(unreachable_code)]
        let type_hint = |read: &$crate::heap::HeapRead<'_, $ty>| {
            &raw const read.get::<$crate::NoLimitTracker>(unreachable!()).$field
        };
        // SAFETY: (DH)
        //  - `std::mem::offset_of!` guarantees there is a field at fixed offset
        //  - `type_hint` guarantees that the field is of type `U` for the safety contract
        unsafe { $crate::heap::cast_as_member_ref_type_hinted($heap_read, offset, type_hint) }
    }};
}

pub(crate) use heap_read_ref_as_field;

/// Represents the reborrow of a `HeapRead` as a reference to a field of the original type.
pub struct BorrowedHeapReadMut<'original, 'a, U: ?Sized> {
    // inner is a projected HeapRead which will never be dropped
    inner: ManuallyDrop<HeapRead<'a, U>>,
    original: PhantomData<&'original mut U>,
}

impl<'a, U: ?Sized> Deref for BorrowedHeapReadMut<'_, 'a, U> {
    type Target = HeapRead<'a, U>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<U: ?Sized> DerefMut for BorrowedHeapReadMut<'_, '_, U> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Unsafe helper for `heap_read_as_field`, do not use. Same safety invariants as `HeapRead::cast_as_member`.
pub(crate) unsafe fn cast_as_member_ref_mut_type_hinted<'r, 'a, T, U>(
    heap_read: &'r mut HeapRead<'a, T>,
    offset: usize,
    _type_hint: impl for<'s> Fn(&'s HeapRead<'a, T>) -> *const U,
) -> BorrowedHeapReadMut<'r, 'a, U> {
    // SAFETY: (DH) - caller upholds `cast_as_member` contract
    unsafe { heap_read.cast_as_member_ref_mut(offset) }
}

macro_rules! heap_read_ref_as_field_mut {
    ($heap_read:ident, $ty:ty, $field:tt) => {{
        let offset = std::mem::offset_of!($ty, $field);
        #[expect(unreachable_code)]
        let type_hint = |read: &$crate::heap::HeapRead<'_, $ty>| {
            &raw const read.get::<$crate::NoLimitTracker>(unreachable!()).$field
        };
        // SAFETY: (DH)
        //  - `std::mem::offset_of!` guarantees there is a field at fixed offset
        //  - `type_hint` guarantees that the field is of type `U` for the safety contract
        unsafe { $crate::heap::cast_as_member_ref_mut_type_hinted($heap_read, offset, type_hint) }
    }};
}

pub(crate) use heap_read_ref_as_field_mut;

/// A single entry inside the heap arena, storing refcount, payload, and hash metadata.
///
/// The `hash_state` field tracks whether the heap entry is hashable and, if so,
/// caches the computed hash. Mutable types (List, Dict) start as `Unhashable` and
/// will raise TypeError if used as dict keys.
///
/// The `color` field encodes the entry's state for the trial-deletion cycle
/// collector (see [`CcColor`]). Outside of a running collection, every live
/// entry is either Black (uninteresting) or Purple (a cycle-root candidate
/// queued for investigation). Cell-typed so `dec_ref` can flip Black → Purple
/// behind a shared reference to the entry.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct HeapEntry {
    refcount: Cell<usize>,
    /// Number of active `HeapRead` pointers into this entry's data.
    ///
    /// Incremented when `HeapReader::read` creates a `HeapRead`, decremented when
    /// the `HeapRead` is dropped. `dec_ref` panics if it would free an entry that
    /// still has active readers — this guarantees that `HeapRead` pointers remain
    /// valid for as long as they exist.
    #[serde(skip, default)] // should always be 0 during serde ops
    readers: Cell<usize>,
    /// The payload data
    data: UnsafeHeapData,
    /// Current hashing status / cached hash value
    hash_state: HashState,
    /// Cycle-collector color. See [`CcColor`].
    ///
    /// Round-trips through serde because a snapshot taken between bytecode
    /// instructions can capture entries in the [`Purple`](CcColor::Purple)
    /// pending-collection state; dropping the color on restore would leak
    /// any cycle that became unreachable just before the snapshot.
    #[serde(default)]
    color: Cell<CcColor>,
}

/// This wrapper containing `UnsafeCell` exists to allow for data inside of `HeapValue`
/// to be safely pointed to via the `HeapReader` API.
///
/// The safety invariants are protected by the `Heap` / `HeapReader` API:
///   - It is never possible to alias mutable and immutable borrows into heap values,
///     whether they are the same or different value.
///   - When a mutable borrow of a heap value exists, no other heap value may be
///     borrowed. (See `Heap::get_mut` and `HeapRead::get`, which both require a `&mut`
///     borrow on the heap.)
struct UnsafeHeapData(UnsafeCell<HeapData>);

impl fmt::Debug for UnsafeHeapData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: (DH) Debug formatting is read-only and never called concurrently
        // with mutation. This matches the safety invariants of the HeapReader API.
        let data = unsafe { &*self.0.get() };
        f.debug_tuple("UnsafeHeapData").field(data).finish()
    }
}

impl serde::Serialize for UnsafeHeapData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // SAFETY: when heap data is being serialized, there is no mutable borrow
        // possible on any data contents
        HeapData::serialize(unsafe { &*self.0.get() }, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for UnsafeHeapData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(UnsafeCell::new(HeapData::deserialize(deserializer)?)))
    }
}

/// Zero-size token returned by [`Heap::incr_recursion_depth`].
///
/// Represents one level of recursion depth that must be released when the
/// recursive operation completes. There are two ways to release the token:
///
/// - **`DropWithHeap`** — for `&mut Heap` paths (e.g., `py_eq`). Compatible with
///   `defer_drop!` and `HeapGuard` for automatic cleanup on all code paths.
/// - **`DropWithImmutableHeap`** — for `&Heap` paths (e.g., `py_repr_fmt`) where
///   only shared access is available. Compatible with `defer_drop_immutable_heap!`
///   and `ImmutableHeapGuard`.
#[derive(Debug)]
pub(crate) struct RecursionToken(());

impl DropWithHeap for RecursionToken {
    #[inline]
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        heap.heap().decr_recursion_depth();
    }
}

/// Reference-counted arena that backs all heap-only runtime values.
///
/// Uses a free list to reuse slots from freed values, keeping memory usage
/// constant for long-running loops that repeatedly allocate and free values.
/// When an value is freed via `dec_ref`, its slot ID is added to the free list.
/// New allocations pop from the free list when available, otherwise append.
///
/// Cycle collection uses Bacon–Rajan trial deletion: candidates come from
/// `dec_ref` (every container whose refcount drops to a non-zero value is
/// flagged [`Purple`](CcColor::Purple)), so the VM does not enumerate live
/// roots — refcount math itself proves reachability and values held only on
/// the Rust stack are correctly preserved by their non-zero refcount.
///
/// Generic over `T: ResourceTracker` to support different resource tracking strategies.
/// When `T = NoLimitTracker` (the default), all resource checks compile away to no-ops.
///
/// Serialization requires `T: Serialize` and `T: Deserialize`. Custom serde implementation
/// handles the Drop constraint by using `std::mem::take` during serialization.
#[derive(Debug)]
pub(crate) struct Heap<T: ResourceTracker> {
    /// Paged storage for heap entries with integrated free list.
    entries: HeapEntries,
    /// Resource tracker for enforcing limits and scheduling GC.
    tracker: T,
    /// Number of entries currently flagged [`Purple`](CcColor::Purple) — i.e.,
    /// suspected cycle roots awaiting collection.
    ///
    /// Acts as both the GC trigger (the collector runs once this exceeds the
    /// configured interval) and an early-out: when zero, `collect_cycles` has
    /// no candidates and skips its heap walk entirely. Reset to zero at the
    /// end of every successful collection.
    ///
    /// All `dec_ref` paths that mutate this counter take `&mut self`, so a
    /// plain `usize` is sufficient (no interior mutability needed).
    purple_count: usize,
    /// Current recursion depth — incremented on function calls and data structure traversals.
    ///
    /// Uses `Cell` for interior mutability so that methods with only `&Heap`
    /// (like `py_repr_fmt`) can still increment/decrement the depth counter.
    recursion_depth: Cell<usize>,
    /// Cached HeapId for the `datetime.timezone.utc` singleton.
    ///
    /// Lazily allocated on first access to `timezone.utc`. Once created, the refcount
    /// is incremented on each access so the caller can drop their reference normally.
    timezone_utc: Option<HeapId>,
}

impl<T: ResourceTracker + serde::Serialize> serde::Serialize for Heap<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Heap", 4)?;
        state.serialize_field("entries", &self.entries)?;
        state.serialize_field("tracker", &self.tracker)?;
        state.serialize_field("purple_count", &self.purple_count)?;
        state.serialize_field("timezone_utc", &self.timezone_utc)?;
        state.end()
    }
}

impl<'de, T: ResourceTracker + serde::Deserialize<'de>> serde::Deserialize<'de> for Heap<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct HeapFields<T> {
            entries: HeapEntries,
            tracker: T,
            #[serde(default)]
            purple_count: usize,
            #[serde(default)]
            timezone_utc: Option<HeapId>,
        }
        let fields = HeapFields::<T>::deserialize(deserializer)?;
        Ok(Self {
            entries: fields.entries,
            tracker: fields.tracker,
            purple_count: fields.purple_count,
            recursion_depth: Cell::new(0),
            timezone_utc: fields.timezone_utc,
        })
    }
}

/// Default cycle-collection trigger threshold, in [`Purple`](CcColor::Purple)
/// candidate count.
///
/// The collector skips its heap walk entirely when no candidates exist, so
/// this knob only sets how many candidates are allowed to accumulate before a
/// trace is forced. The default of 100 000 trades a small steady-state heap
/// growth (bounded by the threshold) for very low collector overhead in
/// allocation-heavy programs.
///
/// When the `memory-model-checks` feature is enabled, this is reduced to 1 to
/// stress-test GC behavior on every cycle candidate.
const DEFAULT_GC_INTERVAL: usize = if cfg!(feature = "memory-model-checks") {
    1
} else {
    100_000
};

impl<T: ResourceTracker> Heap<T> {
    /// Creates a new heap with the given resource tracker.
    ///
    /// Use this to create heaps with custom resource limits or GC scheduling.
    pub fn new(capacity: usize, tracker: T) -> Self {
        let this = Self {
            entries: HeapEntries::with_capacity(capacity),
            tracker,
            purple_count: 0,
            recursion_depth: Cell::new(0),
            timezone_utc: None,
        };

        // The empty-tuple singleton starts with refcount = 1 — that single ref *is* the
        // permanent heap-owned reference. `get_empty_tuple` bumps the refcount on each
        // hand-out so callers can `dec_ref` normally; the heap-owned ref keeps the
        // singleton's rc ≥ 1 forever, which is why trial deletion needs no special-case
        // rooting for it (a debug_assert in `dec_ref` enforces the invariant).
        let empty_tuple = HeapData::Tuple(Tuple::default());
        let hash_state = HashState::for_data(&empty_tuple);
        let new_entry = HeapEntry {
            refcount: Cell::new(1),
            readers: Cell::new(0),
            data: UnsafeHeapData(UnsafeCell::new(empty_tuple)),
            hash_state,
            color: Cell::new(CcColor::Black),
        };

        let empty_tuple = this.entries.allocate(new_entry);
        debug_assert_eq!(empty_tuple, EMPTY_TUPLE_ID);
        this
    }

    /// Returns a reference to the resource tracker.
    pub fn tracker(&self) -> &T {
        &self.tracker
    }

    /// Returns a mutable reference to the resource tracker.
    pub fn tracker_mut(&mut self) -> &mut T {
        &mut self.tracker
    }

    /// Checks whether the configured time limit has been exceeded.
    ///
    /// Delegates to the resource tracker's `check_time()`. For `NoLimitTracker`,
    /// this is inlined as a no-op with zero runtime cost. For `LimitTracker`,
    /// it compares elapsed time against the configured `max_duration_secs`.
    ///
    /// Call this inside Rust-side loops (builtins, sort, iterator collection)
    /// that execute within a single bytecode instruction and would otherwise
    /// bypass the VM's per-instruction timeout check.
    #[inline]
    pub fn check_time(&self) -> Result<(), ResourceError> {
        self.tracker.check_time()
    }

    /// Tracks in-place memory growth of an existing heap object.
    ///
    /// Call this before performing mutations that grow containers (append, insert,
    /// extend, dict set, set add). Returns `Err(ResourceError::Memory)` if the
    /// growth would exceed configured memory limits.
    ///
    /// Does not increment the allocation count since no new heap object is created.
    #[inline]
    pub fn track_growth(&self, additional_bytes: usize) -> Result<(), ResourceError> {
        self.tracker.on_grow(additional_bytes)
    }

    /// Increments the recursion depth and checks the limit via the `ResourceTracker`.
    ///
    /// Returns `Ok(RecursionToken)` if within limits. The caller must ensure the
    /// token is released on all code paths — either via `defer_drop!`/`HeapGuard`
    /// (for `&mut Heap` contexts) or via `RecursionToken::release()` (for `&Heap` contexts).
    ///
    /// Returns `Err(ResourceError::Recursion)` if the limit would be exceeded.
    #[inline]
    pub fn incr_recursion_depth(&self) -> Result<RecursionToken, ResourceError> {
        let depth = self.recursion_depth.get();
        self.tracker.check_recursion_depth(depth)?;
        self.recursion_depth.set(depth + 1);
        Ok(RecursionToken(()))
    }

    /// Increments the recursion depth, returning `Some(RecursionToken)` if within
    /// limits, or `None` if the limit is exceeded.
    ///
    /// Use this in repr-like contexts where exceeding the limit should produce
    /// truncated output (e.g., `[...]`) rather than an error.
    #[inline]
    pub fn incr_recursion_depth_for_repr(&self) -> Option<RecursionToken> {
        self.incr_recursion_depth().ok()
    }

    /// Decrements the recursion depth.
    ///
    /// Called internally by `RecursionToken` — prefer releasing the token
    /// rather than calling this directly.
    #[inline]
    pub(crate) fn decr_recursion_depth(&self) {
        let depth = self.recursion_depth.get();
        debug_assert!(depth > 0, "decr_recursion_depth called when depth is 0");
        self.recursion_depth.set(depth - 1);
    }

    /// Returns the current recursion depth.
    ///
    /// Used during async task switching to compute a task's depth contribution
    /// before adjusting the global counter.
    pub(crate) fn get_recursion_depth(&self) -> usize {
        self.recursion_depth.get()
    }

    /// Sets the recursion depth to an explicit value.
    ///
    /// Used after deserialization to restore the recursion depth to match
    /// the number of active (non-global) namespace frames that were serialized.
    /// Also used during async task switching to subtract/add a task's depth
    /// contribution when switching away from/to that task.
    pub(crate) fn set_recursion_depth(&self, depth: usize) {
        self.recursion_depth.set(depth);
    }

    /// Number of entries in the heap (including freed slots).
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    /// Returns the number of cycle-collection candidates currently flagged
    /// [`Purple`](CcColor::Purple).
    ///
    /// Used by `run_ref_counts` to expose the trial-deletion trigger metric to
    /// tests. Counts seeds, not heap entries — see [`Heap::collect_cycles`] for
    /// how candidates are produced and consumed.
    #[cfg(feature = "ref-count-return")]
    pub fn get_purple_count(&self) -> usize {
        self.purple_count
    }

    /// Allocates a new heap entry.
    ///
    /// Returns `Err(ResourceError)` if allocation would exceed configured limits.
    /// Use this when you need to handle resource limit errors gracefully.
    ///
    /// Trial deletion does not need to be told that a cycle *might* now exist —
    /// the collector seeds itself from `dec_ref` events. Allocation simply hands
    /// the entry back with the default [`Black`](CcColor::Black) color.
    pub fn allocate(&self, data: HeapData) -> Result<HeapId, ResourceError> {
        self.tracker.on_allocate(|| data.py_estimate_size())?;

        let hash_state = HashState::for_data(&data);
        let new_entry = HeapEntry {
            refcount: Cell::new(1),
            readers: Cell::new(0),
            data: UnsafeHeapData(UnsafeCell::new(data)),
            hash_state,
            color: Cell::new(CcColor::Black),
        };

        let id = self.entries.allocate(new_entry);
        Ok(id)
    }

    /// Returns the singleton empty tuple.
    ///
    /// In Python, `() is ()` is always `True` because empty tuples are interned.
    /// This method provides the same optimization by returning the same `HeapId`
    /// for all empty tuple allocations.
    ///
    /// The returned `Value` has its reference count incremented, so the caller
    /// owns a reference and must call `dec_ref` when done.
    pub fn get_empty_tuple(&self) -> Value {
        // Return existing singleton with incremented refcount
        self.inc_ref(EMPTY_TUPLE_ID);
        Value::Ref(EMPTY_TUPLE_ID)
    }

    /// Returns the cached `datetime.timezone.utc` singleton, lazily creating it on first access.
    ///
    /// The returned `Value::Ref` has its refcount incremented so the caller can drop
    /// it normally. The singleton itself is kept alive by the `timezone_utc` field.
    pub fn get_timezone_utc(&mut self) -> Result<Value, ResourceError> {
        if let Some(id) = self.timezone_utc {
            self.inc_ref(id);
            Ok(Value::Ref(id))
        } else {
            let tz = TimeZone::utc();
            let id = self.allocate(HeapData::TimeZone(tz))?;
            // Keep an extra refcount for the singleton cache
            self.inc_ref(id);
            self.timezone_utc = Some(id);
            Ok(Value::Ref(id))
        }
    }

    /// Increments the reference count for an existing heap entry.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn inc_ref(&self, id: HeapId) {
        let value = self.entries.get(id.index());
        value.refcount.update(|r| r + 1);
    }

    /// Decrements the reference count and frees the value (plus children) once it hits zero.
    ///
    /// Uses an iterative work stack instead of recursion to avoid Rust stack overflow
    /// when freeing deeply nested containers (e.g., a list nested 10,000 levels deep).
    /// This is analogous to CPython's "trashcan" mechanism for safe deallocation.
    ///
    /// Implements the candidate-enrollment side of Bacon–Rajan trial deletion: any
    /// GC-tracked entry whose refcount survives the decrement gets flagged
    /// [`Purple`](CcColor::Purple), so the next [`collect_cycles`](Self::collect_cycles)
    /// can investigate it. Entries that drop to zero are freed immediately on the
    /// existing fast path; if such an entry was Purple, the heap-wide
    /// `purple_count` is rebalanced so it stays in sync with the actual number
    /// of Purple entries.
    ///
    /// # Panics
    /// Panics if the value ID is invalid, the value has already been freed, or
    /// the refcount would reach zero while active `HeapRead` readers exist.
    pub fn dec_ref(&mut self, id: HeapId) {
        let mut current_id = id;
        let mut work_stack = Vec::new();
        loop {
            let slot = self.entries.get_mut(current_id.index());
            let entry = slot.as_mut().expect("Heap::dec_ref: object already freed");
            if entry.refcount.get() > 1 {
                entry.refcount.update(|r| r - 1);

                // SAFETY: only `&mut self` paths reach here, so reading the
                // discriminant of `data` cannot race with mutation.
                let is_gc_tracked = unsafe { &*entry.data.0.get() }.is_gc_tracked();
                if is_gc_tracked && entry.color.get() != CcColor::Purple {
                    // The refcount survived — this entry is the only place a
                    // newly unreachable cycle could now be hiding. Flag it as
                    // a candidate for the next `collect_cycles`.
                    entry.color.set(CcColor::Purple);
                    self.purple_count += 1;
                }
            } else {
                debug_assert!(
                    current_id != EMPTY_TUPLE_ID,
                    "Heap::dec_ref: empty-tuple singleton's heap-owned refcount must never reach zero",
                );
                assert!(
                    entry.readers.get() == 0,
                    "Heap::dec_ref: cannot free HeapId({}) with {} active reader(s)",
                    current_id.index(),
                    entry.readers.get(),
                );
                // If the entry was a pending cycle candidate, decrement
                // `purple_count` to reflect that it is leaving the heap before
                // the collector reaches it.
                if entry.color.get() == CcColor::Purple {
                    debug_assert!(self.purple_count > 0);
                    self.purple_count -= 1;
                }
                if let Some(mut value) = slot.take() {
                    // refcount == 1, free the value and add slot to free list for reuse
                    self.entries.free(current_id);

                    // Notify tracker of freed memory
                    self.tracker.on_free(|| value.data.0.get_mut().py_estimate_size());

                    // Collect child IDs and push onto work stack for iterative processing
                    py_dec_ref_ids_for_data(value.data.0.get_mut(), &mut work_stack);
                }
            }

            let Some(next_id) = work_stack.pop() else {
                break;
            };
            current_id = next_id;
        }
    }

    /// Returns an immutable reference to the heap data stored at the given ID.
    ///
    /// # Panics
    /// Panics if the value ID is invalid, the value has already been freed,
    /// or the data is currently borrowed via `with_entry_mut`/`call_attr`.
    #[must_use]
    pub fn get(&self, id: HeapId) -> &HeapData {
        let data = &self.entries.get(id.index()).data;
        // SAFETY: (DH) no mutable references into `HeapData` is possible while the heap is borrowed
        unsafe { &*data.0.get() }
    }

    /// Returns or computes the hash for the heap entry at the given ID.
    ///
    /// Hashes are computed lazily on first use and then cached. Returns
    /// `Ok(Some(hash))` for immutable types, `Ok(None)` for mutable types,
    /// or `Err(ResourceError::Recursion)` if the recursion limit is exceeded.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn get_or_compute_hash(vm: &mut VM<'_, '_, T>, id: HeapId) -> Result<Option<u64>, ResourceError> {
        // TODO: it should be possible to refactor the triple lookup to just one, probably by having an
        // internal `vm.heap.read_entry` method which can then derive the `HeapReadOutput` for `py_hash`
        // later, and can live without a VM borrow to allow reading / writing the hash.
        //
        // That only matters before the hash is cached, so not the worst thing for performance.

        let entry = vm
            .heap
            .entries
            .get_mut(id.index())
            .as_mut()
            .expect("Heap::get_or_compute_hash: object already freed");

        match entry.hash_state {
            HashState::Unhashable => return Ok(None),
            HashState::Cached(hash) => return Ok(Some(hash)),
            HashState::Unknown => {}
        }

        let hash = vm.heap.read(id).py_hash(id, vm)?;

        // Cache the result
        let entry = vm
            .heap
            .entries
            .get_mut(id.index())
            .as_mut()
            .expect("Heap::get_or_compute_hash: object freed during compute");
        entry.hash_state = match hash {
            Some(value) => HashState::Cached(value),
            None => HashState::Unhashable,
        };
        Ok(hash)
    }

    /// Returns the reference count for the heap entry at the given ID.
    ///
    /// This is primarily used for testing reference counting behavior.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    #[must_use]
    #[cfg(feature = "ref-count-return")]
    pub fn get_refcount(&self, id: HeapId) -> usize {
        self.entries.get(id.index()).refcount.get()
    }

    /// Returns the number of live (non-freed) values on the heap.
    ///
    /// This is primarily used for testing to verify that all heap entries
    /// are accounted for in reference count tests.
    ///
    /// Excludes the empty tuple singleton since it's an internal optimization
    /// detail that persists even when not explicitly used by user code.
    #[must_use]
    #[cfg(feature = "ref-count-return")]
    pub fn entry_count(&self) -> usize {
        // Skip index 0 which is the empty tuple singleton
        self.entries.iter().skip(1).count()
    }

    /// Multiplies a heap-allocated value by an `i64`.
    ///
    /// If `id` refers to a `LongInt`, performs integer multiplication with a size
    /// pre-check. Otherwise, treats `id` as a sequence and `int_val` as the repeat
    /// count. This avoids multiple `heap.get()` calls by looking up the data once.
    ///
    /// Returns `Ok(None)` if the heap entry is neither a LongInt nor a sequence type.
    pub fn mult_ref_by_i64(&mut self, id: HeapId, int_val: i64) -> RunResult<Option<Value>> {
        match self.get(id) {
            HeapData::LongInt(li) => {
                check_mult_size(li.bits(), i64_bits(int_val), &self.tracker)?;
                let result = LongInt::new(li.inner().clone()) * LongInt::from(int_val);
                Ok(Some(result.into_value(self)?))
            }
            HeapData::TimeDelta(td) => {
                let total = timedelta::total_microseconds(td)
                    .checked_mul(i128::from(int_val))
                    .ok_or_else(|| {
                        SimpleException::new_msg(ExcType::OverflowError, "timedelta multiplication overflow")
                    })?;
                let delta = timedelta::from_total_microseconds(total)?;
                Ok(Some(Value::Ref(self.allocate(HeapData::TimeDelta(delta))?)))
            }
            _ => {
                let count = i64_to_repeat_count(int_val)?;
                self.mult_sequence(id, count)
            }
        }
    }

    /// Multiplies two heap-allocated values.
    ///
    /// Returns Ok(None) for unsupported type combinations.
    pub fn mult_heap_values(&mut self, id1: HeapId, id2: HeapId) -> RunResult<Option<Value>> {
        let (seq_id, count) = match (self.get(id1), self.get(id2)) {
            (HeapData::LongInt(a), HeapData::LongInt(b)) => {
                check_mult_size(a.bits(), b.bits(), &self.tracker)?;
                let result = LongInt::new(a.inner() * b.inner());
                return Ok(Some(result.into_value(self)?));
            }
            (HeapData::LongInt(li), _) => {
                let count = longint_to_repeat_count(li)?;
                (id2, count)
            }
            (_, HeapData::LongInt(li)) => {
                let count = longint_to_repeat_count(li)?;
                (id1, count)
            }
            _ => return Ok(None),
        };

        self.mult_sequence(seq_id, count)
    }

    /// Multiplies (repeats) a sequence by an integer count.
    ///
    /// This method handles sequence repetition for Python's `*` operator when applied
    /// to sequences (str, bytes, list, tuple). It creates a new heap-allocated sequence
    /// with the elements repeated `count` times.
    ///
    /// # Arguments
    /// * `id` - HeapId of the sequence to repeat
    /// * `count` - Number of times to repeat (0 returns empty sequence)
    ///
    /// # Returns
    /// * `Ok(Some(Value))` - The new repeated sequence
    /// * `Ok(None)` - If the heap entry is not a sequence type
    /// * `Err` - If allocation fails due to resource limits
    pub fn mult_sequence(&mut self, id: HeapId, count: usize) -> RunResult<Option<Value>> {
        match self.get(id) {
            HeapData::Str(s) => {
                check_repeat_size(s.len(), count, &self.tracker)?;
                Ok(Some(Value::Ref(
                    self.allocate(HeapData::Str(s.as_str().repeat(count).into()))?,
                )))
            }
            HeapData::Bytes(b) => {
                check_repeat_size(b.len(), count, &self.tracker)?;
                Ok(Some(Value::Ref(
                    self.allocate(HeapData::Bytes(b.as_slice().repeat(count).into()))?,
                )))
            }
            HeapData::List(list) => {
                check_repeat_size(list.len().saturating_mul(size_of::<Value>()), count, &self.tracker)?;
                let mut result = Vec::with_capacity(list.as_slice().len() * count);
                for _ in 0..count {
                    result.extend(list.as_slice().iter().map(|v| v.clone_with_heap(self)));
                    self.check_time()?;
                }
                Ok(Some(Value::Ref(self.allocate(HeapData::List(List::new(result)))?)))
            }
            HeapData::Tuple(tuple) => {
                if count == 0 {
                    return Ok(Some(self.get_empty_tuple()));
                }
                check_repeat_size(
                    tuple.as_slice().len().saturating_mul(size_of::<Value>()),
                    count,
                    &self.tracker,
                )?;
                let mut result = SmallVec::with_capacity(tuple.as_slice().len() * count);
                for _ in 0..count {
                    result.extend(tuple.as_slice().iter().map(|v| v.clone_with_heap(self)));
                    self.check_time()?;
                }
                Ok(Some(allocate_tuple(result, self)?))
            }
            _ => Ok(None),
        }
    }

    /// Returns whether cycle collection should run.
    ///
    /// True when the number of pending [`Purple`](CcColor::Purple) candidates
    /// has reached the configured trigger threshold. Trial deletion's work is
    /// proportional to the candidate count, so this metric is a direct signal
    /// of how much the collector has to do — unlike the old "any
    /// allocation, any cycle" overestimate it replaces.
    #[inline]
    pub fn should_gc(&self) -> bool {
        let interval = self.tracker.gc_interval().unwrap_or(DEFAULT_GC_INTERVAL);
        self.purple_count >= interval
    }

    /// Runs Bacon–Rajan trial-deletion cycle collection.
    ///
    /// Walks every entry currently flagged [`Purple`](CcColor::Purple) (the
    /// candidates accumulated by `dec_ref`) and frees any references that turn
    /// out to live entirely inside an unreachable cycle. Refcount math itself
    /// proves liveness — entries reachable from outside the candidate set
    /// (including those held only on the Rust stack and those with active
    /// `HeapRead` readers) survive automatically because their refcount or
    /// reader count remains non-zero — so no explicit root walk is required.
    ///
    /// Phases:
    ///
    /// 1. **`MarkRoots`** — single linear pass over `entries` that finds
    ///    Purple entries, runs `MarkGray` on each, and collects the resulting
    ///    seed list. Purple entries reached transitively by an earlier seed's
    ///    `MarkGray` turn Gray and are correctly skipped, so each cycle root
    ///    is only seeded once.
    /// 2. **`Scan`** — for each seed, decide whether the subtree is alive
    ///    (`s.refcount > 0 || s.readers > 0`, resurrect to Black) or condemned
    ///    (mark White and recurse).
    /// 3. **`CollectWhite`** — free White entries iteratively. Child
    ///    refcounts were already balanced by `MarkGray`/`ScanBlack`, so this
    ///    phase does **not** call `dec_ref` on children — it only walks them
    ///    to free transitively.
    ///
    /// All four phases iterate via explicit work stacks instead of recursion
    /// (the textbook formulation is recursive); a 10 000-deep nested cycle
    /// must collect without a Rust stack overflow.
    ///
    /// # Caller Responsibility
    /// The caller should check [`should_gc`](Self::should_gc) before calling
    /// this method. With `purple_count == 0` the function returns immediately
    /// without touching the heap.
    pub fn collect_cycles(&mut self) {
        if self.purple_count == 0 {
            return;
        }

        let seeds = self.mark_roots();
        self.scan_roots(&seeds);
        self.collect_roots(&seeds);

        // After `MarkRoots` no Purple entries remain in the heap; confirm the
        // invariant and zero the counter so the next `dec_ref` event re-seeds
        // from a clean baseline.
        self.purple_count = 0;
    }

    /// `MarkRoots`: linear pass over entries, run `MarkGray` on each Purple
    /// seed, return the seed list.
    ///
    /// Purple entries reached transitively by an earlier seed's `MarkGray`
    /// flip to Gray before the iterator visits them, so the
    /// `entry.color.get() == Purple` check naturally dedupes — the dominator
    /// seed handles the whole subtree.
    fn mark_roots(&mut self) -> Vec<HeapId> {
        // Collect Purple seed IDs without holding a borrow on `self` so we can
        // mutate refcounts during the subsequent `mark_gray` pass.
        let seeds: Vec<HeapId> = self
            .entries
            .iter()
            .filter_map(|(id, entry)| (entry.color.get() == CcColor::Purple).then_some(id))
            .collect();

        let mut filtered_seeds = Vec::with_capacity(seeds.len());
        let mut work_stack = Vec::new();
        for id in seeds {
            // Re-check Purple: an earlier seed's `mark_gray` may have already
            // visited this entry and flipped its color to Gray.
            let entry = self.entries.get(id.index());
            if entry.color.get() == CcColor::Purple {
                self.mark_gray(id, &mut work_stack);
                filtered_seeds.push(id);
            }
        }
        filtered_seeds
    }

    /// `MarkGray` (iterative): paint `s` and its transitive children Gray,
    /// decrementing each child's refcount once per traversal edge.
    ///
    /// After this completes for every seed, every Gray entry's refcount equals
    /// the count of *external* references into it (refs originating outside
    /// the candidate subgraph). `Scan` uses that property to decide
    /// alive/condemned.
    fn mark_gray(&self, start: HeapId, work_stack: &mut Vec<HeapId>) {
        debug_assert!(work_stack.is_empty());
        work_stack.push(start);
        while let Some(id) = work_stack.pop() {
            let entry = self.entries.get(id.index());
            if entry.color.get() == CcColor::Gray {
                continue;
            }
            entry.color.set(CcColor::Gray);
            // SAFETY: read-only walk of children via `&Heap`. No mutable
            // borrows into heap data exist on this code path.
            let data = unsafe { &*entry.data.0.get() };
            let len_before = work_stack.len();
            collect_child_ids(data, work_stack);
            // Decrement each newly enqueued child's refcount. The recursion
            // (push back onto the stack) is what gives us the depth-first
            // descent; the rc-- happens *before* we recurse, matching the
            // textbook Bacon–Rajan ordering.
            for child_id in &work_stack[len_before..] {
                let child = self.entries.get(child_id.index());
                debug_assert!(
                    child.refcount.get() > 0,
                    "mark_gray: child refcount underflow at HeapId({})",
                    child_id.index(),
                );
                child.refcount.update(|r| r - 1);
            }
        }
    }

    /// `Scan` over every seed: resurrect alive subtrees (Black) or condemn
    /// dead ones (White).
    fn scan_roots(&mut self, seeds: &[HeapId]) {
        let mut work_stack = Vec::new();
        for &seed in seeds {
            self.scan(seed, &mut work_stack);
        }
    }

    /// `Scan` (iterative): each Gray entry is either resurrected via
    /// `ScanBlack` (external reference exists — refcount > 0 or active
    /// `HeapRead` reader) or painted White and its Gray children recursed.
    fn scan(&self, start: HeapId, work_stack: &mut Vec<HeapId>) {
        debug_assert!(work_stack.is_empty());
        work_stack.push(start);
        while let Some(id) = work_stack.pop() {
            let entry = self.entries.get(id.index());
            if entry.color.get() != CcColor::Gray {
                continue;
            }
            if entry.refcount.get() > 0 || entry.readers.get() > 0 {
                // External reference exists (either a refcount we couldn't
                // account for inside the candidate set, or a live `HeapRead`
                // pointing into the entry). Resurrect this entry and its
                // transitive Gray children back to Black.
                self.scan_black(id);
            } else {
                entry.color.set(CcColor::White);
                // SAFETY: read-only walk of children.
                let data = unsafe { &*entry.data.0.get() };
                collect_child_ids(data, work_stack);
            }
        }
    }

    /// `ScanBlack` (iterative): resurrect a subtree by re-incrementing
    /// children's refcounts that `MarkGray` previously decremented, restoring
    /// the heap to the state it would have had if no cycle was suspected.
    ///
    /// Children's refcounts are incremented once per traversal edge — even if
    /// the child is already Black — so multi-edge graphs (a child reachable
    /// from two parents in the resurrected subtree) balance the matching
    /// per-edge decrements `MarkGray` performed. Recursion only descends into
    /// non-Black children so each entry is processed at most once.
    fn scan_black(&self, start: HeapId) {
        let mut work_stack = vec![start];
        let mut children_buf = Vec::new();
        while let Some(id) = work_stack.pop() {
            let entry = self.entries.get(id.index());
            if entry.color.get() == CcColor::Black {
                // Already processed via another edge — skip to avoid
                // re-walking children (which would double-increment
                // grandchildren).
                continue;
            }
            entry.color.set(CcColor::Black);
            // SAFETY: read-only walk of children.
            let data = unsafe { &*entry.data.0.get() };
            children_buf.clear();
            collect_child_ids(data, &mut children_buf);
            for &child_id in &children_buf {
                let child = self.entries.get(child_id.index());
                child.refcount.update(|r| r + 1);
                if child.color.get() != CcColor::Black {
                    work_stack.push(child_id);
                }
            }
        }
    }

    /// `CollectRoots` + `CollectWhite` (iterative): free every entry painted
    /// White by `Scan`, walking transitively through White children.
    ///
    /// Refcounts are not adjusted on children: `MarkGray` decremented and
    /// `ScanBlack` re-incremented in balance, so any Black child of a White
    /// parent has its rc already correctly reflecting the lost edge from the
    /// freed parent. Children that are themselves White are about to be freed
    /// and don't need rc adjustments either.
    ///
    /// `py_dec_ref_ids_for_data` is used to walk children — under
    /// `memory-model-checks` it has the side effect of marking child
    /// `Value::Ref`s as `Dereferenced`, which prevents the panic that would
    /// otherwise fire when the freed entry's data is dropped with live
    /// `Value::Ref` payloads.
    fn collect_roots(&mut self, seeds: &[HeapId]) {
        let mut work_stack = Vec::new();
        for &seed in seeds {
            self.collect_white(seed, &mut work_stack);
        }
    }

    fn collect_white(&mut self, start: HeapId, work_stack: &mut Vec<HeapId>) {
        debug_assert!(work_stack.is_empty());
        work_stack.push(start);
        while let Some(id) = work_stack.pop() {
            let slot = self.entries.get_mut(id.index());
            let Some(entry) = slot.as_ref() else {
                // Already freed via another seed's traversal — ignore.
                continue;
            };
            if entry.color.get() != CcColor::White {
                // Either resurrected to Black by `Scan` or never visited
                // (still Black/Gray from somewhere). Don't free.
                continue;
            }
            debug_assert!(
                entry.readers.get() == 0,
                "collect_white: cannot free HeapId({}) with {} active reader(s)",
                id.index(),
                entry.readers.get(),
            );
            let mut value = slot.take().expect("collect_white: slot vanished after color check");
            self.entries.free(id);
            self.tracker.on_free(|| value.data.0.get_mut().py_estimate_size());
            // Walk children, marking child `Value::Ref`s as `Dereferenced`
            // under `memory-model-checks` so dropping the freed entry's data
            // doesn't trip a Drop-panic on a live `Value::Ref` payload. The
            // pushed child IDs feed the work stack so we recursively walk
            // White grandchildren — we do *not* `dec_ref` these children
            // (`MarkGray`/`ScanBlack` already balanced their refcounts).
            py_dec_ref_ids_for_data(value.data.0.get_mut(), work_stack);
        }
    }
}

/// Computes the number of significant bits in an `i64`.
///
/// Returns 0 for zero, otherwise returns the position of the highest set bit
/// plus one. Uses unsigned absolute value to handle negative numbers correctly.
fn i64_bits(value: i64) -> u64 {
    if value == 0 {
        0
    } else {
        u64::from(64 - value.unsigned_abs().leading_zeros())
    }
}

/// Converts an `i64` repeat count to `usize` for sequence repetition.
///
/// Returns 0 for negative values (Python treats negative repeat counts as 0).
/// Returns `OverflowError` if the value exceeds `usize::MAX`.
fn i64_to_repeat_count(n: i64) -> RunResult<usize> {
    if n <= 0 {
        Ok(0)
    } else {
        usize::try_from(n).map_err(|_| ExcType::overflow_repeat_count().into())
    }
}

/// Converts a `LongInt` repeat count to `usize` for sequence repetition.
///
/// Returns 0 for negative values (Python treats negative repeat counts as 0).
/// Returns `OverflowError` if the value exceeds `usize::MAX`.
fn longint_to_repeat_count(li: &LongInt) -> RunResult<usize> {
    if li.is_negative() {
        Ok(0)
    } else if let Some(count) = li.to_usize() {
        Ok(count)
    } else {
        Err(ExcType::overflow_repeat_count().into())
    }
}

/// Collects child HeapIds from a HeapData value for GC traversal.
fn collect_child_ids(data: &HeapData, work_list: &mut Vec<HeapId>) {
    match data {
        HeapData::List(list) => {
            // Skip iteration if no refs - major GC optimization for lists of primitives
            if !list.contains_refs() {
                return;
            }
            for value in list.as_slice() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Tuple(tuple) => {
            // Skip iteration if no refs - GC optimization for tuples of primitives
            if !tuple.contains_refs() {
                return;
            }
            for value in tuple.as_slice() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::NamedTuple(nt) => {
            // Skip iteration if no refs - GC optimization for namedtuples of primitives
            if !nt.contains_refs() {
                return;
            }
            for value in nt.as_vec() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Dict(dict) => {
            // Skip iteration if no refs - major GC optimization for dicts of primitives
            if !dict.has_refs() {
                return;
            }
            for (k, v) in dict {
                if let Value::Ref(id) = k {
                    work_list.push(*id);
                }
                if let Value::Ref(id) = v {
                    work_list.push(*id);
                }
            }
        }
        HeapData::DictKeysView(view) => {
            work_list.push(view.dict_id());
        }
        HeapData::DictItemsView(view) => {
            work_list.push(view.dict_id());
        }
        HeapData::DictValuesView(view) => {
            work_list.push(view.dict_id());
        }
        HeapData::Set(set) => {
            for value in set.storage().iter() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::FrozenSet(frozenset) => {
            for value in frozenset.storage().iter() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Closure(closure) => {
            // Add captured cells to work list
            for cell_id in &closure.cells {
                work_list.push(*cell_id);
            }
            // Add default values that are heap references
            for default in &closure.defaults {
                if let Value::Ref(id) = default {
                    work_list.push(*id);
                }
            }
        }
        HeapData::FunctionDefaults(fd) => {
            // Add default values that are heap references
            for default in &fd.defaults {
                if let Value::Ref(id) = default {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Cell(cell) => {
            // Cell can contain a reference to another heap value
            if let Value::Ref(id) = &cell.0 {
                work_list.push(*id);
            }
        }
        HeapData::Dataclass(dc) => {
            // Dataclass attrs are stored in a Dict - iterate through entries
            for (k, v) in dc.attrs() {
                if let Value::Ref(id) = k {
                    work_list.push(*id);
                }
                if let Value::Ref(id) = v {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Iter(iter) => {
            // Iterator holds a reference to the iterable being iterated
            if let Value::Ref(id) = iter.value() {
                work_list.push(*id);
            }
        }
        HeapData::Module(m) => {
            // Module attrs can contain references to heap values
            if !m.has_refs() {
                return;
            }
            for (k, v) in m.attrs() {
                if let Value::Ref(id) = k {
                    work_list.push(*id);
                }
                if let Value::Ref(id) = v {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Coroutine(coro) => {
            // Add namespace values that are heap references
            for value in &coro.namespace {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::GatherFuture(gather) => {
            // Add coroutine HeapIds to work list
            for item in &gather.items {
                if let GatherItem::Coroutine(coro_id) = item {
                    work_list.push(*coro_id);
                }
            }
            // Add result values that are heap references
            for result in gather.results.iter().flatten() {
                if let Value::Ref(id) = result {
                    work_list.push(*id);
                }
            }
        }
        // Leaf types with no heap references
        _ => {}
    }
}

fn py_dec_ref_ids_for_data(data: &mut HeapData, stack: &mut Vec<HeapId>) {
    match data {
        HeapData::Str(s) => s.py_dec_ref_ids(stack),
        HeapData::Bytes(b) => b.py_dec_ref_ids(stack),
        HeapData::List(l) => l.py_dec_ref_ids(stack),
        HeapData::Tuple(t) => t.py_dec_ref_ids(stack),
        HeapData::NamedTuple(nt) => nt.py_dec_ref_ids(stack),
        HeapData::Dict(d) => d.py_dec_ref_ids(stack),
        HeapData::DictKeysView(view) => view.py_dec_ref_ids(stack),
        HeapData::DictItemsView(view) => view.py_dec_ref_ids(stack),
        HeapData::DictValuesView(view) => view.py_dec_ref_ids(stack),
        HeapData::Set(s) => s.py_dec_ref_ids(stack),
        HeapData::FrozenSet(fs) => fs.py_dec_ref_ids(stack),
        HeapData::Closure(closure) => {
            // Decrement ref count for captured cells
            stack.extend(closure.cells.iter().copied());
            // Decrement ref count for default values that are heap references
            for default in &mut closure.defaults {
                default.py_dec_ref_ids(stack);
            }
        }
        HeapData::FunctionDefaults(fd) => {
            // Decrement ref count for default values that are heap references
            for default in &mut fd.defaults {
                default.py_dec_ref_ids(stack);
            }
        }
        HeapData::Cell(cell) => cell.0.py_dec_ref_ids(stack),
        HeapData::Dataclass(dc) => dc.py_dec_ref_ids(stack),
        HeapData::Iter(iter) => iter.py_dec_ref_ids(stack),
        HeapData::Module(m) => m.py_dec_ref_ids(stack),
        HeapData::Coroutine(coro) => {
            // Decrement ref count for namespace values that are heap references
            for value in &mut coro.namespace {
                value.py_dec_ref_ids(stack);
            }
        }
        HeapData::GatherFuture(gather) => {
            // Decrement ref count for coroutine HeapIds
            for item in &gather.items {
                if let GatherItem::Coroutine(id) = item {
                    stack.push(*id);
                }
            }
            // Decrement ref count for result values that are heap references
            for result in gather.results.iter_mut().flatten() {
                result.py_dec_ref_ids(stack);
            }
        }
        // other types have no nested heap references
        _ => {}
    }
}

/// Compile-fail soundness tests for [`HeapReader`].
///
/// Gated behind `--cfg heap_reader_compile_fail_tests` so they are only compiled
/// when the integration test harness runs `cargo check` with the appropriate flags.
#[cfg(heap_reader_compile_fail_tests)]
#[path = "../tests/heap_reader_compile_fail_cases/cases.rs"]
mod heap_reader_compile_fail_cases;

/// Cycle-collector unit tests.
///
/// These live inside `heap.rs` (rather than under `crates/monty/tests/`)
/// because they need to manipulate `Heap` state directly — building a cycle
/// without a VM, peeking at `purple_count`, and rooting an entry only via a
/// Rust local binding. The integration-test surface only exposes
/// Python-driven execution and cannot construct any of those scenarios.
///
/// In particular, the [`cstack_only_cycle_survives_collection`] test
/// validates the central correctness property of trial deletion: a heap
/// entry referenced *only* from the Rust C stack survives a cycle
/// collection because its non-zero refcount is itself proof of liveness.
/// That behavior was previously a known soundness gap of the explicit-roots
/// mark–sweep collector.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{resource::NoLimitTracker, types::List, value::Value};

    /// Returns whether a heap entry is still allocated at `id`.
    fn is_alive<T: ResourceTracker>(heap: &Heap<T>, id: HeapId) -> bool {
        heap.entries.iter().any(|(other, _)| other == id)
    }

    /// Allocates a self-referencing one-element list and returns its id.
    ///
    /// The list's items become `[Value::Ref(id)]` and its refcount is bumped
    /// to 2 to reflect both the caller's ref and the new self-reference.
    fn alloc_self_cycle(heap: &Heap<NoLimitTracker>) -> HeapId {
        let id = heap.allocate(HeapData::List(List::new(vec![]))).unwrap();
        let entry = heap
            .entries
            .iter()
            .find(|(other, _)| *other == id)
            .map(|(_, e)| e)
            .expect("entry just allocated");
        // SAFETY: no other borrow into this entry's data exists during the test.
        let data = unsafe { &mut *entry.data.0.get() };
        match data {
            HeapData::List(list) => {
                list.set_contains_refs();
                list.as_vec_mut().push(Value::Ref(id));
            }
            _ => unreachable!(),
        }
        // The new self-pointer counts as one more reference into the entry.
        heap.inc_ref(id);
        id
    }

    #[test]
    fn cstack_only_cycle_survives_collection() {
        let mut heap = Heap::<NoLimitTracker>::new(16, NoLimitTracker);
        let id = alloc_self_cycle(&heap);

        // Simulate a Rust-side local `Value::Ref` binding by bumping the
        // refcount one extra time. Then `dec_ref` it back down to 2 — that
        // dec_ref is what enrolls the entry as a Purple candidate, mimicking
        // exactly the situation under the old GC where the local binding
        // wasn't published in any explicit root set.
        heap.inc_ref(id); // rc = 3
        heap.dec_ref(id); // rc = 2, flagged Purple
        assert_eq!(heap.purple_count, 1);

        // Cycle collection must not free the entry: the local "C-stack" ref
        // contributes one of its two surviving refcount units, so trial
        // deletion sees rc > 0 after MarkGray and resurrects the subtree.
        heap.collect_cycles();
        assert_eq!(heap.purple_count, 0);
        assert!(is_alive(&heap, id), "C-stack-rooted cycle was freed");
        assert!(matches!(heap.get(id), HeapData::List(_)));

        // Drop the simulated Rust local. Now the cycle is genuinely isolated
        // (rc 1 = self-pointer only). The next collection must reclaim it.
        heap.dec_ref(id); // rc = 1, re-flagged Purple
        assert_eq!(heap.purple_count, 1);
        heap.collect_cycles();
        assert_eq!(heap.purple_count, 0);
        assert!(!is_alive(&heap, id), "isolated cycle should have been freed");
    }

    #[test]
    fn heap_read_rooted_cycle_survives_collection() {
        let mut heap = Heap::<NoLimitTracker>::new(16, NoLimitTracker);
        let id = alloc_self_cycle(&heap);

        // Bump `readers` manually to mimic a live `HeapRead` pointing into
        // the entry. The borrow checker prevents holding a real `HeapRead`
        // across `collect_cycles` (which requires `&mut Heap`), so we
        // splice the same counter that `HeapRead::Drop` decrements.
        let readers_before = heap.entries.get(id.index()).readers.get();
        heap.entries.get(id.index()).readers.set(readers_before + 1);

        // Drive the entry into Purple via dec_ref: rc 2 → 1. Without the
        // `readers > 0` special-case in `Scan`, the resulting cycle would
        // be condemned to White and freed.
        heap.dec_ref(id); // rc = 1, flagged Purple
        assert_eq!(heap.purple_count, 1);

        heap.collect_cycles();
        assert!(
            is_alive(&heap, id),
            "entry with active HeapRead reader was freed by collect_cycles"
        );

        // Restore the simulated reader so `Heap::drop` can clean up
        // without tripping the `dec_ref` active-readers assertion.
        heap.entries.get(id.index()).readers.set(readers_before);
        // The entry is leaked here on purpose (rc = 1 from the self-ref,
        // no external root remains, but the collector ran already and the
        // color is Black — the next dec_ref would try to recurse into the
        // self-pointer after freeing the entry). `Heap::drop` walks every
        // slot and tears them down regardless of refcount, so leaking
        // here is safe for the duration of the test.
    }

    #[test]
    fn isolated_simple_cycle_is_collected() {
        // Sanity check: a self-reference cycle with no external rooting
        // gets collected on the next `collect_cycles` call.
        let mut heap = Heap::<NoLimitTracker>::new(16, NoLimitTracker);
        let id = alloc_self_cycle(&heap);
        // After alloc_self_cycle: rc = 2 (allocate's 1 + self-ref's 1).
        // Drop the caller's reference. rc 2 → 1, marks Purple.
        heap.dec_ref(id);
        assert_eq!(heap.purple_count, 1);
        heap.collect_cycles();
        assert!(!is_alive(&heap, id));
        assert_eq!(heap.purple_count, 0);
    }

    #[test]
    fn empty_tuple_singleton_survives_collection() {
        // The empty-tuple singleton is no longer rooted explicitly by the
        // collector. Its refcount stays ≥ 1 forever (initial heap-owned
        // ref), which is what keeps it alive — verify the collector does
        // not accidentally free it even after spurious Purple flagging.
        let mut heap = Heap::<NoLimitTracker>::new(16, NoLimitTracker);
        // Fake a dec_ref event that would mark the empty tuple Purple.
        heap.inc_ref(EMPTY_TUPLE_ID);
        heap.dec_ref(EMPTY_TUPLE_ID);
        heap.collect_cycles();
        assert!(
            is_alive(&heap, EMPTY_TUPLE_ID),
            "empty tuple singleton must survive collection"
        );
    }

    #[test]
    fn pending_purple_cycle_round_trips_through_serde() {
        // A snapshot can be taken between any two bytecode instructions, so
        // entries flagged Purple by `dec_ref` but not yet visited by the
        // collector must survive serde round-trips. Otherwise a cycle that
        // becomes garbage just before snapshot would leak permanently after
        // restore (the post-restore VM would never re-touch it).
        let mut heap = Heap::<NoLimitTracker>::new(16, NoLimitTracker);
        let id = alloc_self_cycle(&heap);
        // Drop the caller's external ref so the entry is genuinely
        // unreachable except via its self-pointer. dec_ref flags Purple.
        heap.dec_ref(id); // rc 2 → 1
        assert_eq!(heap.purple_count, 1);
        let pre_color = heap.entries.get(id.index()).color.get();
        assert_eq!(pre_color, CcColor::Purple);

        // Round-trip through postcard.
        let bytes = postcard::to_allocvec(&heap).expect("serialize");
        let mut restored: Heap<NoLimitTracker> = postcard::from_bytes(&bytes).expect("deserialize");

        // `purple_count` and the per-entry color must round-trip.
        assert_eq!(restored.purple_count, 1);
        assert_eq!(restored.entries.get(id.index()).color.get(), CcColor::Purple);

        // Run the collector on the restored heap; the cycle is unreachable
        // and must be reclaimed.
        restored.collect_cycles();
        assert!(!is_alive(&restored, id));
        assert_eq!(restored.purple_count, 0);
    }
}
