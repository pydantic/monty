//! Async/await support types for Monty.
//!
//! This module contains all async-related types including coroutines, futures,
//! and task identifiers. The host acts as the event loop - external function
//! calls return `ExternalFuture` objects that can be awaited.

use ahash::AHashMap;

use crate::{exception_private::RunError, heap::HeapId, intern::FunctionId, value::Value};

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

/// An item that can be gathered - either a coroutine or an external future.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum GatherItem {
    /// A coroutine to spawn as a task.
    Coroutine(HeapId),
    /// An external future to wait for resolution.
    ExternalFuture(CallId),
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
    /// Items to gather (coroutines or external futures).
    ///
    /// Set once at construction and never mutated. The gather inc_refs each
    /// unique `GatherItem::Coroutine` HeapId and is the owner until drop, so
    /// GC must always walk this vector regardless of `state`.
    pub items: Vec<GatherItem>,
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
    /// All children completed successfully. The contained `HeapId` is an
    /// inc_ref'd `HeapData::List` of results; the gather is its owner and
    /// dec_refs it on drop. Re-awaiting the gather inc_refs this list and
    /// returns it.
    Completed(HeapId),
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
    /// Task that called `await` on the enclosing gather. Always present in this
    /// phase — there is no "awaited but no waiter" sub-state.
    pub waiter: TaskId,
    /// Spawned tasks → slots they fill in `results`. One entry per *unique*
    /// coroutine in `items` (duplicates collapse to a single task with
    /// multiple slot indices). Entries are removed as the corresponding
    /// task completes; the task is then immediately cancelled from the
    /// scheduler so refcounts get released eagerly.
    pub pending_tasks: AHashMap<TaskId, Vec<usize>>,
    /// External futures → slots they fill in `results`. Entries are removed
    /// as `resolve_future` resolves each call.
    pub pending_calls: AHashMap<CallId, Vec<usize>>,
    /// Results from each gather item, in order. Indices align with
    /// `GatherFuture::items`. Filled as tasks complete and externals resolve.
    pub results: Vec<Option<Value>>,
}

impl GatherFuture {
    /// Creates a new GatherFuture with the given items.
    ///
    /// # Arguments
    /// * `items` - Coroutines or external futures to run concurrently
    pub fn new(items: Vec<GatherItem>) -> Self {
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
