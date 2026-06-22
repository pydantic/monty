//! Async/await support types for Monty.
//!
//! This module contains all async-related types including coroutines, futures,
//! and task identifiers. The host acts as the event loop - external function
//! calls return `ExternalFuture` objects that can be awaited.

use std::mem;

use ahash::AHashMap;
use smallvec::SmallVec;

use crate::{
    exception_private::RunError,
    heap::{ContainsHeap, DropWithHeap, HeapId},
    intern::FunctionId,
    value::Value,
};

/// Rough byte cost of the dynamically-allocated `Awaited` bookkeeping for a
/// gather with the given pending children and result slots. Authoritative
/// for the tracker charge taken at `Pending → Awaited` *and* for
/// `GatherFuture::py_estimate_size`; keeping the formula in one place
/// prevents the charge and the eventual shrink from silently drifting apart.
///
/// Per-entry sizing matches conventions elsewhere in the codebase: the
/// HashMap's bucket overhead is elided, and only `SmallVec` entries spilled
/// beyond the inline buffer (size 1) are counted.
pub(crate) fn awaited_state_size(
    pending_children: &AHashMap<HeapId, SmallVec<[usize; 1]>>,
    results: &[Option<Value>],
) -> usize {
    let pending_size: usize = pending_children
        .values()
        .map(|slots| {
            let spilled = slots.len().saturating_sub(1);
            mem::size_of::<HeapId>() + spilled * mem::size_of::<usize>()
        })
        .sum();
    mem::size_of_val(results) + pending_size
}

/// Unique identifier for external function calls.
///
/// Sequential integers allocated by the scheduler. Used to correlate
/// external function calls with their results when the host resolves them.
/// The counter always increments, even for sync resolution, to keep IDs unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct CallId(u32);

impl CallId {
    /// Creates a new CallId from a raw value.
    #[inline]
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw u32 value.
    #[inline]
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Unique identifier for an async task.
///
/// Sequential integers allocated by the scheduler. Task 0 is always the main task
/// which uses the VM's stack/frames directly. Spawned tasks (1+) store their own context,
/// hence `TaskId::default()` is the main task.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskId(u32);

impl TaskId {
    /// Creates a new TaskId from a raw value.
    #[inline]
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns true if this is the main task (task 0).
    #[inline]
    pub fn is_main(self) -> bool {
        self.0 == 0
    }
}

/// Coroutine execution state (single-shot semantics).
///
/// Coroutines in Monty follow single-shot semantics - they can only be awaited once.
/// This differs from Python generators which can be resumed multiple times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum CoroutineState {
    /// Coroutine has been created but not yet awaited.
    New,
    /// Coroutine is currently executing (has been awaited).
    Running,
    /// Coroutine has finished execution.
    Completed,
}

/// A coroutine object representing an async function call result.
///
/// Created when an `async def` function is called. Argument binding happens at call time;
/// awaiting the coroutine starts execution. Coroutines use single-shot semantics -
/// they can only be awaited once.
///
/// # Namespace Layout
///
/// The `namespace` vector is pre-sized to match the function's namespace size and contains:
/// ```text
/// [params...][cell_vars...][free_vars...][locals...]
/// ```
/// - Parameter slots are filled with bound argument values at call time
/// - Cell/free var slots contain `Value::Ref` to captured cells
/// - Local slots start as `Value::Undefined`
///
/// When the coroutine is awaited, these values are pushed onto the VM's stack
/// as inline locals, and a new frame is pushed to execute the async function body.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Coroutine {
    /// The async function to execute.
    pub func_id: FunctionId,
    /// Pre-bound namespace values (sized to function namespace).
    /// Contains bound parameters, captured cells, and uninitialized locals.
    pub namespace: Vec<Value>,
    /// Current execution state.
    pub state: CoroutineState,
}
impl Coroutine {
    /// Creates a new coroutine for an async function call.
    ///
    /// # Arguments
    /// * `func_id` - The async function to execute
    /// * `namespace` - Pre-bound namespace with parameters and captured variables
    pub fn new(func_id: FunctionId, namespace: Vec<Value>) -> Self {
        Self {
            func_id,
            namespace,
            state: CoroutineState::New,
        }
    }
}

/// An external future driven by the host.
///
/// Created when the host returns `ExtFunctionResult::Future(call_id)` in
/// response to a function call yield. The future starts in `Pending`, and
/// transitions to `Resolved` (host returned a value) or `Failed` (host
/// returned an error) when [`VM::resolve_future`] / [`VM::fail_future`]
/// fires.
///
/// # Re-await semantics
///
/// `Resolved` / `Failed` futures can be awaited any number of times — each
/// await yields a clone of the cached value or replays the cached exception,
/// matching CPython's Future semantics. `Pending` futures still support only
/// a single in-flight awaiter (the `awaiter: Option<Awaiter>` slot);
/// multi-awaiter on `Pending` is a planned follow-up that needs the same
/// wake/raise plumbing as multi-waiter gathers.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ExternalFuture {
    /// Host-side identifier. Used by the scheduler's `CallId -> HeapId`
    /// index so host resolutions can find the heap entry.
    pub call_id: CallId,
    /// Current state.
    pub state: ExternalFutureState,
}

/// State machine for [`ExternalFuture`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum ExternalFutureState {
    /// Awaiting host resolution. `awaiter` is the downstream that should be
    /// notified on resolution; `None` until someone awaits, `Some(...)` for
    /// the lifetime of the await.
    Pending { awaiter: Option<Awaiter> },
    /// Resolved with a value. Re-awaits clone this Value.
    Resolved(Value),
    /// Rejected with an error. Re-awaits replay (clone) this error.
    Failed(RunError),
}

impl ExternalFuture {
    /// Creates a new `ExternalFuture` in the `Pending` state with no awaiter.
    pub fn new_pending(call_id: CallId) -> Self {
        Self {
            call_id,
            state: ExternalFutureState::Pending { awaiter: None },
        }
    }
}

/// Where the result/error of a completing awaitable should be routed.
///
/// Stored as the "downstream" of an awaitable.
///
/// - `Task` wakes the named task by setting it `Ready` and pushing the value
///   (or routing the error through its frame's exception handler). `TaskId`
///   is just a scheduler-side identifier, not a heap reference, so the
///   variant owns no inc_ref.
/// - `GatherSlot` fans the value into the gather at `gather`, looking up the
///   slot indices it should fill via the awaitable's own `HeapId` (`source`).
///   The wrapper **owns an inc_ref on `gather`**: storing the awaiter on an
///   awaitable keeps the gather alive for the in-flight window, so the
///   awaitable's resolution path can dispatch to it safely without
///   additional cleanup elsewhere. Drop the owned `Awaiter` via
///   [`DropWithHeap`] (and clone via [`Self::clone_with_heap`]) so the
///   `gather` ref count stays balanced.
///
/// Not `Copy` / `Clone` on purpose — the inc_ref discipline requires every
/// duplication to go through `clone_with_heap` and every discard to go
/// through `drop_with_heap`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Awaiter {
    Task(TaskId),
    GatherSlot { gather: HeapId, source: HeapId },
}

impl DropWithHeap for Awaiter {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        if let Self::GatherSlot { gather, .. } = self {
            heap.heap_mut().dec_ref(gather);
        }
    }
}

/// A gather() result tracking multiple coroutines/tasks and external futures.
///
/// Created by `asyncio.gather(*awaitables)`. Does NOT spawn tasks immediately -
/// tasks are spawned when the GatherFuture is awaited in Await.
///
/// # Lifecycle
///
/// The lifecycle is encoded in [`GatherState`]:
///
/// 1. **`Pending`** — created by `gather(coro1, coro2, ...)` but not yet awaited.
///    Only `items` carries data; the per-await bookkeeping does not yet exist.
/// 2. **`Awaited(AwaitedGather)`** — entered by the `Await` opcode. Spawned task
///    ids, the waiter, the per-slot results, and any external futures still
///    being waited on all live inside the [`AwaitedGather`] payload. Tasks and
///    external resolutions write into `results` slots while in this state.
/// 3. **`Completed(list_id)`** — all children completed successfully. The
///    `list_id` is an inc_ref'd `HeapData::List` holding the gathered results;
///    re-awaiting the gather returns this same list, matching CPython's
///    behavior of caching a Future's result.
/// 4. **`Failed(error)`** — a child task or external future raised. The error
///    was propagated to the original waiter on first await, and is cached here
///    so re-awaits re-raise the same exception (again matching CPython).
///
/// Encoding the phases as a `match`-able enum lets every site that touches a
/// gather state-transition explicitly, instead of inferring "have we been
/// awaited?" / "are we done?" from emptiness checks across several `Vec`s.
///
/// # Re-await semantics
///
/// `Completed` and `Failed` gathers can be awaited any number of times — each
/// await yields the same cached result or exception. Re-awaiting a gather that
/// is still in `Awaited` state (in-flight, the original waiter has not finished
/// driving it to completion) is currently rejected; supporting that would
/// require a list of waiters and is left as future work.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct GatherFuture {
    /// Heap ids of items to gather. Each id points to an awaitable
    /// `HeapData` entry (currently `Coroutine` or `ExternalFuture`); the
    /// kind is recovered with `heap.read(id)` at gather-await time.
    ///
    /// Set once at construction and never mutated. The gather inc_refs each
    /// id and is the owner until drop, so GC must always walk this vector
    /// regardless of `state`.
    pub items: Vec<HeapId>,
    /// Phase of the gather lifecycle. See [`GatherState`].
    pub state: GatherState,
}

/// Lifecycle phase of a [`GatherFuture`].
///
/// See the `GatherFuture` docs for the transition rules.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum GatherState {
    /// Created but never awaited. No spawned tasks, no results, no waiter.
    Pending,
    /// Currently being awaited. `AwaitedGather` carries every per-await field.
    Awaited(AwaitedGather),
    /// All children completed successfully. Re-awaiting the gather inc_refs this
    /// value and returns it.
    Completed(Value),
    /// A child task failed (or an external future was rejected). The error
    /// is cached so subsequent awaits re-raise it. `RunError` implements
    /// `Clone`; clone the error when transitioning into this state.
    Failed(RunError),
}

/// Per-await bookkeeping for a [`GatherFuture`] in the `Awaited` phase.
///
/// All fields are populated when the gather is first awaited (in
/// `await_gather_future`) and progressively consumed as children resolve.
///
/// The gather is the single source of truth for "what awaitables I'm waiting
/// on and where their values go". Both maps follow the same lifecycle: each
/// entry is removed as the corresponding child resolves, and the gather is
/// done when both maps are empty.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct AwaitedGather {
    /// Downstream the gather should notify on completion. `Awaiter::Task(t)`
    /// is the user-level `await gather` case (waker is the task that ran
    /// the `await`); `Awaiter::GatherSlot { .. }` is the nested-gather case
    /// (this gather is an item of an outer gather, and its result list
    /// fans into the outer's slot). Single-waiter for now; the multi-waiter
    /// form is a planned follow-up.
    pub awaiter: Awaiter,
    /// Children → slots they fill in `results`. Keyed by each child's own
    /// `HeapId` (coroutine or external future). Duplicates from
    /// `gather(c, c)` produce a single entry whose value is a `SmallVec` of
    /// multiple indices.
    ///
    /// Entries are removed as the corresponding child resolves. The gather
    /// is done when this map is empty.
    pub pending_children: AHashMap<HeapId, SmallVec<[usize; 1]>>,
    /// Results from each gather item, in order. Indices align with
    /// `GatherFuture::items`. Filled as tasks complete and externals resolve.
    pub results: Vec<Option<Value>>,
}

impl GatherFuture {
    /// Creates a new GatherFuture with the given item heap ids.
    ///
    /// # Arguments
    /// * `items` - Heap ids of awaitables (coroutines or external futures)
    pub fn new(items: Vec<HeapId>) -> Self {
        Self {
            items,
            state: GatherState::Pending,
        }
    }

    /// Returns the number of items to gather.
    #[inline]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Returns the per-await bookkeeping if the gather is in the `Awaited`
    /// phase. Convenience for read-only inspection sites that don't need to
    /// distinguish the other phases from one another.
    #[inline]
    pub fn as_awaited(&self) -> Option<&AwaitedGather> {
        match &self.state {
            GatherState::Awaited(awaited) => Some(awaited),
            GatherState::Pending | GatherState::Completed(_) | GatherState::Failed(_) => None,
        }
    }

    /// Mutable counterpart to [`Self::as_awaited`].
    #[inline]
    pub fn as_awaited_mut(&mut self) -> Option<&mut AwaitedGather> {
        match &mut self.state {
            GatherState::Awaited(awaited) => Some(awaited),
            GatherState::Pending | GatherState::Completed(_) | GatherState::Failed(_) => None,
        }
    }
}
