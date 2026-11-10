//! Owned values that release their heap reference on a plain Rust `Drop`.
//!
//! A [`Value`] is only a `HeapId` for heap objects, so a plain `Drop` cannot
//! decrement its refcount: every owner must call `drop_with` on every path,
//! including each `?`. [`Owned`] removes that obligation for Rust locals:
//! dropping it pushes the `HeapId` onto the current VM's [`ReleaseQueue`],
//! which the VM applies where it holds the heap mutably: when each call returns
//! (so frees are visible when they were with `drop_with`), every 64 steps of a
//! native iteration loop, at dispatch checkpoints, GC, turn end, snapshot and
//! VM drop.
//!
//! `Owned` finds the queue through a thread-local rather than a handle in each
//! value, so it stays the size of `Value` and costs nothing until a heap value
//! is dropped. The VM holds its own `Rc` to the queue, so its frequent
//! emptiness checks never touch thread-local storage. Host code can start
//! another run on the same thread from inside a live VM (a print callback), so
//! each VM installs its queue when created and reinstalls the enclosing one
//! when dropped ([`enter_vm`] / [`exit_vm`]); only the innermost VM runs native
//! code, so every queued ID belongs to its heap.
//!
//! Values stored in containers stay plain `Value`s; containers already release
//! their children through `py_dec_ref_ids`. Convert at the boundary with
//! [`Owned::new`] and [`Owned::into_value`].

use std::{
    cell::{Cell, RefCell},
    mem,
    ops::Deref,
    rc::Rc,
    vec,
};

use crate::{
    args::KwargsValues,
    bytecode::VM,
    heap::{ContainsHeap, HeapId},
    value::Value,
};

thread_local! {
    /// Queue of the innermost live VM on this thread, which dropped [`Owned`] values push to.
    static CURRENT: RefCell<Option<Rc<ReleaseQueue>>> = const { RefCell::new(None) };
}

/// Heap releases queued by dropped [`Owned`] values, applied later by one VM.
#[derive(Debug, Default)]
pub(crate) struct ReleaseQueue {
    pending: RefCell<Vec<HeapId>>,
    /// Set by every push so the VM's per-call check is a single load.
    dirty: Cell<bool>,
}

impl ReleaseQueue {
    /// Decrements the refcount of every queued ID.
    ///
    /// Deferring the decrement is always safe (the object lives slightly longer),
    /// but it must run before a snapshot or VM drop, and never while heap readers
    /// are active, since a release may free an entry.
    #[inline]
    pub fn apply(&self, heap: &mut impl ContainsHeap) {
        if self.dirty.get() {
            self.apply_pending(heap);
        }
    }

    #[cold]
    fn apply_pending(&self, heap: &mut impl ContainsHeap) {
        self.dirty.set(false);
        // Freeing never creates `Owned` values, so nothing is queued while this runs;
        // the buffer is swapped out and back only to keep the borrow short.
        let mut pending = mem::take(&mut *self.pending.borrow_mut());
        let heap = heap.heap_mut();
        for id in pending.drain(..) {
            heap.dec_ref(id);
        }
        // Hand the allocation back so steady-state releases do not reallocate.
        *self.pending.borrow_mut() = pending;
    }

    fn push(&self, id: HeapId) {
        self.pending.borrow_mut().push(id);
        self.dirty.set(true);
    }
}

impl Drop for ReleaseQueue {
    fn drop(&mut self) {
        debug_assert!(
            self.pending.get_mut().is_empty(),
            "ReleaseQueue dropped with unapplied releases"
        );
    }
}

/// Installs a fresh queue for a new VM; returns it and the enclosing VM's queue, for [`exit_vm`].
pub(crate) fn enter_vm() -> (Rc<ReleaseQueue>, Option<Rc<ReleaseQueue>>) {
    let queue = Rc::new(ReleaseQueue::default());
    let outer = CURRENT.with_borrow_mut(|current| current.replace(Rc::clone(&queue)));
    (queue, outer)
}

/// Reinstalls the enclosing VM's queue; called when a VM is dropped, after applying its own.
///
/// VMs must be dropped in reverse creation order; otherwise the wrong queue is
/// reinstalled and later releases leak, which debug builds catch here.
pub(crate) fn exit_vm(own: &Rc<ReleaseQueue>, outer: Option<Rc<ReleaseQueue>>) {
    CURRENT.with_borrow_mut(|current| {
        debug_assert!(
            current.as_ref().is_some_and(|current| Rc::ptr_eq(current, own)),
            "VM dropped while a VM created after it is still live"
        );
        *current = outer;
    });
}

/// Queues `id` for release by the innermost live VM.
///
/// `Owned` values only exist inside native code, which only runs under a VM.
#[inline]
fn queue_release(id: HeapId) {
    with_current_queue(|queue| queue.push(id));
}

/// Queues every heap reference in `values` with one thread-local access, for owning containers.
fn queue_values(values: impl IntoIterator<Item = Value>) {
    with_current_queue(|queue| {
        let mut pending = queue.pending.borrow_mut();
        for value in values {
            if let Value::Ref(id) = value {
                pending.push(id);
                discard_released(value);
            }
        }
        queue.dirty.set(true);
    });
}

/// Runs `f` on the innermost live VM's queue; with no live VM the references leak.
#[inline]
fn with_current_queue(f: impl FnOnce(&ReleaseQueue)) {
    CURRENT.with_borrow(|current| match current {
        Some(queue) => f(queue),
        None => debug_assert!(false, "owned heap values dropped outside a VM; their references leak"),
    });
}

/// Discards a `Ref` whose release was already queued.
///
/// Under `memory-model-checks` it is forgotten so `Value`'s leak-detecting `Drop` does not fire.
#[inline]
#[cfg_attr(
    not(feature = "memory-model-checks"),
    expect(clippy::needless_pass_by_value, reason = "consumes only when `Value` has a `Drop`")
)]
fn discard_released(value: Value) {
    #[cfg(feature = "memory-model-checks")]
    mem::forget(value);
    #[cfg(not(feature = "memory-model-checks"))]
    let _ = value;
}

/// A [`Value`] owned by Rust code that releases its heap reference when dropped.
///
/// Use it for locals and arguments in native code: `?`, `_` patterns, drains
/// and overwrites all release correctly without guards. Do not store it in heap
/// containers; unwrap it with [`into_value`](Self::into_value) first.
#[derive(Debug)]
pub(crate) struct Owned(Value);

impl Owned {
    /// Takes ownership of `value` so that dropping it releases its heap reference.
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    /// Releases ownership as a plain `Value`, e.g. to return it or store it in a container.
    #[must_use]
    pub fn into_value(mut self) -> Value {
        // `Value::None` is immediate, so the `Drop` below has nothing to queue.
        mem::replace(&mut self.0, Value::None)
    }

    /// Releases the heap reference now instead of queueing it.
    ///
    /// Optional: dropping is always correct. Hot loops use it to skip the
    /// queue round-trip, which costs a few nanoseconds per heap value.
    #[inline]
    pub fn release(self, heap: &mut impl ContainsHeap) {
        if let Value::Ref(id) = self.0 {
            heap.heap_mut().dec_ref(id);
        }
        // Released (or immediate): skip `Drop`, which would queue the ID again.
        // Forgetting also skips the `memory-model-checks` `Drop` on `Value`.
        mem::forget(self);
    }
}

impl Drop for Owned {
    #[inline]
    fn drop(&mut self) {
        if let Value::Ref(id) = self.0 {
            queue_release(id);
            discard_released(mem::replace(&mut self.0, Value::None));
        }
    }
}

impl Deref for Owned {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

impl VM<'_> {
    /// Applies releases queued by dropped [`Owned`] values.
    #[inline]
    pub(crate) fn apply_releases(&mut self) {
        self.release_queue.apply(self.heap);
    }
}

/// Owning iterator over a `Vec<Value>` that yields [`Owned`] items.
///
/// Built with `OwnedIntoIter::from(vec)`, or declared as a `FromArgs` `varargs`
/// field. Unlike `Vec::drain` or `vec::IntoIter` over plain values, dropping it
/// part way (e.g. on `?`) releases the rest.
#[derive(Debug)]
pub(crate) struct OwnedIntoIter(vec::IntoIter<Value>);

impl From<Vec<Value>> for OwnedIntoIter {
    fn from(values: Vec<Value>) -> Self {
        Self(values.into_iter())
    }
}

impl Iterator for OwnedIntoIter {
    type Item = Owned;

    fn next(&mut self) -> Option<Owned> {
        self.0.next().map(Owned)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for OwnedIntoIter {}

impl Drop for OwnedIntoIter {
    fn drop(&mut self) {
        if self.0.len() > 0 {
            queue_values(self.0.by_ref());
        }
    }
}

/// Keyword arguments owned by Rust code; dropping them releases every key and value.
///
/// The [`KwargsValues`] counterpart of [`Owned`]: declare it as a `FromArgs`
/// `varkwargs` field, or wrap the kwargs from `ArgValues::into_parts`.
/// Use [`into_inner`](Self::into_inner) to pass them on.
#[derive(Debug)]
pub(crate) struct OwnedKwargs(KwargsValues);

impl OwnedKwargs {
    /// Releases ownership of the kwargs, e.g. to hand them to another parser.
    #[must_use]
    pub fn into_inner(mut self) -> KwargsValues {
        mem::replace(&mut self.0, KwargsValues::Empty)
    }
}

impl From<KwargsValues> for OwnedKwargs {
    fn from(kwargs: KwargsValues) -> Self {
        Self(kwargs)
    }
}

impl Deref for OwnedKwargs {
    type Target = KwargsValues;

    fn deref(&self) -> &KwargsValues {
        &self.0
    }
}

impl Drop for OwnedKwargs {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            let kwargs = mem::replace(&mut self.0, KwargsValues::Empty);
            queue_values(kwargs.into_iter().flat_map(|(key, value)| [key, value]));
        }
    }
}

#[cfg(test)]
mod tests {
    use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

    use crate::{
        bytecode::VM,
        heap::{Heap, HeapReader},
        run::Executor,
    };

    /// The thread-local queue is a stack, so dropping the older of two live VMs first is caught.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "VM dropped while a VM created after it is still live")]
    fn dropping_vms_out_of_order_is_caught() {
        let executor = || Executor::new("1".to_owned(), "test.py", Vec::new(), CompileOptions::default()).unwrap();
        let (mut first_executor, mut second_executor) = (executor(), executor());
        let mut first_heap = Heap::new(16, ResourceTracker::default());
        let mut second_heap = Heap::new(16, ResourceTracker::default());
        HeapReader::with(&mut first_heap, &mut first_executor, |reader, executor| {
            let first = VM::new(
                Vec::new(),
                &mut executor.tables,
                &executor.program,
                reader,
                PrintWriter::Disabled,
            );
            HeapReader::with(&mut second_heap, &mut second_executor, |reader, executor| {
                let _second = VM::new(
                    Vec::new(),
                    &mut executor.tables,
                    &executor.program,
                    reader,
                    PrintWriter::Disabled,
                );
                drop(first);
            });
        });
    }
}
