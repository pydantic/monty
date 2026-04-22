use std::{
    mem::ManuallyDrop,
    ptr::addr_of,
    vec::{Drain, IntoIter},
};

use smallvec::SmallVec;

use crate::{
    ResourceTracker,
    heap::{Heap, HeapId, RecursionToken},
    value::Value,
};

/// Heap lifecycle operations for memory tracking and reference cleanup.
///
/// This trait captures the two responsibilities shared by all heap-stored types:
///
/// 1. **Memory estimation** (`py_estimate_size`): reporting approximate byte footprint
///    for resource tracking and memory limit enforcement.
///
/// 2. **Reference collection** (`py_dec_ref_ids`): collecting contained `HeapId`s during
///    reference count decrement so child objects can be freed iteratively.
///
/// Unlike `PyTrait`, which provides Python-level operations (equality, repr, arithmetic),
/// `HeapItem` is purely about heap lifecycle management. This separation allows types like
/// `Closure` and `FunctionDefaults` to participate in heap bookkeeping without needing
/// the full `PyTrait` interface.
///
/// Every `HeapData` variant must implement this trait (either directly on the inner type,
/// or inline in the dispatch for types we don't own like `String`).
pub(crate) trait HeapItem {
    /// Estimates the memory size in bytes of this value.
    ///
    /// Used by resource tracking to enforce memory limits. Returns the approximate
    /// heap footprint including struct overhead and variable-length data (e.g., string
    /// contents, list elements).
    ///
    /// Note: For containers holding `Value::Ref` entries, this counts the size of
    /// the reference slots, not the referenced objects. Nested objects are sized
    /// separately when they are allocated.
    fn py_estimate_size(&self) -> usize;

    /// Pushes any contained `HeapId`s onto the stack for reference counting.
    ///
    /// This is called during `dec_ref` to find nested heap references that
    /// need their refcounts decremented when this value is freed.
    ///
    /// When the `memory-model-checks` feature is enabled, this method also marks all
    /// contained `Value`s as `Dereferenced` to prevent Drop panics. This
    /// co-locates the cleanup logic with the reference collection logic.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>);
}

/// This trait represents types that contain a `Heap`; it allows for more complex structures
/// to participate in the `HeapGuard` pattern.
pub(crate) trait ContainsHeap {
    type ResourceTracker: ResourceTracker;
    fn heap(&self) -> &Heap<Self::ResourceTracker>;
    fn heap_mut(&mut self) -> &mut Heap<Self::ResourceTracker>;
}

impl<T: ResourceTracker> ContainsHeap for Heap<T> {
    type ResourceTracker = T;
    fn heap(&self) -> &Self {
        self
    }
    #[inline]
    fn heap_mut(&mut self) -> &mut Self {
        self
    }
}

/// Trait for types that require heap access for proper cleanup.
///
/// Rust's standard `Drop` trait cannot decrement heap reference counts because it has no
/// access to the `Heap`. This trait provides an explicit drop-with-heap method so that
/// ref-counted values (and containers of them) can properly decrement their counts when
/// they are no longer needed.
///
/// **All types implementing this trait must be cleaned up on every code path** — not just
/// the happy path, but also early returns, conditional branches, `continue`, etc. A missed
/// call on any branch leaks reference counts. Prefer [`defer_drop!`] or [`HeapGuard`] to
/// guarantee cleanup automatically rather than inserting manual calls in every branch.
///
/// Implemented for `Value`, `Option<V>`, `Vec<Value>`, `ArgValues`, iterators, and other
/// types that hold heap references.
pub(crate) trait DropWithHeap: Sized {
    /// Consume `self` and decrement reference counts for any heap-allocated values contained within.
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H);
}

impl DropWithHeap for Value {
    #[inline]
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        Self::drop_with_heap(self, heap);
    }
}

impl<U: DropWithHeap> DropWithHeap for Option<U> {
    #[inline]
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        if let Some(value) = self {
            value.drop_with_heap(heap);
        }
    }
}

impl<U: DropWithHeap> DropWithHeap for Vec<U> {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        for value in self {
            value.drop_with_heap(heap);
        }
    }
}

impl<A: smallvec::Array> DropWithHeap for SmallVec<A>
where
    A::Item: DropWithHeap,
{
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        for value in self {
            value.drop_with_heap(heap);
        }
    }
}

impl<U: DropWithHeap> DropWithHeap for IntoIter<U> {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        for value in self {
            value.drop_with_heap(heap);
        }
    }
}

impl<U: DropWithHeap> DropWithHeap for Drain<'_, U> {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        for value in self {
            value.drop_with_heap(heap);
        }
    }
}

impl<const N: usize> DropWithHeap for [Value; N] {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        for value in self {
            value.drop_with_heap(heap);
        }
    }
}

impl<U: DropWithHeap, V: DropWithHeap> DropWithHeap for (U, V) {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        let (left, right) = self;
        left.drop_with_heap(heap);
        right.drop_with_heap(heap);
    }
}

/// Trait for types that require only an immutable heap reference for cleanup.
///
/// Unlike [`DropWithHeap`], which requires `&mut Heap`, this trait works with `&Heap`.
/// This is needed for cleanup in contexts that only have shared access to the heap,
/// such as `py_repr_fmt` and `py_str` formatting methods.
///
/// Currently implemented for [`RecursionToken`], which decrements the recursion depth
/// counter via interior mutability (`Cell`).
pub(crate) trait DropWithImmutableHeap {
    /// Consume `self` and perform cleanup using an immutable heap reference.
    fn drop_with_immutable_heap<T: ResourceTracker>(self, heap: &Heap<T>);
}

impl DropWithImmutableHeap for RecursionToken {
    #[inline]
    fn drop_with_immutable_heap<T: ResourceTracker>(self, heap: &Heap<T>) {
        heap.decr_recursion_depth();
    }
}

/// RAII guard that ensures a [`DropWithImmutableHeap`] value is cleaned up on every code path.
///
/// Like [`HeapGuard`], but holds an immutable `&Heap<T>` instead of requiring `&mut` access
/// via [`ContainsHeap`]. This is useful in contexts that only have shared access to the heap,
/// such as `py_repr_fmt` formatting methods.
///
/// On the normal path, the guarded value can be borrowed via [`as_parts`](Self::as_parts).
/// The guard's `Drop` impl calls [`DropWithImmutableHeap::drop_with_immutable_heap`]
/// automatically, so cleanup happens on all exit paths.
pub(crate) struct ImmutableHeapGuard<'a, H: ContainsHeap, V: DropWithImmutableHeap> {
    value: ManuallyDrop<V>,
    heap: &'a H,
}

impl<'a, H: ContainsHeap, V: DropWithImmutableHeap> ImmutableHeapGuard<'a, H, V> {
    /// Creates a new `ImmutableHeapGuard` for the given value and immutable heap reference.
    #[inline]
    pub fn new(value: V, heap: &'a H) -> Self {
        Self {
            value: ManuallyDrop::new(value),
            heap,
        }
    }

    /// Borrows the value (immutably) and heap (immutably) out of the guard.
    ///
    /// This is what [`defer_drop_immutable_heap!`] calls internally. The returned
    /// references are tied to the guard's lifetime, so the value cannot escape.
    #[inline]
    pub fn as_parts(&self) -> (&V, &'a H) {
        (&self.value, self.heap)
    }
}

impl<H: ContainsHeap, V: DropWithImmutableHeap> Drop for ImmutableHeapGuard<'_, H, V> {
    fn drop(&mut self) {
        // SAFETY: [DH] - value is never manually dropped until this point
        unsafe { ManuallyDrop::take(&mut self.value) }.drop_with_immutable_heap(self.heap.heap());
    }
}

/// RAII guard that ensures a [`DropWithHeap`] value is cleaned up on every code path.
///
/// The guard's `Drop` impl calls [`DropWithHeap::drop_with_heap`] automatically, so
/// cleanup happens whether the scope exits normally, via `?`, `continue`, early return,
/// or any other branch. This eliminates the need to manually insert `drop_with_heap`
/// calls in every branch.
///
/// On the normal path, the guarded value can be borrowed via [`as_parts`](Self::as_parts) /
/// [`as_parts_mut`](Self::as_parts_mut), or reclaimed via [`into_inner`](Self::into_inner) /
/// [`into_parts`](Self::into_parts) (which consume the guard without dropping the value).
///
/// Prefer the [`defer_drop!`] macro for the common case where you just need to ensure a
/// value is dropped at scope exit. Use `HeapGuard` directly when you need to conditionally
/// reclaim the value (e.g. push it back onto the stack on success) or need mutable access
/// to both the value and heap through [`as_parts_mut`](Self::as_parts_mut).
pub(crate) struct HeapGuard<'a, H: ContainsHeap, V: DropWithHeap> {
    // manually dropped because it needs to be dropped by move.
    value: ManuallyDrop<V>,
    heap: &'a mut H,
}

impl<'a, H: ContainsHeap, V: DropWithHeap> HeapGuard<'a, H, V> {
    /// Creates a new `HeapGuard` for the given value and heap.
    #[inline]
    pub fn new(value: V, heap: &'a mut H) -> Self {
        Self {
            value: ManuallyDrop::new(value),
            heap,
        }
    }

    /// Consumes the guard and returns the contained value without dropping it.
    ///
    /// Use this when the value should survive beyond the guard's scope (e.g. returning
    /// a computed result from a function that used the guard for error-path safety).
    #[inline]
    pub fn into_inner(self) -> V {
        let mut this = ManuallyDrop::new(self);
        // SAFETY: [DH] - `ManuallyDrop::new(self)` prevents `Drop` on self, so we can take the value out
        unsafe { ManuallyDrop::take(&mut this.value) }
    }

    /// Borrows the value (immutably) and heap (mutably) out of the guard.
    ///
    /// This is what [`defer_drop!`] calls internally. The returned references are tied
    /// to the guard's lifetime, so the value cannot escape.
    #[inline]
    pub fn as_parts(&mut self) -> (&V, &mut H) {
        (&self.value, self.heap)
    }

    /// Borrows the value (mutably) and heap (mutably) out of the guard.
    ///
    /// This is what [`defer_drop_mut!`] calls internally. Use this when the value needs
    /// to be mutated in place (e.g. advancing an iterator, swapping during min/max).
    #[inline]
    pub fn as_parts_mut(&mut self) -> (&mut V, &mut H) {
        (&mut self.value, self.heap)
    }

    /// Consumes the guard and returns the value and heap separately, without dropping.
    ///
    /// Use this when you need to reclaim both the value *and* the heap reference — for
    /// example, to push the value back onto the VM stack via the heap owner.
    #[inline]
    pub fn into_parts(self) -> (V, &'a mut H) {
        let mut this = ManuallyDrop::new(self);
        // SAFETY: [DH] - `ManuallyDrop` prevents `Drop` on self, so we can recover the parts
        unsafe { (ManuallyDrop::take(&mut this.value), addr_of!(this.heap).read()) }
    }

    /// Borrows just the heap out of the guard
    #[inline]
    pub fn heap(&mut self) -> &mut H {
        self.heap
    }
}

impl<H: ContainsHeap, V: DropWithHeap> Drop for HeapGuard<'_, H, V> {
    fn drop(&mut self) {
        // SAFETY: [DH] - value is never manually dropped until this point
        unsafe { ManuallyDrop::take(&mut self.value) }.drop_with_heap(self.heap.heap_mut());
    }
}

/// The preferred way to ensure a [`DropWithHeap`] value is cleaned up on every code path.
///
/// Creates a [`HeapGuard`] and immediately rebinds `$value` as `&V` and `$heap` as
/// `&mut H` via [`HeapGuard::as_parts`]. The original owned value is moved into the
/// guard, which will call [`DropWithHeap::drop_with_heap`] when scope exits — whether
/// that's normal completion, early return via `?`, `continue`, or any other branch.
///
/// Beyond safety, this is often much more concise than inserting `drop_with_heap` calls
/// in every branch of complex control flow. For mutable access to the value, use
/// [`defer_drop_mut!`].
///
/// # Limitation
///
/// The macro rebinds `$heap` as a new `let` binding, so it cannot be used when `$heap`
/// is `self`. In `&mut self` methods, first assign `let this = self;` and pass `this`.
#[macro_export]
macro_rules! defer_drop {
    ($value:ident, $heap:ident) => {
        let mut _guard = $crate::heap::HeapGuard::new($value, $heap);
        #[allow(
            clippy::allow_attributes,
            reason = "the reborrowed parts may not both be used in every case, so allow unused vars to avoid warnings"
        )]
        #[allow(unused_variables)]
        let ($value, $heap) = _guard.as_parts();
    };
}

/// Like [`defer_drop!`], but rebinds `$value` as `&mut V` via [`HeapGuard::as_parts_mut`].
///
/// Use this when the value needs to be mutated in place — for example, advancing an
/// iterator with `for_next()`, or swapping values during a min/max comparison.
#[macro_export]
macro_rules! defer_drop_mut {
    ($value:ident, $heap:ident) => {
        let mut _guard = $crate::heap::HeapGuard::new($value, $heap);
        #[allow(
            clippy::allow_attributes,
            reason = "the reborrowed parts may not both be used in every case, so allow unused vars to avoid warnings"
        )]
        #[allow(unused_variables)]
        let ($value, $heap) = _guard.as_parts_mut();
    };
}

/// Like [`defer_drop!`], but for [`DropWithImmutableHeap`] values that only need `&Heap`
/// for cleanup.
///
/// Creates an [`ImmutableHeapGuard`] and immediately rebinds `$value` as `&V` and `$heap`
/// as `&Heap<T>`. The guard will call [`DropWithImmutableHeap::drop_with_immutable_heap`]
/// when scope exits. Use this for values like [`RecursionToken`] in contexts that only have
/// shared access to the heap (e.g., `py_repr_fmt` formatting methods).
#[macro_export]
macro_rules! defer_drop_immutable_heap {
    ($value:ident, $heap:ident) => {
        let _guard = $crate::heap::ImmutableHeapGuard::new($value, $heap);
        #[allow(
            clippy::allow_attributes,
            reason = "the reborrowed parts may not both be used in every case, so allow unused vars to avoid warnings"
        )]
        #[allow(unused_variables)]
        let ($value, $heap) = _guard.as_parts();
    };
}
