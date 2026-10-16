//! Single-word, invariantly branded pointers into permanent arena slots.

use std::{
    cell::{Cell, UnsafeCell},
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use serde::{Serialize, Serializer};

use super::{
    CcColor, HeapData, HeapEntry, HeapIndex, Reference, RuntimeRef, UnsafeHeapData, free_list::FreeList,
    reference::sealed,
};
use crate::{heap_data::CellValue, value::Value};

/// A non-owning pointer whose invariant arena brand prevents escape or cross-arena use.
/// Payload liveness remains checked separately; slots themselves survive free and reuse.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HeapObjectPtr<'heap> {
    entry: NonNull<HeapEntry<Self>>,
    brand: PhantomData<fn(&'heap ()) -> &'heap ()>,
}

impl sealed::Sealed for HeapObjectPtr<'_> {}

impl Reference for HeapObjectPtr<'_> {
    #[cfg(feature = "memory-model-checks")]
    const CHECK_DROP: bool = true;
}

impl RuntimeRef for HeapObjectPtr<'_> {
    #[inline]
    fn index(self) -> usize {
        // SAFETY: an active owner borrow, or its private reconstruction, keeps this
        // permanent slot allocated. Numeric identity survives payload destruction.
        unsafe { self.entry.as_ref() }.index
    }

    #[inline]
    fn pointer(self) -> NonNull<HeapEntry<Self>> {
        self.entry
    }

    unsafe fn from_pointer(entry: NonNull<HeapEntry<Self>>) -> Self {
        Self {
            entry,
            brand: PhantomData,
        }
    }
}

impl Hash for HeapObjectPtr<'_> {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.index().hash(state);
    }
}

impl fmt::Debug for HeapObjectPtr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Erased-state destruction and unwinding must never inspect slot storage.
        f.debug_tuple("HeapPtr").field(&self.entry).finish()
    }
}

impl Serialize for HeapObjectPtr<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.index().serialize(serializer)
    }
}

/// Interior mutability covers the complete slot throughout payload destruction and reuse.
struct Slot<H: Reference>(UnsafeCell<MaybeUninit<HeapEntry<H>>>);

const PAGE_SIZE: usize = 256;
type Page<H> = Box<[Slot<H>; PAGE_SIZE]>;

/// Append-only boxed pages; only the sealed owner may create an arena with a fresh brand.
pub(super) struct HeapEntries<H: Reference> {
    pages: UnsafeCell<Vec<Page<H>>>,
    len: Cell<usize>,
    free_list: FreeList,
}

impl<H: RuntimeRef> HeapEntries<H> {
    /// Reserves pages for the owner establishing this arena's fresh brand.
    ///
    /// # Safety
    /// An active `H` must identify exactly one arena kept alive by its owner borrow.
    /// The sealed owner may use an erased `H` internally, provided neither that heap
    /// nor its handles escape without a fresh activation brand.
    pub(super) unsafe fn with_capacity(capacity: usize) -> Self {
        Self {
            pages: UnsafeCell::new((0..capacity.div_ceil(PAGE_SIZE)).map(|_| create_page()).collect()),
            len: Cell::new(0),
            free_list: FreeList::new(),
        }
    }

    /// Counts permanent slots, including vacant payloads.
    pub fn len(&self) -> usize {
        self.len.get()
    }

    /// Resolves allocation and snapshot indexes through this arena's own provenance.
    #[inline]
    pub fn id_at(&self, index: usize) -> H {
        assert!(index < self.len(), "heap index out of bounds");
        // SAFETY: only the directory moves during growth; boxed slots never move.
        let pages = unsafe { &*self.pages.get() };
        let slot = &pages[index / PAGE_SIZE][index % PAGE_SIZE];
        // SAFETY: exactly the prefix below len contains initialized entries. Constructor
        // isolation establishes H, and its owner keeps the permanent slots alive.
        unsafe { H::from_pointer(NonNull::new_unchecked(slot.0.get().cast())) }
    }

    /// Allocates a counted payload while retaining a permanent numeric slot identity.
    pub fn allocate(&self, value: HeapEntry<H>) -> H {
        self.allocate_with_slot(value).0
    }

    /// Returns the initialized slot directly for creating a scoped reader.
    pub fn allocate_with_slot(&self, mut value: HeapEntry<H>) -> (H, NonNull<HeapEntry<H>>) {
        value.live.set(true);
        let id = if let Some(index) = self.free_list.pop() {
            value.index = index.index();
            let id = self.id_at(index.index());
            // SAFETY: vacant slots have no payload borrows and contain only sentinels.
            unsafe { id.pointer().as_ptr().write(value) };
            id
        } else {
            value.index = self.len();
            self.append(value)
        };
        (id, id.pointer())
    }

    /// The invariant brand proves arena membership without a dynamic lookup.
    #[inline]
    #[expect(clippy::unused_self, reason = "the receiver witnesses the arena brand")]
    pub fn pointer(&self, id: H) -> NonNull<HeapEntry<H>> {
        id.pointer()
    }

    /// Keeps payload access tied to an arena borrow and rejects vacant slots.
    #[inline]
    #[expect(clippy::unused_self, reason = "the receiver bounds the returned borrow")]
    pub fn get(&self, id: H) -> &HeapEntry<H> {
        // SAFETY: H proves arena identity/lifetime; this borrow excludes freeing.
        let entry = unsafe { id.pointer().as_ref() };
        assert!(entry.live.get(), "heap data already freed");
        entry
    }

    /// Exclusive projection for destruction; callers also check active readers.
    pub fn entry(&mut self, id: H) -> Option<Entry<'_, H>> {
        // SAFETY: H names this arena and the exclusive borrow excludes payload borrows.
        let value = unsafe { &mut *id.pointer().as_ptr() };
        value.live.get().then_some(Entry {
            value,
            free_list: &mut self.free_list,
        })
    }

    /// Visits initialized live entries for diagnostics.
    #[cfg(any(feature = "ref-count-return", test))]
    pub fn iter(&self) -> impl Iterator<Item = (H, &HeapEntry<H>)> {
        (0..self.len()).filter_map(|index| {
            let id = self.id_at(index);
            // SAFETY: the shared arena borrow excludes freeing while iteration continues.
            let entry = unsafe { id.pointer().as_ref() };
            entry.live.get().then_some((id, entry))
        })
    }

    /// Establishes every slot before owner-local relocation resolves forward edges.
    pub(super) fn reserve(&self, count: usize) {
        while self.len() < count {
            self.append(vacant_entry(self.len()));
        }
    }

    /// Installs one relocated payload without moving its permanent slot.
    pub(super) fn restore_entry(&mut self, index: usize, value: Option<HeapEntry<H>>) {
        if let Some(mut value) = value {
            value.index = index;
            value.live.set(true);
            // SAFETY: owner-local reconstruction owns the arena, and this slot
            // still contains its allocation-free sentinel.
            unsafe { self.id_at(index).pointer().as_ptr().write(value) };
        } else {
            self.free_list.push(HeapIndex::from_index(index));
        }
    }

    /// Initializes one slot, without mutably reborrowing any existing slot.
    fn append(&self, value: HeapEntry<H>) -> H {
        let index = self.len();
        // SAFETY: an active arena is not Sync; only allocation mutates the directory,
        // and it invokes no callbacks capable of reentering this same arena.
        let pages = unsafe { &mut *self.pages.get() };
        if index / PAGE_SIZE == pages.len() {
            pages.push(create_page());
        }
        let slot = &pages[index / PAGE_SIZE][index % PAGE_SIZE];
        // SAFETY: this slot has never held a payload. Its UnsafeCell permits initialization
        // through shared page access without retagging any previously allocated entry.
        unsafe { slot.0.get().write(MaybeUninit::new(value)) };
        self.len.set(index + 1);
        self.id_at(index)
    }
}

impl<H: Reference> fmt::Debug for HeapEntries<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeapEntries")
            .field("len", &self.len.get())
            .finish_non_exhaustive()
    }
}

impl<H: RuntimeRef> Serialize for HeapEntries<H> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq((0..self.len()).map(|index| {
            // SAFETY: serialization's shared borrow excludes freeing and mutation.
            let entry = unsafe { self.id_at(index).pointer().as_ref() };
            entry.live.get().then_some(entry)
        }))
    }
}

impl<H: Reference> Drop for HeapEntries<H> {
    fn drop(&mut self) {
        let pages = self.pages.get_mut();
        for index in 0..self.len.get() {
            // SAFETY: only the initialized prefix is dropped, once per slot.
            unsafe {
                pages[index / PAGE_SIZE][index % PAGE_SIZE]
                    .0
                    .get_mut()
                    .assume_init_drop();
            };
        }
    }
}

/// A live payload projection; freeing retains initialized slot metadata.
pub(super) struct Entry<'a, H: Reference> {
    value: &'a mut HeapEntry<H>,
    free_list: &'a mut FreeList,
}

impl<H: Reference> Entry<'_, H> {
    /// Replaces a payload by an allocation-free vacant sentinel, retaining its index.
    pub fn free(self) -> HeapEntry<H> {
        let index = self.value.index;
        self.free_list.push(HeapIndex::from_index(index));
        mem::replace(self.value, vacant_entry(index))
    }

    /// Mutably borrows a payload without replacing its slot.
    pub fn get_mut(&mut self) -> &mut HeapEntry<H> {
        self.value
    }
}

impl<H: Reference> Deref for Entry<'_, H> {
    type Target = HeapEntry<H>;
    fn deref(&self) -> &Self::Target {
        self.value
    }
}
impl<H: Reference> DerefMut for Entry<'_, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
    }
}

/// Keeps vacant slots initialized without owning allocations or Python references.
fn vacant_entry<H: Reference>(index: usize) -> HeapEntry<H> {
    HeapEntry {
        index,
        live: Cell::new(false),
        refcount: Cell::new(0),
        readers: Cell::new(0),
        color: Cell::new(CcColor::Black),
        data: UnsafeHeapData(UnsafeCell::new(HeapData::Cell(CellValue(Value::Undefined)))),
    }
}

/// Allocates storage without clearing the unused suffix of a page.
fn create_page<H: Reference>() -> Page<H> {
    // SAFETY: every slot contains only UnsafeCell<MaybeUninit<_>>, which permits
    // uninitialized storage. Only the prefix tracked by len is treated as payloads.
    unsafe { Box::<[Slot<H>; PAGE_SIZE]>::new_uninit().assume_init() }
}

const _: () = assert!(size_of::<HeapObjectPtr<'static>>() == size_of::<usize>());
const _: () = assert!(size_of::<Value<HeapObjectPtr<'static>>>() == 16);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<Slot<HeapObjectPtr<'static>>>() == 104);
#[cfg(target_pointer_width = "32")]
const _: () = assert!(size_of::<Slot<HeapObjectPtr<'static>>>() == 72);
