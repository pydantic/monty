//! Async/await support types for Monty.
//!
//! This module contains all async-related types including coroutines, futures,
//! and task identifiers. The host acts as the event loop - external function
//! calls return `ExternalFuture` objects that can be awaited.

use ahash::{AHashMap, AHashSet};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smallvec::SmallVec;

use crate::{
    exception_private::RunError,
    heap::{ContainsHeap, DropWithContext, HeapId},
    intern::FunctionId,
    value::Value,
};

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
    /// Owned reference to the `exec()` / `eval()` globals dict the async
    /// function was defined under; `None` when its globals are module slots.
    pub globals: Option<HeapId>,
}
impl Coroutine {
    /// Creates a new coroutine for an async function call.
    ///
    /// # Arguments
    /// * `func_id` - The async function to execute
    /// * `namespace` - Pre-bound namespace with parameters and captured variables
    /// * `globals` - The function's globals dict, already inc_ref'd for this coroutine
    pub fn new(func_id: FunctionId, namespace: Vec<Value>, globals: Option<HeapId>) -> Self {
        Self {
            func_id,
            namespace,
            state: CoroutineState::New,
            globals,
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
    /// `asyncio.sleep(delay, result)`: the value to resolve with in place of
    /// the host's, which is only the wake-up signal. Owned; taken on
    /// resolution and released on failure or when the entry is freed.
    pub sleep_result: Option<Value>,
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
    pub fn new_pending(call_id: CallId, sleep_result: Option<Value>) -> Self {
        Self {
            call_id,
            state: ExternalFutureState::Pending { awaiter: None },
            sleep_result,
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
///   additional cleanup elsewhere. Discard the owned `Awaiter` via
///   [`DropWithContext`], which releases exactly the reference
///   [`Self::heap_ref`] reports.
/// - `WaitSlot` is the `wait` analogue of `GatherSlot`: it owns an inc_ref
///   on `wait` and uses `source`, a borrowed child id, to mark that child
///   done.
/// - `UserTask` completes the Python-visible `Task` at `task`, owning an
///   inc_ref on it; it is stored on the future the task wraps.
///
/// `source` is never an owned reference in either slot variant: the gather's
/// items, the wait's inputs, or the wait's timer own the child entry.
///
/// An awaitable whose downstream has already settled keeps its `Awaiter`
/// pointing at it; the value is discarded where it is delivered rather than
/// the link being severed here. See [`GatherState`].
///
/// Not `Copy` / `Clone` on purpose — the inc_ref discipline requires every
/// duplication to go through `clone_with_heap` and every discard to go
/// through `drop_with`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Awaiter {
    Task(TaskId),
    GatherSlot {
        gather: HeapId,
        source: HeapId,
    },
    /// A child settled inside `wait(wait)`: record it on the wait future.
    WaitSlot {
        wait: HeapId,
        source: HeapId,
    },
    /// An adopted host future settled: complete this heap task and fan its
    /// result out to the task's own awaiters.
    UserTask {
        task: HeapId,
    },
}

impl Awaiter {
    /// The heap reference this awaiter owns, if any.
    ///
    /// Every heap-owning variant inc_refs its target when constructed; GC and
    /// `dec_ref` use this to release it without matching each variant by hand.
    #[inline]
    pub fn heap_ref(&self) -> Option<HeapId> {
        match self {
            Self::Task(_) => None,
            Self::GatherSlot { gather: id, .. } | Self::WaitSlot { wait: id, .. } => Some(*id),
            Self::UserTask { task } => Some(*task),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Awaiter {
    fn drop_with(self, heap: &mut C) {
        if let Some(id) = self.heap_ref() {
            heap.heap_mut().dec_ref(id);
        }
    }
}

/// A user-created `asyncio.Task` wrapping a coroutine or a host future.
///
/// The scheduler owns the execution context of a coroutine-backed task; this
/// heap entry is the Python-visible handle and the completion record that
/// `await`, `result()`, `exception()` and `wait()` read. `task_id` is `None`
/// for an adopted host future, which the host already drives and which needs
/// no scheduler task.
///
/// A host-adopted task is completed by [`crate::asyncio::Awaiter::UserTask`]
/// when its future resolves. A coroutine-backed task is completed by
/// `VM::handle_task_completion` / `VM::handle_task_failure`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskFuture {
    /// Scheduler task driving the coroutine, if this is not a host future.
    pub task_id: Option<TaskId>,
    /// Owned reference to the wrapped coroutine, external future or wait
    /// future. Kept so `get_coro()` still works after the scheduler releases
    /// its own reference.
    pub coroutine_id: HeapId,
    /// Display name used by `get_name`, `set_name` and `repr`.
    pub name: String,
    /// Completion record. `Pending` until the scheduler or host resolves it.
    pub state: TaskFutureState,
    /// Downstreams awaiting this task. Owned; each heap-owning awaiter
    /// (`GatherSlot` or `WaitSlot`) holds an inc_ref on its target. A task
    /// may have several awaiters.
    pub awaiters: Vec<Awaiter>,
}

/// Completion state of a [`TaskFuture`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum TaskFutureState {
    /// The wrapped work has not completed.
    Pending,
    /// Completed with this cached result. Re-awaits clone it.
    Completed(Value),
    /// Failed with this cached exception. Re-awaits replay it.
    Failed(RunError),
}

impl TaskFuture {
    /// Creates a pending task handle around `coroutine_id`, taking ownership
    /// of that heap reference and naming the task `name`.
    pub fn new(task_id: Option<TaskId>, coroutine_id: HeapId, name: String) -> Self {
        Self {
            task_id,
            coroutine_id,
            name,
            state: TaskFutureState::Pending,
            awaiters: Vec::new(),
        }
    }

    /// Whether the task has completed, successfully or not.
    #[inline]
    pub fn is_done(&self) -> bool {
        !matches!(self.state, TaskFutureState::Pending)
    }
}

/// The result of an `asyncio.wait()` call, before and after its first await.
///
/// CPython implements `wait` as a coroutine, so calling it starts nothing and
/// awaiting it twice raises `cannot reuse already awaited coroutine`. This
/// heap object mirrors that shape: `Unawaited` owns the argument and option
/// values, `Pending` owns the per-child bookkeeping, and `Consumed` makes a
/// second await fail.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct WaitFuture {
    /// Lifecycle state. See [`WaitFutureState`].
    pub state: WaitFutureState,
}

/// Lifecycle of a [`WaitFuture`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum WaitFutureState {
    /// Created by `asyncio.wait(...)`, not yet awaited.
    Unawaited(UnawaitedWait),
    /// Awaited and waiting for children or its timeout timer.
    Pending(WaitPending),
    /// Its one allowed await has returned; a second await raises.
    Consumed,
}

/// Arguments held by a [`WaitFuture`] until its first await.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct UnawaitedWait {
    /// The caller's `fs` iterable, owned until the first await materialises it.
    pub fs: Value,
    /// `return_when`, validated when the future is first awaited. `None`
    /// means the argument was omitted and defaults to `ALL_COMPLETED`.
    pub return_when: Option<Value>,
    /// Timeout sleep awaitable for a positive timeout, owned until completion.
    /// A settled one means the timeout already elapsed.
    pub timer: Option<HeapId>,
    /// A timeout that elapsed before a timer awaitable existed: `timeout` is
    /// NaN or `<= 0`, or the sleep policy settles immediately. A settled
    /// `timer` is checked at first await.
    pub timed_out: bool,
}

/// Per-await bookkeeping for an [`WaitFuture`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct WaitPending {
    /// Every deduplicated input, owned. Partitioned into the result sets on
    /// completion, so this is also the source of the result objects.
    pub inputs: Vec<HeapId>,
    /// Inputs that have not settled yet. Borrowed from `inputs`.
    #[serde(
        serialize_with = "serialize_heap_id_set",
        deserialize_with = "deserialize_heap_id_set"
    )]
    pub pending: AHashSet<HeapId>,
    /// Inputs that have settled, in resolution order. Borrowed from `inputs`.
    pub done: Vec<HeapId>,
    /// Whether any input failed. Only relevant to `FIRST_EXCEPTION`.
    pub failed_any: bool,
    /// Completion condition supplied to `wait`.
    pub return_when: WaitCondition,
    /// The task or gather that receives the completed `(done, pending)` tuple.
    pub awaiter: Option<Awaiter>,
    /// Timeout sleep awaitable while it can still fire, owned until completion.
    pub timer: Option<HeapId>,
    /// Whether the timer fired before every child settled.
    pub timed_out: bool,
}

/// `asyncio.wait` completion conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum WaitCondition {
    AllCompleted,
    FirstCompleted,
    FirstException,
}

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
    /// `HeapData` entry (`Coroutine`, `ExternalFuture`, `GatherFuture`,
    /// `WaitFuture` or `TaskFuture`); the kind is recovered with
    /// `heap.read(id)` at gather-await time.
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

/// Children of a gather that are still in flight → the slots each fills in the
/// gather's `results`.
///
/// Keyed by the child's own `HeapId` (coroutine, external future or nested
/// gather), which is how every resolution site addresses its parent's slots.
/// Duplicates from `gather(c, c)` share one entry with several indices.
pub(crate) type PendingChildren = AHashMap<HeapId, SmallVec<[usize; 1]>>;

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
    /// Children → slots they fill in `results`, see [`PendingChildren`].
    ///
    /// Entries are removed as the corresponding child resolves. The gather
    /// is done when this map is empty.
    pub pending_children: PendingChildren,
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

/// Serializes a heap-id set in id order so a dump is stable across a reload.
fn serialize_heap_id_set<S>(set: &AHashSet<HeapId>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut ids: Vec<HeapId> = set.iter().copied().collect();
    ids.sort_unstable_by_key(|id| id.index());
    ids.serialize(serializer)
}

/// Rebuilds a heap-id set from the id-ordered sequence written by `serialize_heap_id_set`.
fn deserialize_heap_id_set<'de, D>(deserializer: D) -> Result<AHashSet<HeapId>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Vec::<HeapId>::deserialize(deserializer)?.into_iter().collect())
}
