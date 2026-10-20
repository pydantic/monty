#![doc = include_str!("README.md")]
#![expect(
    unsafe_code,
    reason = "Paged arena hands out typed, aliasable views into UnsafeCell entries"
)]

#[cfg(feature = "ref-count-return")]
use std::collections::HashSet;
use std::{
    cell::{Cell, UnsafeCell},
    collections::BTreeMap,
    fmt,
    iter::once,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    sync::Arc,
};

use monty_types::{DEFAULT_GC_INTERVAL, MontyUuid, ResourceTracker};
use serde::{Deserialize, ser::SerializeStruct};

#[cfg(feature = "ref-count-return")]
use crate::types::Type;
use crate::{
    asyncio::{Awaiter, ExternalFutureState, GatherState},
    types::{ExtFunction, HostClassType, TimeZone, Tuple},
    value::Value,
};
// Re-export items moved to `heap_traits` so that `crate::heap::DropGuard` etc. continue
// to resolve (used by the `defer_drop!` macros and throughout the codebase).
pub(crate) use crate::{
    heap_data::HeapData,
    heap_traits::{ContainsHeap, DropGuard, DropWithContext, HeapItem},
};

mod free_list;
mod object_ptr;
mod owner;
mod reference;
pub(crate) mod relocate;
mod stable_heap;
use object_ptr::HeapEntries;
pub use object_ptr::HeapObjectPtr as HeapId;
pub(crate) use owner::HeapOwner;
pub(crate) use reference::{Reference, RuntimeRef};
pub(crate) use stable_heap::StableHeap;

/// Dense index for pointer-free snapshot graphs, arena bookkeeping and intern tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct HeapIndex(usize);

impl HeapIndex {
    /// Creates an arena index without granting pointer access.
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

/// The empty tuple occupies the first permanent slot of every runtime arena.
const EMPTY_TUPLE_INDEX: usize = 0;

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
    #[serde(rename = "B")]
    Black,
    /// Visited by `MarkGray` during a collection cycle. Children's refcounts
    /// have been provisionally decremented; a later `Scan` pass decides whether
    /// to resurrect (back to [`Black`](Self::Black)) or condemn
    /// ([`White`](Self::White)) the entry.
    #[serde(rename = "G")]
    Gray,
    /// Confirmed unreachable by the current collection: every reference into
    /// the entry comes from another condemned entry. `CollectWhite` will free
    /// it. Only seen mid-collection.
    #[serde(rename = "W")]
    White,
    /// Candidate cycle root. Set by `dec_ref` whenever a GC-tracked entry's
    /// refcount drops to a non-zero value — the only situation in which a new
    /// reference cycle can become unreachable. The collector seeds its work
    /// from every entry currently flagged Purple.
    #[serde(rename = "P")]
    Purple,
}

/// Scoped access token for guarded payload reads and mutation.
///
/// Handles cache typed payload locations while short borrows of this reader serialize access.
/// Its invariant lifetime prevents handles from being used with another reader.
/// Object guards retain a reader count; protected capabilities retain their source borrow.
/// See [`HeapRead::get`] for the payload-liveness and aliasing guarantees.
pub(crate) struct HeapReader<'a, H: RuntimeRef> {
    pub(crate) heap: &'a mut Heap<H>,
    /// Makes the lifetime `'a` invariant.
    phantom: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<H: RuntimeRef> HeapReader<'_, H> {
    /// The ONLY way to get a `HeapReader`. By only providing an API which takes a closure which
    /// must be satisfied for all `'a`, it's impossible to create other `HeapReader` with the
    /// exact same lifetime `'a`.
    ///
    /// To allow other data to be borrowed alongside the `HeapReader`, the closure is given a
    /// `&'a mut D` with the same lifetime as the `HeapReader`, which is forwarded from the
    /// `&mut D` passed to this function. This rebranding lets callers (most notably the
    /// `VM`) hold borrows whose lifetime matches the `HeapReader`'s invariant brand `'a`,
    /// which is what allows the `VM` to be parameterized by a single lifetime.
    pub fn with<R, D: ?Sized>(
        heap: &mut Heap<H>,
        data: &mut D,
        f: impl for<'a> FnOnce(&'a mut HeapReader<'a, H>, &'a mut D) -> R,
    ) -> R {
        f(
            &mut HeapReader {
                heap: &mut *heap,
                phantom: PhantomData,
            },
            data,
        )
    }
}

impl<'a, H: RuntimeRef> HeapReader<'a, H> {
    /// Associates an arena-branded pointer with this reader's independent invariant scope.
    pub(crate) fn read_ptr(&self, id: H) -> HeapPtr<'a> {
        HeapPtr {
            inner: self.heap.entries.pointer(id).cast(),
            brand: PhantomData,
        }
    }

    /// Borrows a live payload under this reader's scope, panicking if its slot is vacant.
    pub fn read(&self, id: H) -> HeapReadOutput<'a, H> {
        self.read_ptr(id).read(id, self)
    }

    /// Reads `id` as the requested concrete payload type.
    ///
    /// Returns `None` when the live entry stores a different payload type.
    pub fn read_as<T: HeapPayload<H>>(&self, id: H) -> Option<HeapObjectRead<'a, H, T>> {
        T::try_from_read(self.read(id)).ok()
    }

    /// Allocates a concrete payload and returns an owning, typed handle to it.
    ///
    /// The allocation's initial reference must eventually be transferred with
    /// `into_value`/`into_id`, or released by guarding the handle with `DropGuard`.
    pub fn allocate_as<T: HeapPayload<H>>(&self, value: T) -> HeapAllocation<'a, H, T> {
        let entry = self.heap.new_entry(value.into_heap_data());
        let (id, slot) = self.heap.entries.allocate_with_slot(entry);
        HeapAllocation {
            id,
            ptr: HeapPtr {
                inner: slot.cast(),
                brand: PhantomData,
            },
            read: None,
        }
    }

    /// Brands a shared external borrow without granting mutable access to its source.
    #[expect(clippy::unused_self, reason = "'a lifetime is used to create the safety guarantees")]
    pub fn protect<'t, U: ?Sized>(&mut self, value: &'t U) -> BorrowedHeapRead<'t, 'a, U> {
        BorrowedHeapRead {
            inner: HeapRead {
                value: NonNull::from(value),
                borrow: PhantomData,
            },
            original: PhantomData,
        }
    }

    /// Brands an exclusive external borrow; payload access is serialized by this reader.
    #[expect(clippy::unused_self, reason = "'a lifetime is used to create the safety guarantees")]
    pub fn protect_mut<'t, U: ?Sized>(&mut self, value: &'t mut U) -> BorrowedHeapReadMut<'t, 'a, U> {
        BorrowedHeapReadMut {
            inner: HeapReadMut {
                read: HeapRead {
                    value: NonNull::from(value),
                    borrow: PhantomData,
                },
            },
            original: PhantomData,
        }
    }
}

impl<H: RuntimeRef> ContainsHeap for HeapReader<'_, H> {
    type Ref = H;

    fn heap(&self) -> &Heap<H> {
        self.heap.heap()
    }
    fn heap_mut(&mut self) -> &mut Heap<H> {
        self.heap.heap_mut()
    }
}

impl<H: RuntimeRef> Deref for HeapReader<'_, H> {
    type Target = Heap<H>;

    fn deref(&self) -> &Self::Target {
        self.heap
    }
}

impl<H: RuntimeRef> DerefMut for HeapReader<'_, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.heap
    }
}

macro_rules! heap_storage_value {
    (inline $value:expr) => {
        $value
    };
    (boxed $value:expr) => {
        Box::new($value)
    };
    (boxed32 $value:expr) => {{
        #[cfg(target_pointer_width = "32")]
        {
            Box::new($value)
        }
        #[cfg(target_pointer_width = "64")]
        {
            $value
        }
    }};
}

/// Maps a concrete Rust payload to its heap representation and typed read variant.
///
/// Implementations are generated from `heap_data::heap_payloads`, keeping each
/// payload's storage conversion and typed read extraction in the central registry.
pub(crate) trait HeapPayload<H: RuntimeRef>: Sized {
    /// Wraps this payload in its corresponding `HeapData` variant.
    fn into_heap_data(self) -> HeapData<H>;

    /// Extracts this payload's typed handle from a dynamic heap read.
    fn try_from_read(read: HeapReadOutput<'_, H>) -> Result<HeapObjectRead<'_, H, Self>, HeapReadOutput<'_, H>>;
}

macro_rules! define_heap_read_support {
    ($h:ident; $(
        $(#[$meta:meta])*
        $variant:ident($storage:ident $payload:ty)
    ),* $(,)?) => {
        /// A type-safe read handle for any payload stored in the heap.
        /// Variants mirror `HeapData`, which carries their docs; its serde attributes
        /// would not compile here, so the registry's attributes are not repeated.
        pub enum HeapReadOutput<'a, $h: Reference> {
            $(
                $variant(HeapObjectRead<'a, $h, $payload>),
            )*
        }

        impl<$h: RuntimeRef> HeapReadOutput<'_, $h> {
            /// Returns the heap entry containing the complete dynamically read object.
            pub fn id(&self) -> $h {
                match self {
                    $(Self::$variant(value) => value.id(),)*
                }
            }
        }

        $(
            impl<$h: RuntimeRef> HeapPayload<$h> for $payload {
                #[inline]
                fn into_heap_data(self) -> HeapData<$h> {
                    HeapData::$variant(heap_storage_value!($storage self))
                }

                #[inline]
                fn try_from_read(
                    read: HeapReadOutput<'_, $h>,
                ) -> Result<HeapObjectRead<'_, $h, Self>, HeapReadOutput<'_, $h>> {
                    if let HeapReadOutput::$variant(value) = read {
                        Ok(value)
                    } else {
                        Err(read)
                    }
                }
            }
        )*

    };
}

crate::heap_data::heap_payloads!(define_heap_read_support, H);

/// A newly allocated heap object with its initial owned reference.
///
/// Typed access is created lazily by [`Self::read`], so transferring an allocation
/// directly into a `Value` or raw ID does not touch the entry's reader count.
#[must_use = "the allocation's initial reference must be transferred"]
pub struct HeapAllocation<'a, H: Reference, T> {
    id: H,
    ptr: HeapPtr<'a>,
    read: Option<HeapObjectRead<'a, H, T>>,
}

impl<'a, H: RuntimeRef, T: HeapPayload<H>> HeapAllocation<'a, H, T> {
    /// Returns the allocated object's identity without transferring ownership.
    pub fn id(&self) -> H {
        self.id
    }

    /// Creates the typed read handle on first access and reuses it thereafter.
    pub fn read(&mut self, heap: &HeapReader<'a, H>) -> &mut HeapObjectRead<'a, H, T> {
        let id = self.id;
        let ptr = self.ptr;
        self.read.get_or_insert_with(|| {
            T::try_from_read(ptr.read(id, heap))
                .unwrap_or_else(|_| unreachable!("allocated payload has its registered type"))
        })
    }

    /// Transfers the allocation's initial reference into a `Value`.
    pub fn into_value(self) -> Value<H> {
        Value::Ref(self.into_id())
    }

    /// Transfers the allocation's initial reference as an owned raw ID.
    pub fn into_id(mut self) -> H {
        let id = self.id();
        drop(self.read.take());
        id
    }
}

impl<H: RuntimeRef, C: ContainsHeap<Ref = H>, T> DropWithContext<C> for HeapAllocation<'_, H, T> {
    fn drop_with(mut self, ctx: &mut C) {
        drop(self.read.take());
        Value::Ref(self.id).drop_with(ctx);
    }
}

/// A typed read handle for a Python object stored in a specific heap entry.
///
/// Unlike [`HeapRead`], this always represents the complete object rather than
/// protected external data or a projected field, so its [`HeapId`] is available
/// to operations that need Python identity or an owned reference to `self`.
pub struct HeapObjectRead<'a, H: Reference, T: ?Sized> {
    /// Identity of the complete Python object exposed by `read`.
    id: H,
    /// Mutable capability; never exposed by value or mutable reference.
    read: HeapReadMut<'a, T>,
    /// Owned reader count, keeping this entry alive until the guard is dropped.
    readers: NonNull<Cell<usize>>,
}

impl<'a, H: RuntimeRef, T: ?Sized> HeapObjectRead<'a, H, T> {
    /// Returns the heap entry containing this object.
    pub fn id(&self) -> H {
        self.id
    }

    /// Returns a new owned reference to this object.
    pub fn clone_value(&self, heap: &HeapReader<'a, H>) -> Value<H> {
        heap.inc_ref(self.id);
        Value::Ref(self.id)
    }
}

impl<'a, H: Reference, T: ?Sized> Deref for HeapObjectRead<'a, H, T> {
    type Target = HeapReadMut<'a, T>;

    fn deref(&self) -> &Self::Target {
        &self.read
    }
}

impl<H: Reference, T: ?Sized> Drop for HeapObjectRead<'_, H, T> {
    fn drop(&mut self) {
        // SAFETY: the paged storage keeps the counter's address stable, and this
        // owned reader count prevents the entry from being freed before the guard.
        let cell = unsafe { self.readers.as_ref() };
        cell.set(cell.get() - 1);
    }
}

/// A typed, identity-free view protected by a [`HeapReader`].
///
/// Complete heap objects are exposed as [`HeapObjectRead`]; this lower-level
/// handle also supports projected fields and data protected by `protect`.
pub struct HeapRead<'a, T: ?Sized> {
    value: NonNull<T>,
    /// Makes the lifetime `'a` invariant. In combination with the invariant lifetime
    /// on `HeapReader` and the `HeapReader::with` API, this guarantees that this
    /// `HeapRead` originated from that matching `HeapReader` (there is no way to
    /// construct another `HeapReader` with the same lifetime).
    borrow: PhantomData<fn(&'a T) -> &'a T>,
}

/// Aliasable mutable-access capability, used only through shared references.
///
/// The matching reader serializes payload access. Owners expose no `DerefMut`,
/// extraction or cloning API: replacing this capability could detach its pointer
/// from the source lifetime or reader count held by its owner.
pub struct HeapReadMut<'a, T: ?Sized> {
    read: HeapRead<'a, T>,
}

impl<'a, T: ?Sized> Deref for HeapReadMut<'a, T> {
    type Target = HeapRead<'a, T>;

    fn deref(&self) -> &Self::Target {
        &self.read
    }
}

impl<'a, T: ?Sized> HeapRead<'a, T> {
    /// Borrows the value while both this handle and the reader remain borrowed.
    ///
    /// Borrowing the handle also keeps protected external data and projection sources alive.
    pub fn get<'r, H: RuntimeRef>(&'r self, _: &'r HeapReader<'a, H>) -> &'r T {
        // SAFETY: (DH)
        //  - The HeapReader has an invariant lifetime 'a which guarantees that this HeapRead
        //    came from the heap borrowed by this HeapReader.
        //  - The address of the `HeapValue` never changes because entries are stored in
        //    paged storage (`HeapEntries`) where each page is never reallocated or moved.
        //  - The owning HeapObjectRead holds a reader count which guarantees the entry
        //    will never be freed by `dec_ref` or `collect_cycles` while the owner is
        //    borrowed through this HeapRead. The cycle collector's
        //    `Scan` phase treats `readers > 0` as an external reference and resurrects
        //    the entry to Black instead of condemning it as White.
        //  - The type of the `HeapValue` can never change once allocated. This is
        //    guaranteed by never exposing `&mut HeapData` outside of this module.
        //  - For protected external data and projections, borrowing this handle keeps the source
        //    borrow alive instead; their wrappers do not own a reader count.
        //  - The borrow on `HeapReader` guarantees that there are no mutable borrows on any heap
        //    data while the return value of this function is alive.
        unsafe { self.value.as_ref() }
    }

    /// Casts this reader to a field of type `U` at some `offset` within the struct.
    ///
    /// Borrows `self` to keep the source alive; the projection owns no reader count.
    ///
    /// # Safety
    ///   - The field of type `U` must ALWAYS exist at `offset` within `T` (i.e. `T` cannot be an enum, union etc)
    unsafe fn cast_as_member_ref<U>(&self, offset: usize) -> BorrowedHeapRead<'_, 'a, U> {
        BorrowedHeapRead {
            inner: HeapRead {
                // SAFETY: caller guarantees offset points to a valid field of type U within T
                value: unsafe { self.value.byte_add(offset) }.cast(),
                borrow: PhantomData,
            },
            original: PhantomData,
        }
    }
}

impl<'a, T: ?Sized> HeapReadMut<'a, T> {
    /// Exclusively borrows the payload, not the capability; aliases share the reader lock.
    pub fn get_mut<'r, H: RuntimeRef>(&'r self, _: &'r mut HeapReader<'a, H>) -> &'r mut T {
        let mut value = self.read.value;
        // SAFETY: the owner guarantees a writable, live pointee. The mutable reader
        // borrow excludes all other payload access until this reference expires.
        unsafe { value.as_mut() }
    }

    /// Casts this reader to a field of type `U` at some `offset` within the struct.
    ///
    /// Borrows the capability's owner; payload exclusivity still comes from the reader.
    ///
    /// # Safety
    ///   - The field of type `U` must ALWAYS exist at `offset` within `T` (i.e. `T` cannot be an enum, union etc)
    unsafe fn cast_as_member_ref_mut<U>(&self, offset: usize) -> BorrowedHeapReadMut<'_, 'a, U> {
        BorrowedHeapReadMut {
            inner: HeapReadMut {
                read: HeapRead {
                    // SAFETY: caller guarantees offset points to a valid field of type U within T
                    value: unsafe { self.read.value.byte_add(offset) }.cast(),
                    borrow: PhantomData,
                },
            },
            original: PhantomData,
        }
    }
}

/// Read-only payload access, including projections that must re-resolve their storage.
///
/// A vector or box may be replaced between accesses through another capability;
/// its projection must borrow the current allocation rather than cache a pointer.
pub(crate) trait HeapView<'a, T: ?Sized> {
    /// Borrows the current payload for the joint lifetime of the view and reader.
    fn get<'r, H: RuntimeRef>(&'r self, reader: &'r HeapReader<'a, H>) -> &'r T;
}

impl<'a, T: ?Sized> HeapView<'a, T> for HeapRead<'a, T> {
    fn get<'r, H: RuntimeRef>(&'r self, reader: &'r HeapReader<'a, H>) -> &'r T {
        self.get(reader)
    }
}

impl<'a, T> HeapRead<'a, Vec<T>> {
    /// Projects the current vector buffer without retaining its allocation address.
    pub fn as_slice(&self) -> HeapSliceRead<'_, 'a, T> {
        HeapSliceRead { source: self }
    }
}

/// A vector projection that observes buffer replacement between reader borrows.
pub struct HeapSliceRead<'source, 'a, T> {
    source: &'source HeapRead<'a, Vec<T>>,
}

impl<'a, T> HeapView<'a, [T]> for HeapSliceRead<'_, 'a, T> {
    fn get<'r, H: RuntimeRef>(&'r self, reader: &'r HeapReader<'a, H>) -> &'r [T] {
        self.source.get(reader).as_slice()
    }
}

impl<'a, T: ?Sized> HeapRead<'a, Box<T>> {
    /// Projects the current box allocation without retaining its address.
    pub fn as_box_value(&self) -> HeapBoxRead<'_, 'a, T> {
        HeapBoxRead { source: self }
    }
}

/// A box projection that observes replacement between reader borrows.
pub struct HeapBoxRead<'source, 'a, T: ?Sized> {
    source: &'source HeapRead<'a, Box<T>>,
}

impl<'a, T: ?Sized> HeapView<'a, T> for HeapBoxRead<'_, 'a, T> {
    fn get<'r, H: RuntimeRef>(&'r self, reader: &'r HeapReader<'a, H>) -> &'r T {
        self.source.get(reader).as_ref()
    }
}

/// Shared capability whose source borrow stays alive through every payload access.
pub struct BorrowedHeapRead<'original, 'a, U: ?Sized> {
    inner: HeapRead<'a, U>,
    original: PhantomData<&'original U>,
}

impl<'a, U: ?Sized> Deref for BorrowedHeapRead<'_, 'a, U> {
    type Target = HeapRead<'a, U>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Helper for `heap_read_ref_as_field`; callers must uphold `HeapRead::cast_as_member_ref`'s contract.
pub(crate) unsafe fn cast_as_member_ref_type_hinted<'r, 'a, T, U>(
    heap_read: &'r HeapRead<'a, T>,
    offset: usize,
    _type_hint: impl for<'s> Fn(&'s T) -> *const U,
) -> BorrowedHeapRead<'r, 'a, U> {
    // SAFETY: (DH) - caller upholds `cast_as_member` contract
    unsafe { heap_read.cast_as_member_ref(offset) }
}

macro_rules! heap_read_ref_as_field {
    ($heap_read:ident, $ty:ty, $field:tt) => {{
        let offset = std::mem::offset_of!($ty, $field);
        let type_hint = |value: &$ty| &raw const value.$field;
        // SAFETY: (DH)
        //  - `std::mem::offset_of!` guarantees there is a field at fixed offset
        //  - `type_hint` guarantees that the field is of type `U` for the safety contract
        #[expect(unsafe_code)]
        unsafe {
            $crate::heap::cast_as_member_ref_type_hinted($heap_read, offset, type_hint)
        }
    }};
}

pub(crate) use heap_read_ref_as_field;

/// Mutable capability backed by a source borrow, never exposing its inner handle mutably.
pub struct BorrowedHeapReadMut<'original, 'a, U: ?Sized> {
    inner: HeapReadMut<'a, U>,
    original: PhantomData<&'original mut U>,
}

impl<'a, U: ?Sized> Deref for BorrowedHeapReadMut<'_, 'a, U> {
    type Target = HeapReadMut<'a, U>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Helper for `heap_read_ref_as_field_mut`; callers must uphold `HeapReadMut::cast_as_member_ref_mut`'s contract.
pub(crate) unsafe fn cast_as_member_ref_mut_type_hinted<'r, 'a, T, U>(
    heap_read: &'r HeapReadMut<'a, T>,
    offset: usize,
    _type_hint: impl for<'s> Fn(&'s T) -> *const U,
) -> BorrowedHeapReadMut<'r, 'a, U> {
    // SAFETY: (DH) - caller upholds `cast_as_member` contract
    unsafe { heap_read.cast_as_member_ref_mut(offset) }
}

macro_rules! heap_read_ref_as_field_mut {
    ($heap_read:ident, $ty:ty, $field:tt) => {{
        let offset = std::mem::offset_of!($ty, $field);
        let type_hint = |value: &$ty| &raw const value.$field;
        // SAFETY: (DH)
        //  - `std::mem::offset_of!` guarantees there is a field at fixed offset
        //  - `type_hint` guarantees that the field is of type `U` for the safety contract
        #[expect(unsafe_code)]
        unsafe {
            $crate::heap::cast_as_member_ref_mut_type_hinted($heap_read, offset, type_hint)
        }
    }};
}

pub(crate) use heap_read_ref_as_field_mut;

/// Scoped pointer to a permanent slot's interior-mutable live/freed state.
///
/// After the backend associates a persistent handle with its arena, the invariant
/// brand prevents mixing reader scopes. GC worklists avoid repeated ownership checks;
/// destruction and reuse change the slot's contents, never the slot itself.
#[derive(Copy, Clone)]
#[repr(transparent)]
pub(crate) struct HeapPtr<'a> {
    inner: NonNull<()>,
    /// Makes `'a` invariant. Matches the brand on [`HeapReader<'a>`] / [`HeapRead<'a, T>`]
    /// so a `HeapPtr` cannot be reborrowed under a different reader scope.
    brand: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<'a> HeapPtr<'a> {
    /// Returns the live [`HeapEntry`] this pointer refers to, panicking if the slot
    /// has been freed.
    ///
    /// Mutable `HeapEntry` fields use interior mutability — bookkeeping via `Cell`
    /// and `data` via `UnsafeCell` — so callers can mutate them through
    /// the returned `&HeapEntry` without ever needing `&mut HeapEntry`. That's
    /// what makes a `&self`-derived `HeapPtr` (with Shared provenance) sound to
    /// dereference: we never derive `&mut` from it, so the SB/TB rules permit
    /// interior mutation via the embedded `Cell`/`UnsafeCell`.
    ///
    /// Use [`Self::try_entry`] for code paths that may legitimately encounter
    /// freed slots (e.g. linear scans over `0..heap.entries.len()`).
    pub fn entry<'r, H: RuntimeRef>(self, reader: &'r HeapReader<'a, H>) -> &'r HeapEntry<H> {
        self.try_entry(reader).expect("HeapPtr::entry: slot has been freed")
    }

    /// Returns the [`HeapEntry`] this pointer refers to, or `None` if the slot is
    /// currently freed.
    ///
    /// Use where a freed slot is part of the expected state (linear scans, root
    /// reseeds, etc.). Mutation paths (cycle collector mark/scan inner loops) should
    /// prefer [`Self::entry`] so that an unexpectedly-freed entry surfaces as a
    /// loud panic rather than a silent skip.
    pub(crate) fn try_entry<'r, H: RuntimeRef>(self, _reader: &'r HeapReader<'a, H>) -> Option<&'r HeapEntry<H>> {
        // SAFETY:
        //  - The invariant `'a` on `_reader` matches this pointer's brand, which is
        //    only settable inside `HeapReader::with`. That guarantees same-heap
        //    origin: a `HeapPtr<'a>` from a different reader scope cannot satisfy
        //    this signature.
        //  - `HeapEntries::pointer` only returns pointers to initialized slots, so
        //    the backend's live/freed representation is always initialized.
        //  - The `&HeapReader` borrow excludes any `&mut HeapReader` op that could
        //    free the slot during the returned reference's lifetime.
        let slot = unsafe { self.inner.cast::<HeapEntry<H>>().as_ref() };
        slot.live.get().then_some(slot)
    }

    /// Returns the entry's [`HeapData`] payload via the `UnsafeHeapData` interior-
    /// mutability boundary.
    ///
    /// `UnsafeCell::get` produces a `*mut HeapData` with `SharedReadWrite` permission,
    /// so the returned `&HeapData` is sound even though the underlying `HeapPtr` was
    /// minted via the slot's `UnsafeCell`. Mutation of the payload still requires
    /// `&mut HeapReader` via the [`HeapReadMut`] machinery.
    pub fn data<'r, H: RuntimeRef>(self, reader: &'r HeapReader<'a, H>) -> &'r HeapData<H> {
        let entry = self.entry(reader);
        // SAFETY: `UnsafeCell::get` yields a `SharedReadWrite` pointer; the `&HeapReader`
        // borrow on `reader` excludes any concurrent `&mut HeapReader` operation that
        // could mutate the payload during the returned reference's lifetime.
        unsafe { &*entry.data.0.get() }
    }

    /// Returns the typed [`HeapReadOutput`] for this entry, incrementing the
    /// reader count owned by the wrapped [`HeapObjectRead<T>`] until its `Drop`.
    ///
    /// The guard's capability pointers are derived through the `UnsafeHeapData`
    /// `UnsafeCell`, retaining `SharedReadWrite` permission for both read access
    /// and mutation via [`HeapReadMut::get_mut`], which requires `&mut HeapReader`.
    fn read<H: RuntimeRef>(self, id: H, reader: &HeapReader<'a, H>) -> HeapReadOutput<'a, H> {
        /// Computes a `HeapObjectRead` from the raw `UnsafeCell` pointer and a shared reference
        /// to the variant field. The `&T` is only used to compute the field's byte offset
        /// within the `HeapData` enum; the returned `NonNull` is derived from the original
        /// `*mut HeapData` pointer so it inherits the `SharedReadWrite` permission from
        /// the `UnsafeCell`, allowing both reads and writes.
        #[inline]
        fn heap_read<'a, H: RuntimeRef, T>(
            id: H,
            base: *mut HeapData<H>,
            field: &T,
            readers: NonNull<Cell<usize>>,
        ) -> HeapObjectRead<'a, H, T> {
            let base_addr = base as usize;
            let field_addr = ptr::from_ref(field) as usize;
            let offset = field_addr - base_addr;
            HeapObjectRead {
                id,
                readers,
                read: HeapReadMut {
                    read: HeapRead {
                        // SAFETY: The pointer is derived from the UnsafeCell's `*mut` via byte
                        // offset, preserving the `SharedReadWrite` permission. No reference retag
                        // occurs — we only use the `&T` for its address, not to derive the pointer.
                        value: unsafe { NonNull::new_unchecked(base.byte_add(offset).cast::<T>()) },
                        borrow: PhantomData,
                    },
                },
            }
        }

        /// Like `heap_read` but for `Box<T>` fields inside `HeapData` variants.
        ///
        /// The `&Box<T>` is only used to locate the box field inside the enum; the
        /// inner pointer is *loaded out of* that field rather than derived by
        /// dereferencing the box. Dereferencing (`boxed.as_ref()`) would create a
        /// shared `&T` whose `SharedReadOnly` provenance makes later
        /// `HeapReadMut::get_mut` writes UB; the loaded pointer instead carries the
        /// box's own stored (writeable) provenance.
        #[expect(
            clippy::borrowed_box,
            reason = "We intentionally take &Box<T> to signal this is for boxed HeapData variants; &T would lose that context"
        )]
        fn heap_read_boxed<'a, H: RuntimeRef, T>(
            id: H,
            base: *mut HeapData<H>,
            boxed: &Box<T>,
            readers: NonNull<Cell<usize>>,
        ) -> HeapObjectRead<'a, H, T> {
            let base_addr = base as usize;
            let field_addr = ptr::from_ref(boxed) as usize;
            let offset = field_addr - base_addr;
            // SAFETY: `offset` locates the live `Box<T>` field within the enum, and the
            // pointer to it is derived from the `UnsafeCell`'s `*mut` (not from `&Box<T>`),
            // preserving `SharedReadWrite` permission for the load below. `Box<T>` with
            // `T: Sized` is guaranteed to be represented as a single non-null pointer, so
            // loading the field as `*mut T` yields the box's data pointer together with
            // the read/write provenance it was stored with.
            let value = unsafe { NonNull::new_unchecked(base.byte_add(offset).cast::<*mut T>().read()) };
            HeapObjectRead {
                id,
                readers,
                read: HeapReadMut {
                    read: HeapRead {
                        value,
                        borrow: PhantomData,
                    },
                },
            }
        }

        macro_rules! heap_read_value {
            (inline $id:expr, $base:expr, $value:expr, $readers:expr) => {
                heap_read($id, $base, $value, $readers)
            };
            (boxed $id:expr, $base:expr, $value:expr, $readers:expr) => {
                heap_read_boxed($id, $base, $value, $readers)
            };
            (boxed32 $id:expr, $base:expr, $value:expr, $readers:expr) => {{
                #[cfg(target_pointer_width = "32")]
                {
                    heap_read_boxed($id, $base, $value, $readers)
                }
                #[cfg(target_pointer_width = "64")]
                {
                    heap_read($id, $base, $value, $readers)
                }
            }};
        }

        let entry = self.entry(reader);
        // Increment the reader count for this entry. The corresponding decrement
        // happens in `HeapObjectRead::drop`.
        entry.readers.set(entry.readers.get() + 1);
        let readers = NonNull::from(&entry.readers);
        // Get the raw pointer from the UnsafeCell — this has SharedReadWrite permission.
        let base: *mut HeapData<H> = entry.data.0.get();

        macro_rules! read_heap_payload {
            ($h:ident; $(
                $(#[$meta:meta])*
                $variant:ident($storage:ident $payload:ty)
            ),* $(,)?) => {
                // SAFETY: `base` points to this live entry's `UnsafeHeapData`.
                match unsafe { &*base } {
                    $(
                        HeapData::$variant(value) => HeapReadOutput::$variant(
                            heap_read_value!($storage id, base, value, readers)
                        ),
                    )*
                }
            };
        }

        // SAFETY: Match on a shared reference (`&*base`) to read the discriminant without
        // creating a Unique retag. The shared retag is compatible with existing
        // SharedReadWrite permissions from prior `read()` calls into the same UnsafeCell.
        // The `heap_read` helper then derives the NonNull from `base` (not from `&T`),
        // so the returned pointer retains full SharedReadWrite permission.
        crate::heap_data::heap_payloads!(read_heap_payload, H)
    }
}

/// A single entry inside the heap arena, storing refcount and payload.
///
/// Hashing state lives on the per-type structs that benefit from it
/// ([`Str`], [`Bytes`], [`Tuple`], [`NamedTuple`], [`FrozenSet`] (via
/// `SetStorage`), [`Path`]). Cheap-to-hash types ([`Range`], [`Slice`],
/// dates etc.) recompute on demand. Unhashable types ([`List`], [`Dict`],
/// [`Set`]) return `None` from `py_hash` directly. None of these need
/// per-entry metadata.
///
/// The `color` field encodes the entry's state for the trial-deletion cycle
/// collector (see [`CcColor`]). Outside of a running collection, every live
/// entry is either Black (uninteresting) or Purple (a cycle-root candidate
/// queued for investigation); Gray and White are transient states only seen
/// during [`Heap::collect_cycles`]. Cell-typed for symmetry with `refcount`
/// — every write goes through an `&mut Heap` path (`dec_ref` and the
/// collector's `mark_gray`/`scan`/`scan_black`).
#[derive(Debug)]
#[repr(C)]
pub struct HeapEntry<H: Reference> {
    /// Permanent runtime identity, reconstructed from slot position.
    index: usize,
    refcount: Cell<usize>,
    /// Number of active `HeapRead` pointers into this entry's data.
    ///
    /// Incremented when `HeapReader::read` creates a `HeapRead`, decremented when
    /// the `HeapRead` is dropped. `dec_ref` panics if it would free an entry that
    /// still has active readers — this guarantees that `HeapRead` pointers remain
    /// valid for as long as they exist.
    readers: Cell<usize>,
    /// Keep the hot live-state and GC tags beside the payload, not after it.
    color: Cell<CcColor>,
    /// Payload liveness is independent of the permanent slot's lifetime.
    live: Cell<bool>,
    /// The payload data
    data: UnsafeHeapData<H>,
}

impl<H: Reference + serde::Serialize> serde::Serialize for HeapEntry<H> {
    /// Preserves dump field order independently of the physical layout.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HeapEntry", 3)?;
        state.serialize_field("R", &self.refcount)?;
        state.serialize_field("D", &self.data)?;
        state.serialize_field("C", &self.color)?;
        state.end()
    }
}

/// What one heap entry costs, payload and bookkeeping together.
///
/// For preflighting a burst of allocations whose count is caller-controlled:
/// charging only the payloads would under-count by the refcount, reader count
/// and collector colour every entry also carries.
pub(crate) const HEAP_ENTRY_SIZE: usize = size_of::<HeapEntry<HeapId<'static>>>();

/// This wrapper containing `UnsafeCell` exists to allow for data inside of `HeapValue`
/// to be safely pointed to via the `HeapReader` API.
///
/// The safety invariants are protected by the `Heap` / `HeapReader` API:
///   - It is never possible to alias mutable and immutable borrows into heap values,
///     whether they are the same or different value.
///   - When a mutable borrow of a heap value exists, no other heap value may be
///     borrowed. (See `Heap::get_mut` and `HeapRead::get`, which both require a `&mut`
///     borrow on the heap.)
struct UnsafeHeapData<H: Reference>(UnsafeCell<HeapData<H>>);

impl<H: Reference> fmt::Debug for UnsafeHeapData<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: (DH) Debug formatting is read-only and never called concurrently
        // with mutation. This matches the safety invariants of the HeapReader API.
        let data = unsafe { &*self.0.get() };
        f.debug_tuple("UnsafeHeapData").field(data).finish()
    }
}

impl<H: Reference + serde::Serialize> serde::Serialize for UnsafeHeapData<H> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // SAFETY: when heap data is being serialized, there is no mutable borrow
        // possible on any data contents
        HeapData::serialize(unsafe { &*self.0.get() }, serializer)
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
/// Owns the [`ResourceTracker`] that enforces memory/time limits and schedules
/// GC; unconfigured limits reduce every check to a single predictable branch.
#[derive(Debug)]
pub(crate) struct Heap<H: RuntimeRef> {
    /// Permanent pointer-addressed slots belonging to this heap's invariant arena brand.
    entries: HeapEntries<H>,
    /// Resource tracker for enforcing limits and scheduling GC.
    pub tracker: ResourceTracker,
    /// Number of entries currently flagged [`Purple`](CcColor::Purple) — i.e.,
    /// suspected cycle roots awaiting collection.
    ///
    /// Used as an early-out: when zero, `collect_cycles` has no candidates
    /// and skips its heap walk entirely. The actual GC *frequency* is still
    /// driven by `allocations_since_gc` against the configured interval, so
    /// programs that produce no cycle candidates pay no collector cost
    /// regardless of how many allocations they perform.
    ///
    /// All `dec_ref` paths that mutate this counter take `&mut self`, so a
    /// plain `usize` is sufficient (no interior mutability needed).
    purple_count: usize,
    /// Number of GC-applicable allocations since the last cycle collection.
    ///
    /// Incremented for every GC-tracked allocation (see [`HeapData::is_gc_tracked`])
    /// and reset to zero at the end of every successful [`collect_cycles`]
    /// call. Combined with [`purple_count`](Self::purple_count) to gate
    /// automatic collections in [`should_gc`](Self::should_gc).
    ///
    /// Uses `Cell` for interior mutability so that `allocate(&self)` can
    /// increment.
    allocations_since_gc: Cell<u32>,
    /// When true, [`should_gc`](Self::should_gc) returns false regardless of
    /// the candidate count, suppressing automatic cycle-collection passes.
    /// Toggled by the `gc.disable()` / `gc.enable()` Python helpers (only
    /// registered under the `test-hooks` feature). Explicit `gc.collect()`
    /// calls still run.
    #[cfg(feature = "test-hooks")]
    gc_disabled: bool,
    /// Cached HeapId for the `datetime.timezone.utc` singleton.
    ///
    /// Lazily allocated on first access to `timezone.utc`. Once created, the refcount
    /// is incremented on each access so the caller can drop their reference normally.
    timezone_utc: Option<H>,
    /// Live external functions indexed by name without owning heap references.
    ///
    /// Uses `BTreeMap` to avoid large residual capacity from spikes of `ExtFunction` allocations.
    ext_function_cache: BTreeMap<Arc<str>, H>,
    /// Live sandbox classes and instances by boundary uuid, so a value the host
    /// hands back resolves to the original object. Weak like
    /// `ext_function_cache`: cleared when the object is freed, rebuilt on restore.
    boundary_index: BTreeMap<MontyUuid, H>,
    /// Live host class type objects by class uuid — one `HostClassType` entry
    /// per host class, shared by its instances and `type(x)`. Separate from
    /// `boundary_index` because host and sandbox uuids are distinct namespaces.
    /// Weak like the others: cleared on free, rebuilt on restore.
    host_type_index: BTreeMap<MontyUuid, H>,
}

impl<H: RuntimeRef> serde::Serialize for Heap<H> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Heap", 5)?;
        state.serialize_field("entries", &self.entries)?;
        state.serialize_field("tracker", &self.tracker)?;
        state.serialize_field("purple_count", &self.purple_count)?;
        state.serialize_field("allocations_since_gc", &self.allocations_since_gc.get())?;
        state.serialize_field("timezone_utc", &self.timezone_utc)?;
        state.end()
    }
}

/// Pointer-free heap input; only the sealed owner may relocate it into live storage.
#[derive(serde::Deserialize)]
#[serde(rename = "Heap")]
pub(crate) struct HeapWire {
    entries: Vec<Option<HeapEntryWire>>,
    tracker: ResourceTracker,
    purple_count: usize,
    allocations_since_gc: u32,
    timezone_utc: Option<HeapIndex>,
}

/// Wire entries omit runtime metadata and retain the persisted field order for sequence codecs.
#[derive(Deserialize)]
#[serde(rename = "HeapEntry")]
struct HeapEntryWire {
    #[serde(rename = "R")]
    refcount: usize,
    #[serde(rename = "D")]
    data: HeapData<HeapIndex>,
    #[serde(rename = "C")]
    color: CcColor,
}

impl HeapEntryWire {
    /// Creates payload bookkeeping only after the destination owner's slots exist.
    fn relocate<H: RuntimeRef>(self, reference: &mut impl FnMut(HeapIndex) -> H) -> HeapEntry<H> {
        HeapEntry {
            index: 0,
            refcount: Cell::new(self.refcount),
            readers: Cell::new(0),
            color: Cell::new(self.color),
            live: Cell::new(false),
            data: UnsafeHeapData(UnsafeCell::new(self.data.relocate(reference))),
        }
    }
}

/// Rebuilds weak indexes after the owner has validated GC colors and reconstructed its slots.
fn restore_entries<H: RuntimeRef>(entries: &mut HeapEntries<H>) -> WeakIndexes<H> {
    let mut indexes = WeakIndexes::default();
    for index in 0..entries.len() {
        let id = entries.id_at(index);
        let Some(mut entry) = entries.entry(id) else {
            continue;
        };
        match entry.get_mut().data.0.get_mut() {
            HeapData::ExtFunction(function) => {
                indexes.ext_function_cache.insert(function.cache_key(), id);
            }
            HeapData::Instance(instance) => {
                indexes.index_boundary_uuid(instance.uuid(), id);
            }
            HeapData::Class(class) => {
                indexes.index_boundary_uuid(class.uuid(), id);
            }
            HeapData::HostClassType(ty) => {
                indexes.host_type_index.insert(ty.type_id(), id);
            }
            _ => {}
        }
    }
    indexes
}

/// A freed object's key in one of the heap's weak indexes.
enum WeakIndexKey {
    ExtFunction(Arc<str>),
    Boundary(MontyUuid),
    HostType(MontyUuid),
}

/// The heap's weak (non-owning) indexes, rebuilt from the entries on restore.
struct WeakIndexes<H: Reference> {
    ext_function_cache: BTreeMap<Arc<str>, H>,
    boundary_index: BTreeMap<MontyUuid, H>,
    host_type_index: BTreeMap<MontyUuid, H>,
}

impl<H: Reference> Default for WeakIndexes<H> {
    fn default() -> Self {
        Self {
            ext_function_cache: BTreeMap::new(),
            boundary_index: BTreeMap::new(),
            host_type_index: BTreeMap::new(),
        }
    }
}

impl<H: RuntimeRef> WeakIndexes<H> {
    /// Indexes `id` under `uuid` when the object has one (it has crossed to the host).
    fn index_boundary_uuid(&mut self, uuid: Option<MontyUuid>, id: H) {
        if let Some(uuid) = uuid {
            self.boundary_index.insert(uuid, id);
        }
    }
}

/// GC interval when the resource tracker sets none: [`DEFAULT_GC_INTERVAL`].
///
/// The trial-deletion collector additionally short-circuits the trace when
/// `purple_count == 0`, so programs that produce no cycle candidates pay no
/// collector cost regardless of their allocation rate.
///
/// When the `memory-model-checks` feature is enabled, this is reduced to 1 to
/// stress-test GC behavior on every allocation.
const GC_INTERVAL: usize = if cfg!(feature = "memory-model-checks") {
    1
} else {
    DEFAULT_GC_INTERVAL
};

#[cfg(test)]
impl Heap<HeapId<'static>> {
    /// Runs existing heap unit tests under the same generative owner boundary as execution.
    pub(crate) fn with_test<R>(
        capacity: usize,
        tracker: ResourceTracker,
        test: impl for<'arena> FnOnce(&mut Heap<HeapId<'arena>>) -> R,
    ) -> R {
        HeapOwner::new(capacity, tracker).activate(|state| test(&mut state.heap))
    }
}

impl<H: RuntimeRef> Heap<H> {
    /// Creates storage for the sealed owner; never an independent arena of an existing brand.
    ///
    /// # Safety
    /// The owner must seal the heap with all roots and expose only fresh invariant activation brands.
    /// The erased construction brand must never escape that owner.
    unsafe fn new(capacity: usize, tracker: ResourceTracker) -> Self {
        let this = Self {
            // SAFETY: the sealed owner establishes a fresh brand for this entire arena.
            entries: unsafe { HeapEntries::with_capacity(capacity) },
            tracker,
            purple_count: 0,
            allocations_since_gc: Cell::new(0),
            #[cfg(feature = "test-hooks")]
            gc_disabled: false,
            timezone_utc: None,
            ext_function_cache: BTreeMap::new(),
            boundary_index: BTreeMap::new(),
            host_type_index: BTreeMap::new(),
        };

        // The empty-tuple singleton starts with refcount = 1 — that single ref *is* the
        // permanent heap-owned reference. `get_empty_tuple` bumps the refcount on each
        // hand-out so callers can `dec_ref` normally; the heap-owned ref keeps the
        // singleton's rc ≥ 1 forever, which is why trial deletion needs no special-case
        // rooting for it (a debug_assert in `dec_ref` enforces the invariant).
        let empty_tuple = Tuple::default().into_heap_data();
        let new_entry = HeapEntry {
            index: 0,
            live: Cell::new(true),
            refcount: Cell::new(1),
            readers: Cell::new(0),
            data: UnsafeHeapData(UnsafeCell::new(empty_tuple)),
            color: Cell::new(CcColor::Black),
        };

        let empty_tuple = this.entries.allocate(new_entry);
        debug_assert_eq!(empty_tuple.index(), EMPTY_TUPLE_INDEX);
        this
    }

    /// Number of entries in the heap (including freed slots).
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    /// Returns the number of GC-tracked allocations since the last cycle
    /// collection. Reset to zero by [`collect_cycles`](Self::collect_cycles).
    ///
    /// Used by `run_ref_counts` to expose the GC trigger metric to tests:
    /// a value much smaller than the configured `gc_interval` after many
    /// allocations is evidence that collection ran.
    #[cfg(feature = "ref-count-return")]
    pub fn get_allocations_since_gc(&self) -> u32 {
        self.allocations_since_gc.get()
    }

    /// Allocates a new heap entry.
    ///
    /// GC-tracked types bump `allocations_since_gc` so that
    /// [`should_gc`](Self::should_gc) eventually fires; trial deletion's own
    /// candidate enrollment happens later, at `dec_ref` time. Leaf types
    /// (strings, bytes, …) cannot participate in cycles and don't count
    /// against the GC interval.
    pub fn allocate(&self, data: HeapData<H>) -> H {
        self.entries.allocate(self.new_entry(data))
    }

    /// Builds the initialized entry shared by typed and untyped allocation.
    fn new_entry(&self, data: HeapData<H>) -> HeapEntry<H> {
        if data.is_gc_tracked() {
            self.allocations_since_gc
                .set(self.allocations_since_gc.get().wrapping_add(1));
        }

        HeapEntry {
            index: 0,
            live: Cell::new(true),
            refcount: Cell::new(1),
            readers: Cell::new(0),
            data: UnsafeHeapData(UnsafeCell::new(data)),
            color: Cell::new(CcColor::Black),
        }
    }

    /// Returns the singleton empty tuple.
    ///
    /// In Python, `() is ()` is always `True` because empty tuples are interned.
    /// This method provides the same optimization by returning the same `HeapId`
    /// for all empty tuple allocations.
    ///
    /// The returned `Value` has its reference count incremented, so the caller
    /// owns a reference and must call `dec_ref` when done.
    pub fn get_empty_tuple(&self) -> Value<H> {
        let id = self.entries.id_at(EMPTY_TUPLE_INDEX);
        self.inc_ref(id);
        Value::Ref(id)
    }

    /// Returns the external function for `name`, reusing a live object when possible.
    ///
    /// The cache holds no reference count of its own. Its entry is removed when the
    /// last owning reference is dropped, before the heap slot can be reused.
    pub fn get_ext_function(&mut self, name: &str) -> Value<H> {
        if let Some(id) = self.ext_function_cache.get(name).copied() {
            self.inc_ref(id);
            Value::Ref(id)
        } else {
            let function = ExtFunction::new(name);
            let cache_key = function.cache_key();
            let id = self.allocate(HeapData::ExtFunction(function));
            let previous = self.ext_function_cache.insert(cache_key, id);
            debug_assert!(previous.is_none());
            Value::Ref(id)
        }
    }

    /// Removes `id` from the weak cache without disturbing a duplicate-name winner.
    fn remove_ext_function_cache_entry(cache: &mut BTreeMap<Arc<str>, H>, name: &str, id: H) {
        if cache.get(name) == Some(&id) {
            cache.remove(name);
        }
    }

    /// The key `data` holds in a weak index, read before its slot is freed so
    /// the index entry can be cleared (see [`Heap::remove_weak_index_entry`]).
    fn weak_index_key(data: &HeapData<H>) -> Option<WeakIndexKey> {
        match data {
            HeapData::ExtFunction(function) => Some(WeakIndexKey::ExtFunction(function.cache_key())),
            HeapData::Instance(instance) => instance.uuid().map(WeakIndexKey::Boundary),
            HeapData::Class(class) => class.uuid().map(WeakIndexKey::Boundary),
            HeapData::HostClassType(ty) => Some(WeakIndexKey::HostType(ty.type_id())),
            _ => None,
        }
    }

    /// Clears the weak index entry for the object at `id` before its slot can
    /// be reused, only if the entry still points at this exact object (a
    /// restored dump may hold duplicates).
    fn remove_weak_index_entry(&mut self, key: Option<WeakIndexKey>, id: H) {
        match key {
            Some(WeakIndexKey::ExtFunction(name)) => {
                Self::remove_ext_function_cache_entry(&mut self.ext_function_cache, &name, id);
            }
            Some(WeakIndexKey::Boundary(uuid)) if self.boundary_index.get(&uuid) == Some(&id) => {
                self.boundary_index.remove(&uuid);
            }
            Some(WeakIndexKey::HostType(uuid)) if self.host_type_index.get(&uuid) == Some(&id) => {
                self.host_type_index.remove(&uuid);
            }
            Some(WeakIndexKey::Boundary(_) | WeakIndexKey::HostType(_)) | None => {}
        }
    }

    /// Boundary uuid of the sandbox class or instance at `id`, generated and
    /// indexed on its first crossing to the host so the host can hand the
    /// object back by id (see [`Heap::resolve_boundary_uuid`]).
    ///
    /// # Panics
    /// If `id` is not a live `Instance` or `Class` entry.
    pub(crate) fn boundary_uuid(&mut self, id: H) -> MontyUuid {
        let mut entry = self
            .entries
            .entry(id)
            .expect("Heap::boundary_uuid: entry already freed");
        let uuid = match entry.get_mut().data.0.get_mut() {
            HeapData::Instance(instance) => instance.boundary_uuid(),
            HeapData::Class(class) => class.boundary_uuid(),
            _ => unreachable!("Heap::boundary_uuid: only classes and instances carry a boundary uuid"),
        };
        self.boundary_index.insert(uuid, id);
        uuid
    }

    /// The live sandbox class or instance that crossed to the host as `uuid`,
    /// if it still exists; the index holds no reference, so the returned id is
    /// borrowed.
    #[must_use]
    pub(crate) fn resolve_boundary_uuid(&self, uuid: &MontyUuid) -> Option<H> {
        self.boundary_index.get(uuid).copied()
    }

    /// The live type object for the host class `uuid`, if the sandbox holds
    /// one; the index owns no reference, so the returned id is borrowed.
    #[must_use]
    pub(crate) fn resolve_host_type(&self, uuid: &MontyUuid) -> Option<H> {
        self.host_type_index.get(uuid).copied()
    }

    /// Allocates and indexes the type object for a host class the sandbox has
    /// not seen yet; callers must check [`Heap::resolve_host_type`] first so
    /// the one-entry-per-class invariant holds.
    pub(crate) fn allocate_host_type(&mut self, ty: HostClassType<H>) -> H {
        let uuid = ty.type_id();
        let id = self.allocate(HeapData::HostClassType(Box::new(ty)));
        let previous = self.host_type_index.insert(uuid, id);
        debug_assert!(
            previous.is_none(),
            "Heap::allocate_host_type: class {uuid} already has a type object"
        );
        id
    }

    /// Returns the cached `datetime.timezone.utc` singleton, lazily creating it on first access.
    ///
    /// The returned `Value::Ref` has its refcount incremented so the caller can drop
    /// it normally. The singleton itself is kept alive by the `timezone_utc` field.
    pub fn get_timezone_utc(&mut self) -> Value<H> {
        if let Some(id) = self.timezone_utc {
            self.inc_ref(id);
            Value::Ref(id)
        } else {
            let tz = TimeZone::utc();
            let id = self.allocate(HeapData::TimeZone(tz));
            // Keep an extra refcount for the singleton cache
            self.inc_ref(id);
            self.timezone_utc = Some(id);
            Value::Ref(id)
        }
    }

    /// Increments the reference count for an existing heap entry.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn inc_ref(&self, id: H) {
        let value = self.entries.get(id);
        value.refcount.update(|r| r + 1);
    }

    /// Decrements the reference count and frees the value (plus children) once it hits zero.
    ///
    /// This is the low-level release operation for an owned raw `HeapId`. Ordinary
    /// control flow should instead keep local ownership in `Value::Ref` and use
    /// `defer_drop!` or `DropGuard`. Heap-stored owners declare child references in
    /// `HeapItem::py_dec_ref_ids`; direct calls are appropriate in cleanup for other
    /// structures that own raw IDs.
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
    pub fn dec_ref(&mut self, id: H) {
        HeapReader::with(self, &mut (), |reader, ()| {
            let mut current_id = id;
            // A fresh Vec is deliberate: it costs nothing unless children are
            // actually pushed, whereas pooling it on the Heap was measured
            // (CodSpeed, PR #536) to add take/restore traffic to every call.
            let mut work_stack = Vec::new();
            loop {
                // Using `HeapPtr` avoids the possibility of aliasing with live borrows
                // held by `HeapRead` handles.
                let ptr = reader.read_ptr(current_id);
                let heap_entry = ptr.entry(reader);
                if heap_entry.refcount.get() > 1 {
                    heap_entry.refcount.update(|r| r - 1);

                    let is_gc_tracked = ptr.data(reader).is_gc_tracked();
                    if is_gc_tracked && heap_entry.color.get() != CcColor::Purple {
                        // The refcount survived — a newly unreachable cycle could
                        // now be hiding. Flag it as a candidate for the next `collect_cycles`.
                        heap_entry.color.set(CcColor::Purple);
                        reader.heap.purple_count += 1;
                    }
                } else {
                    debug_assert!(
                        current_id.index() != EMPTY_TUPLE_INDEX,
                        "Heap::dec_ref: empty-tuple singleton's heap-owned refcount must never reach zero",
                    );
                    assert!(
                        heap_entry.readers.get() == 0,
                        "Heap::dec_ref: cannot free HeapId({}) with {} active reader(s)",
                        current_id.index(),
                        heap_entry.readers.get(),
                    );
                    // If the entry was a pending cycle candidate, decrement
                    // `purple_count` to reflect that it is leaving the heap before
                    // the collector reaches it.
                    if heap_entry.color.get() == CcColor::Purple {
                        reader.heap.purple_count -= 1;
                    }
                    // Remove weak-index entries before the slot becomes available for reuse.
                    let weak_key = Self::weak_index_key(ptr.data(reader));
                    reader.heap.remove_weak_index_entry(weak_key, current_id);

                    // Freeing also updates arena bookkeeping and requires exclusive access.
                    let mut value = reader.heap.entries.entry(current_id).expect("already looked up").free();

                    // Collect child IDs and push onto work stack for iterative processing
                    py_dec_ref_ids_for_data(value.data.0.get_mut(), &mut work_stack);
                }

                let Some(next_id) = work_stack.pop() else {
                    break;
                };
                current_id = next_id;
            }
        });
    }

    /// Returns an immutable reference to the heap data stored at the given ID. This can be more efficient
    /// than `.read()` for short-lived borrows that need read-only access (avoids reader bookkeeping).
    ///
    /// # Panics
    /// Panics if the value ID is invalid, the value has already been freed,
    /// or the data is currently borrowed via `with_entry_mut`/`call_attr`.
    #[must_use]
    pub fn get(&self, id: H) -> &HeapData<H> {
        let data = &self.entries.get(id).data;
        // SAFETY: (DH) no mutable references into `HeapData` is possible while the heap is borrowed
        unsafe { &*data.0.get() }
    }

    /// Returns the reference count for the heap entry at the given ID.
    ///
    /// This is primarily used for testing reference counting behavior.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    #[must_use]
    #[cfg(feature = "ref-count-return")]
    pub fn get_refcount(&self, id: H) -> usize {
        self.entries.get(id).refcount.get()
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

    /// Returns whether cycle collection should run.
    ///
    /// True when the configured allocation interval has elapsed *and* at
    /// least one [`Purple`](CcColor::Purple) candidate is pending. The
    /// alloc-count check sets the maximum collector frequency the user
    /// asked for; the `purple_count` check is an additional early-out so
    /// programs that produce no cycle candidates pay no collector cost
    /// regardless of their allocation rate.
    ///
    /// Always returns false when [`disable_gc`](Self::disable_gc) has been
    /// called without a matching [`enable_gc`](Self::enable_gc); explicit
    /// [`collect_cycles`](Self::collect_cycles) calls still run regardless.
    #[inline]
    pub fn should_gc(&self) -> bool {
        #[cfg(feature = "test-hooks")]
        if self.gc_disabled {
            return false;
        }
        if self.purple_count == 0 {
            return false;
        }
        let interval = self.tracker.gc_interval().unwrap_or(GC_INTERVAL);
        (self.allocations_since_gc.get() as usize) >= interval
    }

    /// Suppresses automatic garbage collection until [`enable_gc`](Self::enable_gc)
    /// is called.
    ///
    /// Explicit [`collect_cycles`](Self::collect_cycles) calls still run while
    /// disabled, so a script can build a known amount of garbage and then time
    /// exactly one collection pass.
    #[cfg(feature = "test-hooks")]
    pub fn disable_gc(&mut self) {
        self.gc_disabled = true;
    }

    /// Resumes automatic garbage collection after a prior [`disable_gc`](Self::disable_gc).
    ///
    /// Calling [`enable_gc`](Self::enable_gc) on an already-enabled heap is a no-op.
    #[cfg(feature = "test-hooks")]
    pub fn enable_gc(&mut self) {
        self.gc_disabled = false;
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
    /// Returns the number of unreachable entries that were freed during the sweep.
    ///
    /// # Caller Responsibility
    /// The caller should check [`should_gc`](Self::should_gc) before calling
    /// this method. With `purple_count == 0` the function returns immediately
    /// without touching the heap.
    pub fn collect_cycles(&mut self) -> usize {
        if self.purple_count == 0 {
            return 0;
        }
        // The mark/scan phases use `HeapPtr<'a>` to check arena ownership once
        // per worklist insertion; the brand `'a` comes from the
        // surrounding `HeapReader::with` scope.
        HeapReader::with(self, &mut (), |reader, ()| reader.collect_cycles_inner())
    }

    fn collect_white(&mut self, roots: Vec<H>) -> usize {
        let mut work_stack = roots;
        let mut freed = 0;
        while let Some(id) = work_stack.pop() {
            let Some(mut entry) = self.entries.entry(id) else {
                // Already freed via another seed's traversal — ignore.
                continue;
            };
            let heap_entry = entry.get_mut();
            if heap_entry.color.get() != CcColor::White {
                // Either resurrected to Black by `Scan` or never visited
                // (still Black/Gray from somewhere). Don't free.
                continue;
            }
            debug_assert!(
                heap_entry.readers.get() == 0,
                "collect_white: cannot free HeapId({}) with {} active reader(s)",
                id.index(),
                heap_entry.readers.get(),
            );
            let mut value = entry.free();
            // Clear weak entries before freeing their slots, just as `dec_ref`.
            let weak_key = Self::weak_index_key(value.data.0.get_mut());
            self.remove_weak_index_entry(weak_key, id);
            freed += 1;
            // Walk children, marking child `Value::Ref`s as `Dereferenced`
            // under `memory-model-checks` so dropping the freed entry's data
            // doesn't trip a Drop-panic on a live `Value::Ref` payload. The
            // pushed child IDs feed the work stack so we recursively walk
            // White grandchildren — we do *not* `dec_ref` these children
            // (`MarkGray`/`ScanBlack` already balanced their refcounts).
            py_dec_ref_ids_for_data(value.data.0.get_mut(), &mut work_stack);
        }
        freed
    }
}

/// Cycle-collection inner phases.
///
/// Implemented on [`HeapReader<'a>`] so the work stacks can carry
/// [`HeapPtr<'a>`] instead of [`HeapId`]: once an entry is reached, every
/// subsequent access is a pointer deref rather than a paged-storage lookup.
/// Entered only via [`Heap::collect_cycles`], which establishes the brand `'a`
/// through [`HeapReader::with`]. [`Heap::collect_white`] is intentionally kept
/// in `HeapId` space because freeing a slot requires consulting the free list,
/// which is keyed by id.
impl<'a, H: RuntimeRef> HeapReader<'a, H> {
    /// Implementation of [`Heap::collect_cycles`]. Phase 1 (discover Purple
    /// roots and `mark_gray` their subtrees), Phase 2 (`scan`), and Phase 3
    /// (`collect_white`) run sequentially. After `scan` the `HeapPtr` work
    /// stack is drained, so no branded pointer escapes into Phase 3.
    ///
    /// Each phase iterates `for_each_child_id` inline rather than staging child
    /// `HeapId`s through a scratch `Vec`. Mutation of entry fields goes through
    /// the `Cell`/`UnsafeCell`-backed interior mutability on `HeapEntry`, so the
    /// `&HeapEntry`/`&HeapData` borrows from [`HeapPtr::entry`]/[`HeapPtr::data`]
    /// can coexist with the `&self.read_ptr` calls in the closure body.
    fn collect_cycles_inner(&mut self) -> usize {
        let mut roots: Vec<H> = Vec::new();
        let mut work_stack: Vec<HeapPtr<'a>> = Vec::new();

        // 1. Discover roots by finding Purple entries. Mark each root (and its subtree) Gray.
        //    The linear scan may legitimately encounter freed slots, so we use
        //    `try_entry` to skip them rather than panicking.
        for i in 0..self.heap.entries.len() {
            let id = self.heap.entries.id_at(i);
            let ptr = self.read_ptr(id);
            let Some(entry) = ptr.try_entry(self) else {
                continue;
            };
            if entry.color.get() != CcColor::Purple {
                continue;
            }
            if entry.readers.get() > 0 {
                // This entry cannot possibly be a root since it has active readers; reset
                // to Black so it won't be a candidate in the next cycle.
                entry.color.set(CcColor::Black);
                continue;
            }
            roots.push(id);
            self.mark_gray(ptr, &mut work_stack);
            debug_assert!(work_stack.is_empty(), "mark_gray must drain its work stack");
        }

        // 2. For each root, scan and resurrect if alive (refcount > 0 or active readers).
        //    Roots were live at Phase 1 and nothing has freed them between then and
        //    now, so a freed slot would indicate a bug.
        work_stack.extend(roots.iter().map(|&id| self.read_ptr(id)));
        self.scan(&mut work_stack);
        debug_assert!(work_stack.is_empty(), "scan must drain its work stack");

        // 3. Collect each root's White children as unreachable garbage. This phase
        //    keeps working in HeapId-space because `entry.free()` pushes to the
        //    free list, which is keyed by id.
        let freed = self.heap.collect_white(roots);

        // After this pass no Purple entries remain in the heap; zero the
        // counter so the next `dec_ref` event re-seeds from a clean baseline,
        // and reset the alloc-count gate so the next interval starts now.
        self.heap.purple_count = 0;
        self.heap.allocations_since_gc.set(0);
        freed
    }

    /// `MarkGray` (iterative): paint `s` and its transitive children Gray,
    /// decrementing each child's refcount once per traversal edge.
    ///
    /// After this completes for every root, every Gray entry's refcount equals
    /// the count of *external* references into it (refs originating outside
    /// the candidate subgraph). `Scan` uses that property to decide
    /// alive/condemned.
    fn mark_gray(&mut self, mut ptr: HeapPtr<'a>, work_stack: &mut Vec<HeapPtr<'a>>) {
        ptr.entry(self).color.set(CcColor::Gray);
        loop {
            for_each_child_id(ptr.data(self), |child_id| {
                let child_ptr = self.read_ptr(child_id);
                let entry = child_ptr.entry(self);
                debug_assert!(entry.refcount.get() > 0, "mark_gray: refcount underflow");
                entry.refcount.update(|r| r - 1);
                if entry.color.replace(CcColor::Gray) == CcColor::Gray {
                    // Already marked via another edge, don't push again
                    return;
                }
                work_stack.push(child_ptr);
            });
            let Some(next_ptr) = work_stack.pop() else {
                break;
            };
            ptr = next_ptr;
        }
    }

    /// `Scan` (iterative): each Gray entry is either resurrected via
    /// `ScanBlack` (external reference exists — refcount > 0 or active
    /// `HeapRead` reader) or painted White and its Gray children recursed.
    fn scan(&mut self, work_stack: &mut Vec<HeapPtr<'a>>) {
        let mut black_work_stack: Vec<HeapPtr<'a>> = Vec::new();
        while let Some(ptr) = work_stack.pop() {
            let entry = ptr.entry(self);
            if entry.color.get() != CcColor::Gray {
                // Already processed via another edge
                continue;
            }
            if entry.refcount.get() == 0 && entry.readers.get() == 0 {
                entry.color.set(CcColor::White);
                for_each_child_id(ptr.data(self), |child_id| {
                    let child_ptr = self.read_ptr(child_id);
                    let entry = child_ptr.entry(self);
                    if entry.color.get() != CcColor::Gray {
                        // Already processed via another edge
                        return;
                    }
                    work_stack.push(child_ptr);
                });
            } else {
                // External reference exists (either a refcount we couldn't
                // account for inside the candidate set, or a live `HeapRead`
                // pointing into the entry). Resurrect this entry and its
                // transitive Gray children back to Black via `mark_black`.
                self.mark_black(ptr, &mut black_work_stack);
                debug_assert!(black_work_stack.is_empty());
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
    fn mark_black(&mut self, mut ptr: HeapPtr<'a>, work_stack: &mut Vec<HeapPtr<'a>>) {
        ptr.entry(self).color.set(CcColor::Black);
        loop {
            for_each_child_id(ptr.data(self), |child_id| {
                let child_ptr = self.read_ptr(child_id);
                let entry = child_ptr.entry(self);
                entry.refcount.update(|r| r + 1);
                if entry.color.replace(CcColor::Black) == CcColor::Black {
                    // Already marked via another edge
                    return;
                }
                work_stack.push(child_ptr);
            });
            let Some(next_ptr) = work_stack.pop() else {
                break;
            };
            ptr = next_ptr;
        }
    }
}

/// Leak detection for reference-counting tests.
///
/// Implemented on [`HeapReader`] because reading an entry's children needs
/// [`HeapPtr::data`]. It borrows the cycle collector's [`for_each_child_id`] to
/// walk edges but is not part of collection: nothing here mutates the heap, and
/// it is reached only from `run_ref_counts`, never from [`Heap::collect_cycles`].
#[cfg(feature = "ref-count-return")]
impl<H: RuntimeRef> HeapReader<'_, H> {
    /// Returns live heap entries unreachable from `roots`, with each entry's type
    /// so callers holding `Interns` can name it ([`Type::name`]).
    ///
    /// `run_ref_counts` uses this to prove a test leaked nothing: an entry that
    /// is alive but reachable from no named variable is a missed `drop_with`.
    /// The walk is transitive, so an object owned by another object — a class's
    /// `__annotations__`, a nested list, an instance attribute — is accounted
    /// for by its owner and need not be bound to a name by the test itself.
    ///
    /// Unlike the cycle collector's [`mark_gray`](Self::mark_gray) this touches
    /// no colors or refcounts, so it cannot perturb the state under test.
    pub(crate) fn unreachable_entries(&self, roots: impl IntoIterator<Item = H>) -> Vec<(H, Type<H>)> {
        let mut seen: HashSet<H> = HashSet::new();
        let mut work_stack: Vec<H> = Vec::new();
        for root in roots {
            if seen.insert(root) {
                work_stack.push(root);
            }
        }

        // A root's subtree is live by construction, but `try_entry` keeps the
        // walk total in case a test observes the heap mid-teardown.
        while let Some(id) = work_stack.pop() {
            let ptr = self.read_ptr(id);
            if ptr.try_entry(self).is_none() {
                continue;
            }
            for_each_child_id(ptr.data(self), |child_id| {
                if seen.insert(child_id) {
                    work_stack.push(child_id);
                }
            });
        }

        self.entries
            .iter()
            // The empty tuple singleton is an internal optimisation that is
            // always live and never named, so it is not a leak. See `entry_count`.
            .filter(|(id, _)| id.index() != EMPTY_TUPLE_INDEX && !seen.contains(id))
            .map(|(id, _)| (id, self.read_ptr(id).data(self).py_type()))
            .collect()
    }
}

// With `memory-model-checks` enabled, need to manually clean up the heap to avoid the
// bookkeeping causing panics at shutdown.
#[cfg(feature = "memory-model-checks")]
impl<H: RuntimeRef> Drop for Heap<H> {
    fn drop(&mut self) {
        for id in 0..self.entries.len() {
            if let Some(mut entry) = self.entries.entry(self.entries.id_at(id)) {
                // Mark all `Value::Ref` payloads as `Dereferenced` so they don't panic when dropped
                py_dec_ref_ids_for_data(entry.get_mut().data.0.get_mut(), &mut Vec::new());
                entry.free();
            }
        }
    }
}

/// Walks the GC-relevant children of a `HeapData` value and calls `on_child`
/// for each contained `HeapId`.
///
/// The cycle collector's mark/scan phases use this directly, combining the
/// child-id iteration with a `HeapId → HeapPtr` conversion in the closure
/// body — that's why this is closure-shaped rather than producing a `Vec`.
/// [`py_dec_ref_ids_for_data`] mirrors the same match arms for the
/// freeing/decref paths; the two must stay in sync.
fn for_each_child_id<H: RuntimeRef, F: FnMut(H)>(data: &HeapData<H>, mut on_child: F) {
    match data {
        HeapData::List(list) => {
            // Skip iteration if no refs - major GC optimization for lists of primitives
            if !list.contains_refs() {
                return;
            }
            for value in list.as_slice() {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            }
        }
        // MUST report exactly the same ids as `Deque::py_dec_ref_ids` — reporting
        // fewer leaks, reporting more is a use-after-free.
        HeapData::Deque(deque) => {
            if !deque.contains_refs() {
                return;
            }
            for value in deque.iter() {
                if let Value::Ref(id) = value {
                    on_child(*id);
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
                    on_child(*id);
                }
            }
        }
        HeapData::NamedTuple(nt) => {
            // Report the owned reference to the class object (factory instances
            // only) before the `contains_refs` early-out — that flag is computed
            // from `items` alone, so an all-primitive instance like `Point(1, 2)`
            // still has a live class edge. MUST report exactly the same ids as
            // `NamedTuple::py_dec_ref_ids`.
            if let Some(class_id) = nt.class_id() {
                on_child(class_id);
            }
            // Skip iteration if no refs - GC optimization for namedtuples of primitives
            if !nt.contains_refs() {
                return;
            }
            for value in nt.as_vec() {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            }
        }
        // A namedtuple *class* owns its default values and its `__module__`.
        // MUST report the same ids as `NamedTupleClass::py_dec_ref_ids`.
        HeapData::NamedTupleClass(class) => {
            if !class.contains_refs() {
                return;
            }
            for value in class.defaults().iter().chain(once(class.module())) {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            }
        }
        HeapData::Dict(dict) => {
            // Report the default_factory (a defaultdict with a heap-ref factory,
            // e.g. a lambda) before the `has_refs` early-out. MUST report exactly
            // the same ids as `Dict::py_dec_ref_ids`: reporting fewer leaks,
            // reporting more is a use-after-free.
            if let Some(Value::Ref(id)) = dict.default_factory() {
                on_child(*id);
            }
            // Skip iteration if no refs - major GC optimization for dicts of primitives
            if !dict.has_refs() {
                return;
            }
            for (k, v) in dict {
                if let Value::Ref(id) = k {
                    on_child(*id);
                }
                if let Value::Ref(id) = v {
                    on_child(*id);
                }
            }
        }
        HeapData::DictKeysView(view) => {
            on_child(view.dict_id());
        }
        HeapData::DictItemsView(view) => {
            on_child(view.dict_id());
        }
        HeapData::DictValuesView(view) => {
            on_child(view.dict_id());
        }
        HeapData::Set(set) => {
            for value in set.storage().iter() {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            }
        }
        HeapData::FrozenSet(frozenset) => {
            for value in frozenset.storage().iter() {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            }
        }
        HeapData::Closure(closure) => {
            // Add captured cells to work list
            for cell_id in &closure.cells {
                on_child(*cell_id);
            }
            // Add default values that are heap references
            for default in &closure.defaults {
                if let Value::Ref(id) = default {
                    on_child(*id);
                }
            }
            if let Some(globals) = closure.globals {
                on_child(globals);
            }
        }
        HeapData::FunctionDefaults(fd) => {
            // Add default values that are heap references
            for default in &fd.defaults {
                if let Value::Ref(id) = default {
                    on_child(*id);
                }
            }
            if let Some(globals) = fd.globals {
                on_child(globals);
            }
        }
        HeapData::Cell(cell) => {
            // Cell can contain a reference to another heap value
            if let Value::Ref(id) = &cell.0 {
                on_child(*id);
            }
        }
        HeapData::HostClass(dc) => {
            // The owned class entry, then the attrs Dict — MUST report exactly
            // the same ids as `HostClass::py_dec_ref_ids`.
            on_child(dc.class_id());
            for (k, v) in dc.attrs() {
                if let Value::Ref(id) = k {
                    on_child(*id);
                }
                if let Value::Ref(id) = v {
                    on_child(*id);
                }
            }
        }
        HeapData::HostClassType(t) => {
            // Eager class attrs can hold containers the sandbox can reach
            // (`Klass.data`) and mutate to close a cycle back to this type
            // object, so they must be traced like `HostClass` attrs.
            for (k, v) in t.attrs() {
                if let Value::Ref(id) = k {
                    on_child(*id);
                }
                if let Value::Ref(id) = v {
                    on_child(*id);
                }
            }
        }
        HeapData::Class(class) => {
            // The class namespace holds method/class-variable values.
            for (k, v) in class.namespace() {
                if let Value::Ref(id) = k {
                    on_child(*id);
                }
                if let Value::Ref(id) = v {
                    on_child(*id);
                }
            }
        }
        HeapData::Instance(instance) => {
            // An instance references its class plus its attribute dict's entries.
            on_child(instance.class());
            for (k, v) in instance.attrs() {
                if let Value::Ref(id) = k {
                    on_child(*id);
                }
                if let Value::Ref(id) = v {
                    on_child(*id);
                }
            }
        }
        HeapData::BoundMethod(bm) => {
            if let Value::Ref(id) = &bm.instance {
                on_child(*id);
            }
            if let Value::Ref(id) = &bm.func {
                on_child(*id);
            }
        }
        HeapData::DataclassField(field) => {
            // A captured default or factory can reach back to the class the
            // field belongs to (`x: object = SomeInstanceOfIt`, or a factory
            // closure capturing the class), closing a cycle.
            for value in field.ref_children() {
                on_child(value);
            }
        }
        HeapData::ListIterator(iter) => on_child(iter.list_id()),
        HeapData::DequeIterator(iter) => on_child(iter.deque_id()),
        HeapData::TupleIterator(iter) => on_child(iter.source_id()),
        HeapData::StringIterator(iter) => {
            if let Some(id) = iter.source_id() {
                on_child(id);
            }
        }
        HeapData::BytesIterator(iter) => {
            if let Some(id) = iter.source_id() {
                on_child(id);
            }
        }
        HeapData::RangeIterator(_) => {}
        HeapData::DictKeyIterator(iter) => on_child(iter.source_id()),
        HeapData::DictItemIterator(iter) => on_child(iter.source_id()),
        HeapData::DictValueIterator(iter) => on_child(iter.source_id()),
        HeapData::SetIterator(iter) => on_child(iter.source_id()),
        HeapData::CallableIterator(iter) => iter.for_each_child_id(on_child),
        HeapData::DirScan(scan) => scan.for_each_child_id(on_child),
        HeapData::Itertools(iter) => iter.for_each_child_id(on_child),
        HeapData::Csv(csv) => csv.for_each_child_id(on_child),
        HeapData::Partial(partial) => partial.for_each_child_id(on_child),
        HeapData::GenericAlias(alias) => alias.for_each_child_id(on_child),
        HeapData::Union(union) => union.for_each_child_id(on_child),
        HeapData::Module(m) => {
            // Module attrs can contain references to heap values
            if !m.has_refs() {
                return;
            }
            for (k, v) in m.attrs() {
                if let Value::Ref(id) = k {
                    on_child(*id);
                }
                if let Value::Ref(id) = v {
                    on_child(*id);
                }
            }
        }
        HeapData::Coroutine(coro) => {
            // Add namespace values that are heap references
            for value in &coro.namespace {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            }
            if let Some(globals) = coro.globals {
                on_child(globals);
            }
        }
        HeapData::GatherFuture(gather) => {
            // Add inc_ref'd item HeapIds. Both coroutines and external
            // futures are owned by the gather for its entire lifecycle.
            for item in &gather.items {
                on_child(*item);
            }
            // Walk per-state heap refs: in-flight slot results plus this
            // gather's own awaiter (if `GatherSlot`, it owns an inc_ref on
            // the outer gather), or the cached result list once the gather
            // has completed successfully. `Pending` and `Failed` carry no
            // heap refs.
            match &gather.state {
                GatherState::Awaited(awaited) => {
                    if let Awaiter::GatherSlot { gather, .. } = &awaited.awaiter {
                        on_child(*gather);
                    }
                    for result in awaited.results.iter().flatten() {
                        if let Value::Ref(id) = result {
                            on_child(*id);
                        }
                    }
                }
                GatherState::Completed(Value::Ref(id)) => on_child(*id),
                GatherState::Pending | GatherState::Failed(_) | GatherState::Completed(_) => {}
            }
        }
        HeapData::ExternalFuture(fut) => {
            // `Pending { awaiter: Some(GatherSlot { gather, .. }) }` owns an
            // inc_ref on `gather`. `Awaiter::Task` / `None` and the `Failed`
            // state carry no heap refs. `Resolved` owns the cached value, and
            // a pending sleep owns the `result` it will resolve with.
            if let Some(Value::Ref(id)) = &fut.sleep_result {
                on_child(*id);
            }
            match &fut.state {
                ExternalFutureState::Resolved(Value::Ref(id)) => on_child(*id),
                ExternalFutureState::Pending {
                    awaiter: Some(Awaiter::GatherSlot { gather, .. }),
                } => on_child(*gather),
                _ => {}
            }
        }
        HeapData::DateTime(dt) => {
            // Aware datetimes retain a heap reference to the tzinfo object so that
            // `dt.tzinfo is tz` identity is preserved across attribute lookups.
            // GC must follow that reference, otherwise the timezone gets swept
            // while the datetime still points at the freed slot.
            if let Some(tz_id) = dt.tzinfo_ref() {
                on_child(tz_id);
            }
        }
        HeapData::Time(t) => {
            // Same retained-tzinfo contract as `DateTime` above.
            if let Some(tz_id) = t.tzinfo_ref() {
                on_child(tz_id);
            }
        }
        HeapData::OpenFile(file) => {
            // Kept in sync with `py_dec_ref_ids_for_data`: the file owns one
            // ref on its loaded buffer. (`OpenFile` is not GC-tracked today, so
            // this arm is not reached by the collector, but the two walkers must
            // mirror each other per the contract above.)
            if let Some(buffer_id) = file.buffer_id() {
                on_child(buffer_id);
            }
        }
        HeapData::ReMatch(m) => {
            // Mirror `py_dec_ref_ids_for_data`: a match holds one ref on its
            // shared subject string (`None` for an interned subject).
            if let Value::Ref(id) = m.subject_ref() {
                on_child(*id);
            }
        }
        // Leaf types with no heap references
        _ => {}
    }
}

fn py_dec_ref_ids_for_data<H: RuntimeRef>(data: &mut HeapData<H>, stack: &mut Vec<H>) {
    match data {
        HeapData::Str(s) => s.py_dec_ref_ids(stack),
        HeapData::Bytes(b) => b.py_dec_ref_ids(stack),
        HeapData::List(l) => l.py_dec_ref_ids(stack),
        HeapData::Deque(d) => d.py_dec_ref_ids(stack),
        HeapData::Tuple(t) => t.py_dec_ref_ids(stack),
        HeapData::NamedTuple(nt) => nt.py_dec_ref_ids(stack),
        HeapData::NamedTupleClass(class) => class.py_dec_ref_ids(stack),
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
            stack.extend(closure.globals);
        }
        HeapData::FunctionDefaults(fd) => {
            // Decrement ref count for default values that are heap references
            for default in &mut fd.defaults {
                default.py_dec_ref_ids(stack);
            }
            stack.extend(fd.globals);
        }
        HeapData::Cell(cell) => cell.0.py_dec_ref_ids(stack),
        HeapData::HostClass(dc) => dc.py_dec_ref_ids(stack),
        HeapData::HostClassType(t) => t.py_dec_ref_ids(stack),
        HeapData::Class(class) => class.py_dec_ref_ids(stack),
        HeapData::Instance(instance) => instance.py_dec_ref_ids(stack),
        HeapData::BoundMethod(bm) => bm.py_dec_ref_ids(stack),
        HeapData::DataclassField(field) => field.py_dec_ref_ids(stack),
        HeapData::DataclassParams(params) => params.py_dec_ref_ids(stack),
        HeapData::ListIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::DequeIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::TupleIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::StringIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::BytesIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::RangeIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::DictKeyIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::DictItemIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::DictValueIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::SetIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::CallableIterator(iter) => iter.py_dec_ref_ids(stack),
        HeapData::DirScan(scan) => scan.py_dec_ref_ids(stack),
        HeapData::Itertools(iter) => iter.py_dec_ref_ids(stack),
        HeapData::Csv(csv) => csv.py_dec_ref_ids(stack),
        HeapData::Partial(partial) => partial.py_dec_ref_ids(stack),
        HeapData::GenericAlias(alias) => alias.py_dec_ref_ids(stack),
        HeapData::Union(union) => union.py_dec_ref_ids(stack),
        HeapData::Module(m) => m.py_dec_ref_ids(stack),
        HeapData::Coroutine(coro) => {
            // Decrement ref count for namespace values that are heap references
            for value in &mut coro.namespace {
                value.py_dec_ref_ids(stack);
            }
            stack.extend(coro.globals);
        }
        HeapData::GatherFuture(gather) => {
            // Decrement ref count for owned item HeapIds (coroutines and
            // external futures are both owned by the gather).
            stack.extend(gather.items.iter().copied());
            // Release per-state heap refs: in-flight slot results plus this
            // gather's own awaiter (if `GatherSlot`, it owns an inc_ref on
            // the outer gather), or the cached result list once the gather
            // has completed successfully. `Pending` and `Failed` carry no
            // heap refs.
            match &mut gather.state {
                GatherState::Awaited(awaited) => {
                    if let Awaiter::GatherSlot { gather, .. } = &awaited.awaiter {
                        stack.push(*gather);
                    }
                    for result in awaited.results.iter_mut().flatten() {
                        result.py_dec_ref_ids(stack);
                    }
                }
                GatherState::Completed(value) => value.py_dec_ref_ids(stack),
                GatherState::Pending | GatherState::Failed(_) => {}
            }
        }
        HeapData::ExternalFuture(fut) => {
            // Mirror `for_each_child_id`: a pending sleep's `result` is owned too.
            if let Some(result) = &mut fut.sleep_result {
                result.py_dec_ref_ids(stack);
            }
            match &mut fut.state {
                ExternalFutureState::Resolved(value) => value.py_dec_ref_ids(stack),
                ExternalFutureState::Pending {
                    awaiter: Some(Awaiter::GatherSlot { gather, .. }),
                } => stack.push(*gather),
                ExternalFutureState::Pending {
                    awaiter: None | Some(Awaiter::Task(_)),
                }
                | ExternalFutureState::Failed(_) => {}
            }
        }
        HeapData::DateTime(dt) => {
            // Mirror `for_each_child_id`: when an aware datetime is freed we must
            // also drop the retained tzinfo reference so its refcount is balanced.
            if let Some(tz_id) = dt.tzinfo_ref() {
                stack.push(tz_id);
            }
        }
        HeapData::Time(t) => {
            // Mirror `for_each_child_id`: an aware time owns its tzinfo reference.
            if let Some(tz_id) = t.tzinfo_ref() {
                stack.push(tz_id);
            }
        }
        HeapData::OpenFile(f) => {
            // Kept in sync with `for_each_child_id`: release the file's owned
            // ref on its loaded buffer when the file is freed (e.g. read but
            // never `close()`d).
            f.py_dec_ref_ids(stack);
        }
        // Release the shared subject reference (mirrors `for_each_child_id`).
        HeapData::ReMatch(m) => m.py_dec_ref_ids(stack),
        // other types have no nested heap references
        _ => {}
    }
}

/// Compile-fail soundness tests for [`HeapReader`].
///
/// Gated behind `--cfg heap_reader_compile_fail_tests` so they are only compiled
/// when the integration test harness runs `cargo rustc` with the appropriate flags.
#[cfg(heap_reader_compile_fail_tests)]
#[path = "../../tests/heap_reader_compile_fail_cases/cases.rs"]
mod heap_reader_compile_fail_cases;

/// Valid borrowed-handle usage, kept here to exercise the private production API under Miri.
#[cfg(test)]
#[path = "../../tests/heap_reader/borrowed.rs"]
mod borrowed_tests;

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
    use monty_types::ResourceTracker;

    use super::*;
    use crate::{
        types::{List, callable_iterator::CallableIterator},
        value::Value,
    };

    /// Returns whether a heap entry is still allocated at `id`.
    fn is_alive<H: RuntimeRef>(heap: &Heap<H>, id: H) -> bool {
        heap.entries.iter().any(|(other, _)| other == id)
    }

    /// Allocates a self-referencing one-element list and returns its id.
    ///
    /// The list's items become `[Value::Ref(id)]` and its refcount is bumped
    /// to 2 to reflect both the caller's ref and the new self-reference.
    fn alloc_self_cycle<H: RuntimeRef>(heap: &Heap<H>) -> H {
        let id = heap.allocate(HeapData::List(List::new(vec![])));
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

    /// Allocates a two-element cycle where one direction has multiplicity 3:
    /// `P → [A, A, A]` and `A → [P]`. Returns `(p_id, a_id)`.
    ///
    /// Final refcounts: `P.rc = 2` (alloc + one edge from A), `A.rc = 4`
    /// (alloc + three edges from P). The caller "owns" one of each — both
    /// can be dropped via `dec_ref` to isolate the cycle.
    ///
    /// Exercises the duplicate-edges-within-one-element shape that
    /// `mark_gray`/`mark_black` must handle correctly: each edge from P
    /// to A is one independent decrement (or increment), but the work
    /// stack must not grow with edge multiplicity — otherwise the outer
    /// pop processes A's children multiple times and over-counts.
    fn alloc_dup_child_cycle<H: RuntimeRef>(heap: &Heap<H>) -> (H, H) {
        let p_id = heap.allocate(HeapData::List(List::new(vec![])));
        let a_id = heap.allocate(HeapData::List(List::new(vec![])));

        let push_refs = |target: H, refs: &[H]| {
            let entry = heap
                .entries
                .iter()
                .find(|(other, _)| *other == target)
                .map(|(_, e)| e)
                .expect("entry just allocated");
            // SAFETY: no other borrow into this entry's data exists during the test.
            let data = unsafe { &mut *entry.data.0.get() };
            match data {
                HeapData::List(list) => {
                    list.set_contains_refs();
                    for r in refs {
                        list.as_vec_mut().push(Value::Ref(*r));
                    }
                }
                _ => unreachable!(),
            }
            for r in refs {
                heap.inc_ref(*r);
            }
        };

        push_refs(p_id, &[a_id, a_id, a_id]);
        push_refs(a_id, &[p_id]);
        (p_id, a_id)
    }

    #[test]
    fn cstack_only_cycle_survives_collection() {
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let id = alloc_self_cycle(heap);

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
            assert!(is_alive(heap, id), "C-stack-rooted cycle was freed");
            assert!(matches!(heap.get(id), HeapData::List(_)));

            // Drop the simulated Rust local. Now the cycle is genuinely isolated
            // (rc 1 = self-pointer only). The next collection must reclaim it.
            heap.dec_ref(id); // rc = 1, re-flagged Purple
            assert_eq!(heap.purple_count, 1);
            heap.collect_cycles();
            assert_eq!(heap.purple_count, 0);
            assert!(!is_alive(heap, id), "isolated cycle should have been freed");
        });
    }

    #[test]
    fn heap_read_rooted_cycle_survives_collection() {
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let id = alloc_self_cycle(heap);

            // Bump `readers` manually to mimic a live `HeapRead` pointing into
            // the entry. The borrow checker prevents holding a real `HeapRead`
            // across `collect_cycles` (which requires `&mut Heap`), so we
            // splice the same counter that `HeapRead::Drop` decrements.
            let readers_before = heap.entries.get(id).readers.get();
            heap.entries.get(id).readers.set(readers_before + 1);

            // Drive the entry into Purple via dec_ref: rc 2 → 1. Without the
            // `readers > 0` special-case in `Scan`, the resulting cycle would
            // be condemned to White and freed.
            heap.dec_ref(id); // rc = 1, flagged Purple
            assert_eq!(heap.purple_count, 1);

            heap.collect_cycles();
            assert!(
                is_alive(heap, id),
                "entry with active HeapRead reader was freed by collect_cycles"
            );

            // Restore the simulated reader so `Heap::drop` can clean up
            // without tripping the `dec_ref` active-readers assertion.
            heap.entries.get(id).readers.set(readers_before);
            // The entry is leaked here on purpose (rc = 1 from the self-ref,
            // no external root remains, but the collector ran already and the
            // color is Black — the next dec_ref would try to recurse into the
            // self-pointer after freeing the entry). `Heap::drop` walks every
            // slot and tears them down regardless of refcount, so leaking
            // here is safe for the duration of the test.
        });
    }

    /// Regression test for a soundness gap in [`Heap::dec_ref`]'s trial-deletion
    /// candidate-enrollment path.
    ///
    /// The surviving-refcount branch calls `heap_entry.data.0.get_mut()` to
    /// check `is_gc_tracked`, even though [`HeapData::is_gc_tracked`] only
    /// needs `&self`. That `UnsafeCell::get_mut()` is a fresh `&mut HeapData`
    /// (Unique) retag of the same allocation the live `HeapRead` is already
    /// pointing into via a `SharedReadWrite` raw pointer, so it invalidates
    /// the prior pointer under Stacked / Tree Borrows.
    ///
    /// Pure `cargo test` cannot observe this — the pointer arithmetic still
    /// reads valid bytes — but `cargo +nightly miri test` flags the access.
    /// The branch is reachable from normal Monty code (e.g. `list.remove`
    /// on a self-referential list holds a `HeapObjectRead<List>`, clones the
    /// matching element, and the deferred drop of that clone calls
    /// `dec_ref` on the same `HeapId` while the list reader is still live).
    #[test]
    fn dec_ref_must_not_invalidate_live_heap_read() {
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let id = heap.allocate(HeapData::List(List::new(vec![])));
            // Bump refcount so `dec_ref` enters the non-freeing branch where
            // the offending `data.0.get_mut()` lives. `List` is GC-tracked, so
            // `is_gc_tracked` returns true and the branch is fully exercised.
            heap.inc_ref(id);

            HeapReader::with(heap, &mut (), |heap, ()| {
                let HeapReadOutput::List(list) = heap.read(id) else {
                    unreachable!()
                };
                // Holding `list` does NOT borrow the heap — only `list.get(heap)`
                // does. That is what lets the borrow checker accept this
                // sequence, while the underlying raw-pointer aliasing is
                // nevertheless violated by the next call.
                heap.dec_ref(id);
                // Read through the now-invalidated `SharedReadWrite` raw pointer.
                // Miri's aliasing model fails here.
                let _ = list.get(heap).as_slice().len();
            });
        });
    }

    #[test]
    fn isolated_simple_cycle_is_collected() {
        // Sanity check: a self-reference cycle with no external rooting
        // gets collected on the next `collect_cycles` call.
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let id = alloc_self_cycle(heap);
            // After alloc_self_cycle: rc = 2 (allocate's 1 + self-ref's 1).
            // Drop the caller's reference. rc 2 → 1, marks Purple.
            heap.dec_ref(id);
            assert_eq!(heap.purple_count, 1);
            heap.collect_cycles();
            assert!(!is_alive(heap, id));
            assert_eq!(heap.purple_count, 0);
        });
    }

    #[test]
    fn empty_tuple_singleton_survives_collection() {
        // The empty-tuple singleton is no longer rooted explicitly by the
        // collector. Its refcount stays ≥ 1 forever (initial heap-owned
        // ref), which is what keeps it alive — verify the collector does
        // not accidentally free it even after spurious Purple flagging.
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            // Fake a dec_ref event that would mark the empty tuple Purple.
            let empty_tuple = heap.entries.id_at(EMPTY_TUPLE_INDEX);
            heap.inc_ref(empty_tuple);
            heap.dec_ref(empty_tuple);
            heap.collect_cycles();
            assert!(
                is_alive(heap, empty_tuple),
                "empty tuple singleton must survive collection"
            );
        });
    }

    #[test]
    fn pending_purple_cycle_round_trips_through_serde() {
        let (bytes, index) = Heap::with_test(16, ResourceTracker::default(), |heap| {
            let id = alloc_self_cycle(heap);
            heap.dec_ref(id);
            assert_eq!(heap.purple_count, 1);
            assert_eq!(heap.entries.get(id).color.get(), CcColor::Purple);
            (minicbor_serde::to_vec(heap).expect("serialize"), id.index())
        });
        let wire = minicbor_serde::from_slice(&bytes).expect("deserialize");
        let mut owner = HeapOwner::from_wire(wire, Vec::new(), None).expect("relocate");
        owner.activate(|state| {
            let restored = &mut state.heap;
            let id = restored.entries.id_at(index);
            assert_eq!(restored.purple_count, 1);
            assert_eq!(restored.entries.get(id).color.get(), CcColor::Purple);
            restored.collect_cycles();
            assert!(!is_alive(restored, id));
            assert_eq!(restored.purple_count, 0);
        });
    }

    #[test]
    fn isolated_cycle_with_duplicate_child_refs_is_collected() {
        // Regression: an unreachable cycle where one element references its
        // sibling multiple times within its child list. The mark phase must
        // (a) decrement the sibling's refcount once per edge, and (b) push
        // the sibling onto the work stack at most once, even when many
        // edges from the same parent target it.
        //
        // Without (b), the outer pop walks the sibling's children once per
        // duplicate edge, over-decrementing the *sibling's* children's
        // refcounts. In debug builds this trips `mark_gray`'s
        // refcount-underflow `debug_assert`; in release it wraps the
        // refcount to `usize::MAX` and `scan` resurrects the cycle —
        // either way, the cycle is not collected.
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let (p_id, a_id) = alloc_dup_child_cycle(heap);
            // After construction: P.rc = 2, A.rc = 4.

            // Drop the caller's refs; the cycle is now genuinely unreachable.
            heap.dec_ref(p_id); // P.rc 2 → 1, flagged Purple
            heap.dec_ref(a_id); // A.rc 4 → 3, flagged Purple
            assert_eq!(heap.purple_count, 2);

            heap.collect_cycles();

            assert!(!is_alive(heap, p_id), "P should be collected");
            assert!(!is_alive(heap, a_id), "A should be collected");
            assert_eq!(heap.purple_count, 0);
        });
    }

    #[test]
    fn cstack_rooted_cycle_with_duplicate_child_refs_collects_after_pin_dropped() {
        // Regression: when `scan` resurrects a Purple cycle via `mark_black`,
        // the per-edge refcount *increment* must mirror `mark_gray`'s
        // per-edge decrement. Duplicate edges from one element to its
        // sibling must not over-increment via repeated outer pops of the
        // sibling.
        //
        // We pin P externally (extra inc_ref) so `scan` is forced to take
        // the resurrect path through `mark_black`. After collection the
        // refcounts must be exactly the pre-collection values — verified by
        // dropping the pin and confirming the next `collect_cycles` reclaims
        // the cycle. If `mark_black` over-incremented during resurrection,
        // dropping the pin leaves the refcounts artificially high and the
        // cycle leaks.
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let (p_id, a_id) = alloc_dup_child_cycle(heap);
            // After construction: P.rc = 2, A.rc = 4.

            // Pin P externally. After the inc_ref + dec_ref pair, P is Purple
            // but its refcount still includes one unit not accounted for by
            // any in-cycle edge — `scan` must resurrect via `mark_black`.
            heap.inc_ref(p_id); // P.rc = 3 (alloc + from A + pin)
            heap.dec_ref(p_id); // P.rc = 2, flagged Purple
            heap.dec_ref(a_id); // A.rc = 3, flagged Purple

            heap.collect_cycles();

            // First pass: cycle survives because of P's external pin.
            assert!(is_alive(heap, p_id), "P should survive (external pin)");
            assert!(is_alive(heap, a_id), "A should survive (reachable via P)");

            // Drop the pin. If `mark_black` correctly restored refcounts during
            // resurrection, the cycle is now isolated and the next collection
            // reclaims it. Over-incrementing in `mark_black` leaves P's
            // refcount artificially high so `scan` resurrects it again.
            heap.dec_ref(p_id);
            heap.collect_cycles();

            assert!(!is_alive(heap, p_id), "P should be collected after pin dropped");
            assert!(!is_alive(heap, a_id), "A should be collected after pin dropped");
        });
    }

    /// The GC must see BOTH refs a `callable_iterator` owns, with correct
    /// multiplicity: under-tracing here would let a live object be collected.
    #[test]
    fn callable_iterator_traces_callable_and_sentinel() {
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let c = heap.allocate(HeapData::List(List::new(vec![])));
            let s = heap.allocate(HeapData::List(List::new(vec![])));

            heap.inc_ref(c);
            heap.inc_ref(s);
            let iter = heap.allocate(HeapData::CallableIterator(CallableIterator::new(
                Value::Ref(c),
                Value::Ref(s),
            )));

            let mut traced = vec![];
            for_each_child_id(heap.get(iter), |id| traced.push(id));
            assert_eq!(traced, vec![c, s], "callable and sentinel are both traced");

            // Multiplicity: when callable IS sentinel, two counted refs point at one
            // object, so the id must be reported twice or trial deletion
            // under-decrements and frees a live object.
            heap.inc_ref(c);
            heap.inc_ref(c);
            let shared = heap.allocate(HeapData::CallableIterator(CallableIterator::new(
                Value::Ref(c),
                Value::Ref(c),
            )));

            let mut dup = vec![];
            for_each_child_id(heap.get(shared), |id| dup.push(id));
            assert_eq!(dup, vec![c, c], "a shared callable/sentinel is traced twice");
        });
    }

    /// End-to-end: a cycle through the callable must be collected and the
    /// non-cycle sentinel released — exercising the mark phase
    /// (`for_each_child_id`) and the free phase (`py_dec_ref_ids`) together.
    #[test]
    fn callable_iterator_cycle_is_collected() {
        Heap::with_test(16, ResourceTracker::default(), |heap| {
            let sentinel = heap.allocate(HeapData::List(List::new(vec![])));
            let list = heap.allocate(HeapData::List(List::new(vec![])));
            heap.inc_ref(list);
            heap.inc_ref(sentinel);
            let iter = heap.allocate(HeapData::CallableIterator(CallableIterator::new(
                Value::Ref(list),
                Value::Ref(sentinel),
            )));

            // Close the cycle: the callable list references the iterator back.
            heap.inc_ref(iter);
            HeapReader::with(heap, &mut (), |reader, ()| {
                let HeapReadOutput::List(l) = reader.read(list) else {
                    unreachable!("just allocated a list")
                };
                let l = l.get_mut(reader);
                l.set_contains_refs();
                l.as_vec_mut().push(Value::Ref(iter));
            });
            // list.rc = 2, iter.rc = 2, sentinel.rc = 2.

            // Drop the allocation refs: {iter, list} is now an unreachable cycle and
            // the sentinel is held only by the iterator.
            heap.dec_ref(list);
            heap.dec_ref(iter);
            heap.dec_ref(sentinel);
            heap.collect_cycles();
            assert!(!is_alive(heap, iter), "callable_iterator in a cycle must be collected");
            assert!(!is_alive(heap, list), "the cycle's other node must be collected");
            assert!(
                !is_alive(heap, sentinel),
                "sentinel held only by the freed iterator must be released"
            );
        });
    }
}
