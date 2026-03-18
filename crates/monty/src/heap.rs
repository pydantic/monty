use std::{
    cell::Cell,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    mem::size_of,
    vec,
};

use smallvec::SmallVec;

// Re-export items moved to `heap_traits` so that `crate::heap::HeapGuard` etc. continue
// to resolve (used by the `defer_drop!` macros and throughout the codebase).
pub(crate) use crate::heap_data::HeapData;
pub(crate) use crate::heap_traits::{ContainsHeap, DropWithHeap, HeapGuard, HeapItem, ImmutableHeapGuard};
use crate::{
    args::ArgValues,
    asyncio::GatherItem,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult},
    heap_data::HeapDataMut,
    intern::Interns,
    resource::{ResourceError, ResourceTracker, check_mult_size, check_repeat_size},
    types::{List, LongInt, PyTrait, Tuple, allocate_tuple},
    value::{EitherStr, Value},
};

/// Unique identifier for values stored inside the heap arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct HeapId(usize);

impl HeapId {
    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0
    }
}

/// The empty tuple is a singleton which is allocated at startup.
const EMPTY_TUPLE_ID: HeapId = HeapId(0);

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
            | HeapData::LongInt(_) => Self::Unknown,
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

/// A single entry inside the heap arena, storing refcount, payload, and hash metadata.
///
/// The `hash_state` field tracks whether the heap entry is hashable and, if so,
/// caches the computed hash. Mutable types (List, Dict) start as `Unhashable` and
/// will raise TypeError if used as dict keys.
///
/// The `data` field is an Option to support temporary borrowing: when methods like
/// `with_entry_mut` or `call_attr` need mutable access to both the data and the heap,
/// they can `.take()` the data out (leaving `None`), pass `&mut Heap` to user code,
/// then restore the data. This avoids unsafe code while keeping `refcount` accessible
/// for `inc_ref`/`dec_ref` during the borrow.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct HeapValue {
    refcount: Cell<usize>,
    /// The payload data. Temporarily `None` while borrowed via `with_entry_mut`/`call_attr`.
    data: Option<HeapData>,
    /// Current hashing status / cached hash value
    hash_state: HashState,
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
/// Generic over `T: ResourceTracker` to support different resource tracking strategies.
/// When `T = NoLimitTracker` (the default), all resource checks compile away to no-ops.
///
/// Serialization requires `T: Serialize` and `T: Deserialize`. Custom serde implementation
/// handles the Drop constraint by using `std::mem::take` during serialization.
#[derive(Debug)]
pub(crate) struct Heap<T: ResourceTracker> {
    entries: Vec<Option<HeapValue>>,
    /// IDs of freed slots available for reuse. Populated by `dec_ref`, consumed by `allocate`.
    free_list: Vec<HeapId>,
    /// Resource tracker for enforcing limits and scheduling GC.
    tracker: T,
    /// True if reference cycles may exist. Set when a container stores a Ref,
    /// cleared after GC completes. When false, GC can skip mark-sweep entirely.
    may_have_cycles: bool,
    /// Number of GC applicable allocations since the last GC.
    allocations_since_gc: u32,
    /// Current recursion depth — incremented on function calls and data structure traversals.
    ///
    /// Uses `Cell` for interior mutability so that methods with only `&Heap`
    /// (like `py_repr_fmt`) can still increment/decrement the depth counter.
    recursion_depth: Cell<usize>,
}

impl<T: ResourceTracker + serde::Serialize> serde::Serialize for Heap<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Heap", 6)?;
        state.serialize_field("entries", &self.entries)?;
        state.serialize_field("free_list", &self.free_list)?;
        state.serialize_field("tracker", &self.tracker)?;
        state.serialize_field("may_have_cycles", &self.may_have_cycles)?;
        state.serialize_field("allocations_since_gc", &self.allocations_since_gc)?;
        state.end()
    }
}

impl<'de, T: ResourceTracker + serde::Deserialize<'de>> serde::Deserialize<'de> for Heap<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct HeapFields<T> {
            entries: Vec<Option<HeapValue>>,
            free_list: Vec<HeapId>,
            tracker: T,
            may_have_cycles: bool,
            allocations_since_gc: u32,
        }
        let fields = HeapFields::<T>::deserialize(deserializer)?;
        Ok(Self {
            entries: fields.entries,
            free_list: fields.free_list,
            tracker: fields.tracker,
            may_have_cycles: fields.may_have_cycles,
            allocations_since_gc: fields.allocations_since_gc,
            recursion_depth: Cell::new(0),
        })
    }
}

macro_rules! take_data {
    ($self:ident, $id:expr, $func_name:literal) => {
        $self
            .entries
            .get_mut($id.index())
            .expect(concat!("Heap::", $func_name, ": slot missing"))
            .as_mut()
            .expect(concat!("Heap::", $func_name, ": object already freed"))
            .data
            .take()
            .expect(concat!("Heap::", $func_name, ": data already borrowed"))
    };
}

macro_rules! restore_data {
    ($self:ident, $id:expr, $new_data:expr, $func_name:literal) => {{
        let entry = $self
            .entries
            .get_mut($id.index())
            .expect(concat!("Heap::", $func_name, ": slot missing"))
            .as_mut()
            .expect(concat!("Heap::", $func_name, ": object already freed"));
        entry.data = Some($new_data);
    }};
}

/// GC interval - run GC every 100,000 applicable allocations.
///
/// This is intentionally infrequent to minimize overhead while still
/// eventually collecting reference cycles.
const GC_INTERVAL: u32 = 100_000;

impl<T: ResourceTracker> Heap<T> {
    /// Creates a new heap with the given resource tracker.
    ///
    /// Use this to create heaps with custom resource limits or GC scheduling.
    pub fn new(capacity: usize, tracker: T) -> Self {
        let mut this = Self {
            entries: Vec::with_capacity(capacity),
            free_list: Vec::new(),
            tracker,
            may_have_cycles: false,
            allocations_since_gc: 0,
            recursion_depth: Cell::new(0),
        };
        // TBC: should the empty tuple contribute to the resource limits?
        // If not, can just place it in `entries` directly without going through `allocate()`.
        let empty_tuple = this
            .allocate(HeapData::Tuple(Tuple::default()))
            .expect("Failed to allocate empty tuple singleton");
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

    /// Number of entries in the heap
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    /// Marks that a reference cycle may exist in the heap.
    ///
    /// Call this when a container (list, dict, tuple, etc.) stores a reference
    /// to another heap object. This enables the GC to skip mark-sweep entirely
    /// when no cycles are possible.
    #[inline]
    pub fn mark_potential_cycle(&mut self) {
        self.may_have_cycles = true;
    }

    /// Returns the number of GC-tracked allocations since the last garbage collection.
    ///
    /// This counter increments for each allocation of a GC-tracked type (List, Dict, etc.)
    /// and resets to 0 when `collect_garbage` runs. Useful for testing GC behavior.
    #[cfg(feature = "ref-count-return")]
    pub fn get_allocations_since_gc(&self) -> u32 {
        self.allocations_since_gc
    }

    /// Allocates a new heap entry.
    ///
    /// Returns `Err(ResourceError)` if allocation would exceed configured limits.
    /// Use this when you need to handle resource limit errors gracefully.
    ///
    /// Only GC-tracked types (containers that can hold references) count toward the
    /// GC allocation threshold. Leaf types like strings don't trigger GC.
    ///
    /// When allocating a container that contains heap references, marks potential
    /// cycles to enable garbage collection.
    pub fn allocate(&mut self, data: HeapData) -> Result<HeapId, ResourceError> {
        self.tracker.on_allocate(|| data.py_estimate_size())?;
        if data.is_gc_tracked() {
            self.allocations_since_gc = self.allocations_since_gc.wrapping_add(1);
            // Mark potential cycles if this container has heap references.
            // This is essential for types like Dict where setitem doesn't call
            // mark_potential_cycle() - the allocation is the only place to detect refs.
            if data.has_refs() {
                self.may_have_cycles = true;
            }
        }

        let hash_state = HashState::for_data(&data);
        let new_entry = HeapValue {
            refcount: Cell::new(1),
            data: Some(data),
            hash_state,
        };

        let id = if let Some(id) = self.free_list.pop() {
            // Reuse a freed slot
            self.entries[id.index()] = Some(new_entry);
            id
        } else {
            // No free slots, append new entry
            let id = self.entries.len();
            self.entries.push(Some(new_entry));
            HeapId(id)
        };

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
    pub fn get_empty_tuple(&mut self) -> Value {
        // Return existing singleton with incremented refcount
        self.inc_ref(EMPTY_TUPLE_ID);
        Value::Ref(EMPTY_TUPLE_ID)
    }

    /// Increments the reference count for an existing heap entry.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn inc_ref(&self, id: HeapId) {
        let value = self
            .entries
            .get(id.index())
            .expect("Heap::inc_ref: slot missing")
            .as_ref()
            .expect("Heap::inc_ref: object already freed");
        value.refcount.update(|r| r + 1);
    }

    /// Decrements the reference count and frees the value (plus children) once it hits zero.
    ///
    /// Uses an iterative work stack instead of recursion to avoid Rust stack overflow
    /// when freeing deeply nested containers (e.g., a list nested 10,000 levels deep).
    /// This is analogous to CPython's "trashcan" mechanism for safe deallocation.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn dec_ref(&mut self, id: HeapId) {
        let mut current_id = id;
        let mut work_stack = Vec::new();
        loop {
            let slot = self
                .entries
                .get_mut(current_id.index())
                .expect("Heap::dec_ref: slot missing");
            let entry = slot.as_mut().expect("Heap::dec_ref: object already freed");
            if entry.refcount.get() > 1 {
                entry.refcount.update(|r| r - 1);
            } else if let Some(value) = slot.take() {
                // refcount == 1, free the value and add slot to free list for reuse
                self.free_list.push(current_id);

                // Notify tracker of freed memory
                if let Some(ref data) = value.data {
                    self.tracker.on_free(|| data.py_estimate_size());
                }

                // Collect child IDs and push onto work stack for iterative processing
                if let Some(mut data) = value.data {
                    data.py_dec_ref_ids(&mut work_stack);
                    drop(data);
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
        self.entries
            .get(id.index())
            .expect("Heap::get: slot missing")
            .as_ref()
            .expect("Heap::get: object already freed")
            .data
            .as_ref()
            .expect("Heap::get: data currently borrowed")
    }

    /// Returns a mutable reference to the heap data stored at the given ID.
    ///
    /// # Panics
    /// Panics if the value ID is invalid, the value has already been freed,
    /// or the data is currently borrowed via `with_entry_mut`/`call_attr`.
    pub fn get_mut(&mut self, id: HeapId) -> HeapDataMut<'_> {
        self.entries
            .get_mut(id.index())
            .expect("Heap::get_mut: slot missing")
            .as_mut()
            .expect("Heap::get_mut: object already freed")
            .data
            .as_mut()
            .expect("Heap::get_mut: data currently borrowed")
            .to_mut()
    }

    /// Returns or computes the hash for the heap entry at the given ID.
    ///
    /// Hashes are computed lazily on first use and then cached. Returns
    /// `Ok(Some(hash))` for immutable types, `Ok(None)` for mutable types,
    /// or `Err(ResourceError::Recursion)` if the recursion limit is exceeded.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn get_or_compute_hash(&mut self, id: HeapId, interns: &Interns) -> Result<Option<u64>, ResourceError> {
        let entry = self
            .entries
            .get_mut(id.index())
            .expect("Heap::get_or_compute_hash: slot missing")
            .as_mut()
            .expect("Heap::get_or_compute_hash: object already freed");

        match entry.hash_state {
            HashState::Unhashable => return Ok(None),
            HashState::Cached(hash) => return Ok(Some(hash)),
            HashState::Unknown => {}
        }

        // Handle Cell specially - uses identity-based hashing (like Python cell objects)
        if let Some(HeapData::Cell(_)) = &entry.data {
            let mut hasher = DefaultHasher::new();
            id.hash(&mut hasher);
            let hash = hasher.finish();
            entry.hash_state = HashState::Cached(hash);
            return Ok(Some(hash));
        }

        // Compute hash lazily - need to temporarily take data to avoid borrow conflict.
        // IMPORTANT: data must be restored to the entry on ALL paths (including errors)
        // to avoid dropping HeapData containing Value::Ref without proper cleanup.
        let mut data = entry.data.take().expect("Heap::get_or_compute_hash: data borrowed");
        let hash = data.to_mut().compute_hash_if_immutable(self, interns);

        // Restore data before handling the result
        let entry = self
            .entries
            .get_mut(id.index())
            .expect("Heap::get_or_compute_hash: slot missing after compute")
            .as_mut()
            .expect("Heap::get_or_compute_hash: object freed during compute");
        entry.data = Some(data);

        // Now handle the result and cache if successful
        let hash = hash?;
        entry.hash_state = match hash {
            Some(value) => HashState::Cached(value),
            None => HashState::Unhashable,
        };
        Ok(hash)
    }

    /// Calls an attribute on the heap entry, returning an `CallResult` that may signal
    /// OS, external, or method calls.
    ///
    /// Temporarily takes ownership of the payload to avoid borrow conflicts when attribute
    /// implementations also need mutable heap access (e.g. for refcounting).
    ///
    /// Returns `CallResult` which may be:
    /// - `Value(v)` - Method completed synchronously with value `v`
    /// - `OsCall(func, args)` - Method needs OS operation; VM should yield to host
    /// - `ExternalCall(id, args)` - Method needs external function call
    /// - `MethodCall(name, args)` - Dataclass method call; VM should yield to host
    pub fn call_attr(vm: &mut VM<'_, '_, T>, id: HeapId, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        // Take data out so the borrow of self.entries ends
        let heap = &mut *vm.heap;
        let mut data = take_data!(heap, id, "call_attr");

        let result = data.py_call_attr(id, vm, attr, args);

        // Restore data
        let heap = &mut *vm.heap;
        restore_data!(heap, id, data, "call_attr");
        result
    }

    /// Gives mutable access to a heap entry while allowing reentrant heap usage
    /// inside the closure (e.g. to read other values or allocate results).
    ///
    /// The data is temporarily taken from the heap entry, so the closure can safely
    /// mutate both the entry data and the heap (e.g. to allocate new values).
    /// The data is automatically restored after the closure completes.
    pub fn with_entry_mut<'a, 'p, F, R>(vm: &mut VM<'a, 'p, T>, id: HeapId, f: F) -> R
    where
        F: FnOnce(&mut VM<'a, 'p, T>, HeapDataMut) -> R,
    {
        // Take data out in a block so the borrow of self.entries ends
        let heap = &mut *vm.heap;
        let mut data = take_data!(heap, id, "with_entry_mut");

        let result = f(vm, data.to_mut());

        // Restore data
        let heap = &mut *vm.heap;
        restore_data!(heap, id, data, "with_entry_mut");
        result
    }

    /// Temporarily takes ownership of two heap entries so their data can be borrowed
    /// simultaneously while still permitting mutable access to the VM (e.g. to
    /// allocate results). Automatically restores both entries after the closure
    /// finishes executing.
    ///
    /// This is a static method that takes `&mut VM` instead of `&mut self` so that
    /// the closure receives `&mut VM` — matching the `with_entry_mut` pattern and
    /// allowing the closure to call methods that need `vm` (e.g. `py_eq`).
    pub fn with_two<'a, 'p, F, R>(vm: &mut VM<'a, 'p, T>, left: HeapId, right: HeapId, f: F) -> R
    where
        F: FnOnce(&mut VM<'a, 'p, T>, &HeapData, &HeapData) -> R,
    {
        if left == right {
            // Same value - take data once and pass it twice
            let heap = &mut *vm.heap;
            let data = take_data!(heap, left, "with_two");

            let result = f(vm, &data, &data);

            let heap = &mut *vm.heap;
            restore_data!(heap, left, data, "with_two");
            result
        } else {
            // Different values - take both
            let heap = &mut *vm.heap;
            let left_data = take_data!(heap, left, "with_two (left)");
            let right_data = take_data!(heap, right, "with_two (right)");

            let result = f(vm, &left_data, &right_data);

            // Restore in reverse order
            let heap = &mut *vm.heap;
            restore_data!(heap, right, right_data, "with_two (right)");
            restore_data!(heap, left, left_data, "with_two (left)");
            result
        }
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
        self.entries
            .get(id.index())
            .expect("Heap::get_refcount: slot missing")
            .as_ref()
            .expect("Heap::get_refcount: object already freed")
            .refcount
            .get()
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
        // 1.. to skip index 0 which is the empty tuple singleton
        self.entries[1..].iter().filter(|o| o.is_some()).count()
    }

    /// Helper for List in-place add: extends the destination vec with items from a heap list.
    ///
    /// This method exists to work around borrow checker limitations when List::py_iadd
    /// needs to read from one heap entry while extending another. By keeping both
    /// the read and the refcount increments within Heap's impl block, we can use the
    /// take/restore pattern to avoid the lifetime propagation issues.
    ///
    /// Returns `true` if successful, `false` if the source ID is not a List.
    pub fn iadd_extend_list(&mut self, source_id: HeapId, dest: &mut Vec<Value>) -> bool {
        if let HeapData::List(list) = self.get(source_id) {
            let items: Vec<Value> = list.as_slice().iter().map(|v| v.clone_with_heap(self)).collect();
            dest.extend(items);
            true
        } else {
            false
        }
    }

    /// Multiplies a heap-allocated value by an `i64`.
    ///
    /// If `id` refers to a `LongInt`, performs integer multiplication with a size
    /// pre-check. Otherwise, treats `id` as a sequence and `int_val` as the repeat
    /// count. This avoids multiple `heap.get()` calls by looking up the data once.
    ///
    /// Returns `Ok(None)` if the heap entry is neither a LongInt nor a sequence type.
    pub fn mult_ref_by_i64(&mut self, id: HeapId, int_val: i64) -> RunResult<Option<Value>> {
        if let HeapData::LongInt(li) = self.get(id) {
            check_mult_size(li.bits(), i64_bits(int_val), &self.tracker)?;
            let result = LongInt::new(li.inner().clone()) * LongInt::from(int_val);
            Ok(Some(result.into_value(self)?))
        } else {
            let count = i64_to_repeat_count(int_val)?;
            self.mult_sequence(id, count)
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

    /// Returns whether garbage collection should run.
    ///
    /// True if reference cycles count exist in the heap
    /// and the number of allocations since the last GC exceeds the interval.
    #[inline]
    pub fn should_gc(&self) -> bool {
        self.may_have_cycles && self.allocations_since_gc >= GC_INTERVAL
    }

    /// Runs mark-sweep garbage collection to free unreachable cycles.
    ///
    /// This method takes a closure that provides an iterator of root HeapIds
    /// (typically from the VM's globals and stack). It marks all reachable objects starting
    /// from roots, then sweeps (frees) any unreachable objects.
    ///
    /// This is necessary because reference counting alone cannot free cycles
    /// where objects reference each other but are unreachable from the program.
    ///
    /// # Caller Responsibility
    /// The caller should check `should_gc()` before calling this method.
    /// If no cycles are possible, the caller can skip GC entirely.
    ///
    /// # Arguments
    /// * `root` - HeapIds that are roots
    pub fn collect_garbage(&mut self, root: Vec<HeapId>) {
        // Mark phase: collect all reachable IDs using BFS
        // Use Vec<bool> instead of HashSet for O(1) operations without hashing overhead
        let mut reachable: Vec<bool> = vec![false; self.entries.len()];
        let mut work_list: Vec<HeapId> = root;

        while let Some(id) = work_list.pop() {
            let idx = id.index();
            // Skip if out of bounds or already visited
            if idx >= reachable.len() || reachable[idx] {
                continue;
            }
            reachable[idx] = true;

            // Add children to work list
            if let Some(Some(entry)) = self.entries.get(idx)
                && let Some(ref data) = entry.data
            {
                collect_child_ids(data, &mut work_list);
            }
        }

        // Sweep phase: free unreachable values
        for (id, value) in self.entries.iter_mut().enumerate() {
            if reachable[id] {
                continue;
            }

            // This entry is unreachable - free it
            if let Some(value) = value.take() {
                // Notify tracker of freed memory
                if let Some(ref data) = value.data {
                    self.tracker.on_free(|| data.py_estimate_size());
                }

                self.free_list.push(HeapId(id));

                // Mark Values as Dereferenced when ref-count-panic is enabled
                #[cfg(feature = "ref-count-panic")]
                if let Some(mut data) = value.data {
                    data.py_dec_ref_ids(&mut Vec::new());
                }
            }
        }

        // Reset cycle flag after GC - cycles have been collected
        self.may_have_cycles = false;
        self.allocations_since_gc = 0;
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

/// Drop implementation for Heap that marks all contained Objects as Dereferenced
/// before dropping to prevent panics when the `ref-count-panic` feature is enabled.
#[cfg(feature = "ref-count-panic")]
impl<T: ResourceTracker> Drop for Heap<T> {
    fn drop(&mut self) {
        // Mark all contained Objects as Dereferenced before dropping.
        // We use py_dec_ref_ids for this since it handles the marking
        // (we ignore the collected IDs since we're dropping everything anyway).
        let mut dummy_stack = Vec::new();
        for value in self.entries.iter_mut().flatten() {
            if let Some(data) = &mut value.data {
                data.py_dec_ref_ids(&mut dummy_stack);
            }
        }
    }
}
