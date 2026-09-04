//! Task scheduler for async execution and call ID allocation.
//!
//! # Task Model
//!
//! The current task's context lives in the VM; all other contexts live in their tasks.
//! Task 0 is the main task. Spawned tasks (1+) deliver results to their gather.

use std::{collections::VecDeque, mem};

use ahash::AHashMap;
use smallvec::{SmallVec, smallvec};

use super::FrameNamespace;
use crate::{
    asyncio::{Awaiter, CallId, TaskId},
    exception_private::RunResult,
    heap::{ContainsHeap, DropWithContext, Heap, HeapId, HeapReadOutput, HeapReader},
    intern::FunctionId,
    types::lru_cache::CacheStore,
    value::Value,
};

/// Live tasks are runnable or blocked; completion removes the task from the scheduler.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum TaskState {
    /// Queued or currently executing. `Err` must be raised before executing more bytecode.
    Ready(RunResult<()>),
    /// Owns an inc_ref on the awaitable; its awaiter identifies the task to wake.
    Blocked(HeapId),
}

impl<C: ContainsHeap> DropWithContext<C> for TaskState {
    fn drop_with(self, heap: &mut C) {
        match self {
            Self::Ready(_) => {}
            Self::Blocked(id) => heap.heap_mut().dec_ref(id),
        }
    }
}

/// A live async task. Its execution context is saved here while another task runs.
/// The current task's frames and stacks live in the VM, including for the main task.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Task {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Saved frames; empty while this task is current or has not started.
    pub frames: Vec<SerializedTaskFrame>,
    /// Saved operand stack; the current task uses the VM's stack.
    pub stack: Vec<Value>,
    /// Exception stack for nested except blocks.
    pub exception_stack: Vec<Value>,
    /// VM-level instruction_ip (for exception table lookup).
    pub instruction_ip: usize,
    /// Owned coroutine reference for a spawned task; the main task has none.
    pub coroutine_id: Option<HeapId>,
    /// Result recipient, owning an inc_ref for a gather. An already-settled gather
    /// discards the result. The main task has no awaiter.
    pub awaiter: Option<Awaiter>,
    /// Current execution state.
    state: TaskState,
}

impl<C: ContainsHeap> DropWithContext<C> for Task {
    fn drop_with(mut self, heap: &mut C) {
        self.stack.drain(..).drop_with(heap);
        self.exception_stack.drain(..).drop_with(heap);
        // A frame parked mid-way through a cached call still owns its pending
        // stores; abandoning the task drops them without storing anything.
        for frame in self.frames.drain(..) {
            frame.namespace.drop_with(heap);
            frame.cache_stores.drop_with(heap);
        }
        self.state.drop_with(heap);
        if let Some(coro_id) = self.coroutine_id.take() {
            heap.heap_mut().dec_ref(coro_id);
        }
        if let Some(awaiter) = self.awaiter.take() {
            awaiter.drop_with(heap);
        }
    }
}

/// Serialized call frame for task storage.
///
/// Similar to `SerializedFrame` but used within the scheduler for task context.
/// Cannot store `&Code` references - uses `FunctionId` to look up code on resume.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SerializedTaskFrame {
    /// Which function's code this frame executes (None = module-level).
    pub function_id: Option<FunctionId>,
    /// Instruction pointer within this frame's bytecode.
    pub ip: usize,
    /// Base index into the VM stack for this frame's locals region.
    pub stack_base: usize,
    /// Number of local variable slots (0 for module-level frames).
    pub locals_count: u16,
    /// Base index into the VM-wide `exception_stack` for this frame.
    /// See `CallFrame.exception_stack_base`.
    pub exception_stack_base: usize,
    /// Caller's bytecode offset at the call site (for tracebacks). See
    /// `CallFrame.call_offset`.
    pub call_offset: Option<u32>,
    /// Whether this frame is a class `__init__` (see `CallFrame.is_initializer`).
    #[serde(default)]
    pub is_initializer: bool,
    /// Frame namespace, owning its dict references (see `CallFrame.namespace`).
    pub namespace: Option<Box<FrameNamespace>>,
    /// The pending `functools.lru_cache` stores of this frame's cached calls
    /// (see `CallFrame.cache_stores`).
    #[serde(default)]
    pub cache_stores: Vec<CacheStore>,
}

impl Task {
    /// Creates a runnable task, taking ownership of its coroutine and awaiter references.
    pub fn new(id: TaskId, coroutine_id: Option<HeapId>, awaiter: Option<Awaiter>) -> Self {
        Self {
            id,
            frames: Vec::new(),
            stack: Vec::new(),
            exception_stack: Vec::new(),
            instruction_ip: 0,
            coroutine_id,
            awaiter,
            state: TaskState::Ready(Ok(())),
        }
    }
}

/// Owns live tasks, pending external futures, and call IDs for both sync and async execution.
/// A blocked task is queued once when its awaitable settles, for either a value or an error.
/// Selecting that task consumes its queue entry and resume result before bytecode can run.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Scheduler {
    /// All tasks keyed by their `TaskId`.
    tasks: AHashMap<TaskId, Task>,
    /// Tasks ready to execute or raise a pending exception.
    ready_queue: VecDeque<TaskId>,
    /// Task whose context is in the VM, or `None` when that context has been discarded.
    current_task: Option<TaskId>,
    /// Counter for generating new task IDs.
    next_task_id: u32,
    /// Counter for external call IDs (always incremented, even for sync resolution).
    next_call_id: u32,
    /// Host-side index mapping each unresolved external call to its
    /// `HeapData::ExternalFuture` entry. The scheduler holds an inc_ref on
    /// each value until the host resolves or fails the call — that ref keeps
    /// the future entry alive between yield and resume even if no awaiter is
    /// holding a `Value::Ref` to it.
    pending_externals: AHashMap<CallId, HeapId>,
    /// Index mapping a spawned coroutine's `HeapId` to the `TaskId` driving
    /// it. Populated in [`Scheduler::spawn`] and removed in
    /// [`Scheduler::cancel_task`]. Lets `GatherFuture` and other call sites
    /// dispatch from coroutine heap id back to the driving task without
    /// scanning all tasks.
    coroutine_to_task: AHashMap<HeapId, TaskId>,
}

impl Scheduler {
    /// Creates the main task as current; it needs no queue entry to start executing.
    pub fn new() -> Self {
        let main_task_id = TaskId::default();
        let main_task = Task::new(main_task_id, None, None);
        let mut tasks = AHashMap::new();
        tasks.insert(main_task_id, main_task);
        Self {
            tasks,
            ready_queue: VecDeque::new(), // Main task is current, not in ready queue
            current_task: Some(main_task_id),
            next_task_id: 1,
            next_call_id: 0,
            pending_externals: AHashMap::new(),
            coroutine_to_task: AHashMap::new(),
        }
    }

    /// Identifies the VM's loaded context, even when that task is blocked on a host call.
    #[inline]
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.current_task
    }

    /// Whether awaiting the current call can proceed without delaying other work.
    pub fn can_await_eagerly(&self) -> bool {
        self.ready_queue.is_empty() && self.pending_externals.is_empty()
    }

    /// Returns a mutable reference to a task by ID.
    ///
    /// # Panics
    /// Panics if the task ID doesn't exist.
    #[inline]
    pub fn get_task_mut(&mut self, task_id: TaskId) -> &mut Task {
        self.tasks
            .get_mut(&task_id)
            .expect("Scheduler::get_task_mut: task not found")
    }

    /// Allocates a new CallId for an external function call.
    ///
    /// The counter always increments, even for sync resolution, to keep IDs unique.
    pub fn allocate_call_id(&mut self) -> CallId {
        let id = CallId::new(self.next_call_id);
        self.next_call_id += 1;
        id
    }

    /// Registers a freshly created `ExternalFuture` for `call_id`.
    ///
    /// The scheduler inc_refs `future_id` so the entry stays alive between
    /// the yield to the host and the matching `resolve_future` / `fail_future`
    /// call, even if no awaiter holds a `Value::Ref` to it.
    pub fn add_pending_external(&mut self, call_id: CallId, future_id: HeapId, heap: &Heap) {
        heap.inc_ref(future_id);
        let prev = self.pending_externals.insert(call_id, future_id);
        debug_assert!(prev.is_none(), "add_pending_external: CallId already registered");
    }

    /// Removes and returns the `ExternalFuture` heap id for `call_id`, if any.
    ///
    /// The caller becomes responsible for the inc_ref previously held by the
    /// scheduler (typically dec_ref'd once the state transition is committed).
    pub fn take_pending_external(&mut self, call_id: CallId) -> Option<HeapId> {
        self.pending_externals.remove(&call_id)
    }

    /// Marks the current task as `Blocked` on the awaitable at `awaitable_id`.
    ///
    /// The task will be unblocked when the awaitable settles and its awaiter
    /// slot routes back here (`Awaiter::Task(task_id)` on either
    /// `ExternalFuture::Pending` or `AwaitedGather`).
    pub fn block_current_on(&mut self, awaitable_id: HeapId, heap: &Heap) {
        if let Some(task_id) = self.current_task {
            let task = self.get_task_mut(task_id);
            debug_assert!(matches!(task.state, TaskState::Ready(Ok(()))));
            heap.inc_ref(awaitable_id);
            task.state = TaskState::Blocked(awaitable_id);
        }
    }

    /// Returns all pending (unresolved) CallIds.
    pub fn pending_call_ids(&self) -> Vec<CallId> {
        self.pending_externals.keys().copied().collect()
    }

    /// Removes the queue entry when delivering directly to an exiting task's waiter.
    pub fn remove_from_ready_queue(&mut self, task_id: TaskId) {
        self.ready_queue.retain(|&id| id != task_id);
    }

    /// Spawns a new task from a coroutine, enforcing one-task-per-coroutine.
    ///
    /// Returns `None` if `coroutine_id` is already driving a task —
    /// caught here because cross-gather reuse can hit two spawns while
    /// both coroutine states are still `New`, so the state check in
    /// `await_coroutine` doesn't catch it. Callers translate `None`
    /// into a `RuntimeError: cannot reuse already awaited coroutine`.
    /// Both `coroutine_id` and the `GatherSlot` built from `gather_id` become
    /// **owning** references held by the new task; the matching `dec_ref`
    /// happens in [`Scheduler::cancel_task`]. It takes `gather_id` rather than
    /// a ready-made `Awaiter` so the `None` return above has nothing to unwind.
    pub fn spawn(&mut self, heap: &Heap, coroutine_id: HeapId, gather_id: Option<HeapId>) -> Option<TaskId> {
        if self.coroutine_to_task.contains_key(&coroutine_id) {
            return None;
        }

        let task_id = TaskId::new(self.next_task_id);
        self.next_task_id += 1;

        // Take ownership of the heap references — the task now holds an inc_ref'd
        // pointer to its coroutine and (if applicable) its enclosing gather.
        // The slot is keyed by the coroutine's own id, which is what
        // `resolve_child` looks up.
        heap.inc_ref(coroutine_id);
        let awaiter = gather_id.map(|gather| {
            heap.inc_ref(gather);
            Awaiter::GatherSlot {
                gather,
                source: coroutine_id,
            }
        });

        let task = Task::new(task_id, Some(coroutine_id), awaiter);
        self.tasks.insert(task_id, task);
        self.coroutine_to_task.insert(coroutine_id, task_id);
        self.ready_queue.push_back(task_id);

        Some(task_id)
    }

    /// Gets the next ready task from the queue.
    ///
    /// Returns `None` if no tasks are ready.
    pub fn next_ready_task(&mut self) -> Option<TaskId> {
        self.ready_queue.pop_front()
    }

    /// Wakes a blocked task with a value already on its stack, or an exception to raise.
    pub fn make_ready(&mut self, task_id: TaskId, result: RunResult<()>, heap: &mut Heap) {
        let task = self.get_task_mut(task_id);
        debug_assert!(matches!(task.state, TaskState::Blocked(_)));
        let old_state = mem::replace(&mut task.state, TaskState::Ready(result));
        old_state.drop_with(heap);
        self.ready_queue.push_back(task_id);
    }

    /// Takes the selected task's resume result exactly once, before executing its context.
    pub fn take_resume_result(&mut self, task_id: TaskId) -> RunResult<()> {
        match &mut self.get_task_mut(task_id).state {
            TaskState::Ready(result) => mem::replace(result, Ok(())),
            TaskState::Blocked(_) => panic!("cannot resume a blocked task"),
        }
    }

    /// Sets the current task.
    pub fn set_current_task(&mut self, task_id: Option<TaskId>) {
        self.current_task = task_id;
    }

    /// Removes a task and releases its saved context and heap references.
    /// Cancels tasks under any gather it is still blocked on, using an iterative walk
    /// so deeply nested gathers cannot overflow the native stack during teardown.
    pub fn cancel_task(&mut self, task_id: TaskId, heap: &mut HeapReader<'_>) {
        let mut pending: SmallVec<[TaskId; 4]> = smallvec![task_id];
        while let Some(task_id) = pending.pop() {
            self.cancel_one(task_id, heap, &mut pending);
        }
    }

    /// Cancels one task, queueing the tasks spawned under any gather it was
    /// blocked on for [`Scheduler::cancel_task`] to drain.
    ///
    /// Dropping this task ahead of the children it queued is sound: each owns
    /// an inc_ref on that same gather (see [`Scheduler::spawn`]).
    fn cancel_one(&mut self, task_id: TaskId, heap: &mut HeapReader<'_>, pending: &mut SmallVec<[TaskId; 4]>) {
        // No-op if the task has already been removed (idempotent — finalization
        // sites may iterate task ids that include already-cancelled siblings).
        let Some(task) = self.tasks.remove(&task_id) else {
            return;
        };

        // The VM must discard this task's loaded context before activating another task.
        if self.current_task == Some(task_id) {
            self.current_task = None;
        }

        if let Some(coroutine_id) = task.coroutine_id {
            self.coroutine_to_task.remove(&coroutine_id);
        }

        self.ready_queue.retain(|&id| id != task_id);

        // Blocked on a gather: queue the tasks spawned under it. An
        // external future needs no extra teardown.
        if let TaskState::Blocked(blocked_id) = task.state {
            self.queue_gather_tasks(blocked_id, heap, pending);
        }

        task.drop_with(heap);
    }

    /// Queues every task spawned under the gather `root`, walking nested
    /// gathers iteratively.
    ///
    /// A gather item can itself be a gather (`gather(gather(coro()))`), whose
    /// tasks are just as orphaned as direct coroutine children if left in
    /// `self.tasks` — they would keep running and then deliver a result to the
    /// task cancelled here. External children *are* left alone: the owning
    /// `Awaiter::GatherSlot` anchors them.
    ///
    /// Must run while the cancelled task still holds its `Blocked` inc_ref on
    /// `root`, since the walk takes no references of its own: each nested
    /// gather is kept alive by its parent's `items`, and the parent in turn by
    /// the `Awaiter::GatherSlot` inc_ref that nested child holds.
    fn queue_gather_tasks(&self, root: HeapId, heap: &HeapReader<'_>, pending: &mut SmallVec<[TaskId; 4]>) {
        // Gathers nest as a tree — a gather may only be awaited once, so the
        // walk cannot revisit a node and terminates.
        let mut gathers: SmallVec<[HeapId; 4]> = smallvec![root];
        while let Some(gather_id) = gathers.pop() {
            // Coroutine and external children land here too; only gathers have
            // children of their own to walk.
            let HeapReadOutput::GatherFuture(gather) = heap.read(gather_id) else {
                continue;
            };
            if let Some(awaited) = gather.get(heap).as_awaited() {
                for child_id in awaited.pending_children.keys() {
                    match self.coroutine_to_task.get(child_id) {
                        Some(&task_id) => pending.push(task_id),
                        None => gathers.push(*child_id),
                    }
                }
            }
            drop(gather);
        }
    }

    /// Whether this task still exists and is waiting for an awaitable to settle.
    #[inline]
    pub fn is_blocked(&self, task_id: TaskId) -> bool {
        self.tasks
            .get(&task_id)
            .is_some_and(|task| matches!(task.state, TaskState::Blocked(_)))
    }

    /// Returns true if a task with `task_id` currently exists in the
    /// scheduler. Cancelled tasks are removed from the map, so this returning
    /// `false` means the task is gone.
    #[inline]
    pub fn has_task(&self, task_id: TaskId) -> bool {
        self.tasks.contains_key(&task_id)
    }

    /// Number of tasks the scheduler still holds.
    ///
    /// Test-only: a task whose await has already failed must not stay parked,
    /// and a finished run tears the scheduler down, so this count while
    /// suspended is the only way a test can see one.
    #[cfg(feature = "test-hooks")]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Cleans up all scheduler resources: the pending-future inc_refs and
    /// every remaining task (via [`Scheduler::cancel_task`]).
    pub fn cleanup(&mut self, heap: &mut HeapReader<'_>) {
        // Release the inc_refs the scheduler holds on each pending future.
        for (_, future_id) in mem::take(&mut self.pending_externals) {
            heap.dec_ref(future_id);
        }
        let task_ids: Vec<TaskId> = self.tasks.keys().copied().collect();
        for task_id in task_ids {
            self.cancel_task(task_id, heap);
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
