//! Task scheduler for async execution and call ID allocation.
//!
//! # Task Model
//!
//! - Task 0 is the "main task" which uses the VM's stack/frames directly
//! - Spawned tasks (1+) store their own execution context in the Task struct
//! - When switching tasks, the scheduler swaps contexts with the VM

use std::{collections::VecDeque, mem};

use ahash::{AHashMap, AHashSet};

use crate::{
    args::ArgValues,
    asyncio::{CallId, TaskId},
    exception_private::RunError,
    heap::{ContainsHeap, DropWithHeap, Heap, HeapData, HeapId, HeapReadOutput, HeapReader},
    intern::FunctionId,
    parse::CodeRange,
    resource::ResourceTracker,
    value::Value,
};

/// Task execution state for async scheduling.
///
/// Tracks whether a task is ready to run, blocked waiting for something,
/// or has completed (successfully or with an error).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum TaskState {
    /// Task is ready to execute (in the ready queue).
    Ready,
    /// Task is blocked waiting for an external call to resolve.
    BlockedOnCall(CallId),
    /// Task is blocked waiting for a GatherFuture to complete.
    BlockedOnGather(HeapId),
    /// Task completed successfully with a return value.
    Completed(Value),
    /// Task failed with an error.
    Failed(RunError),
}

impl DropWithHeap for TaskState {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        match self {
            Self::Ready | Self::BlockedOnCall(_) | Self::Failed(_) => {}
            Self::BlockedOnGather(gather_id) => heap.heap_mut().dec_ref(gather_id),
            Self::Completed(value) => value.drop_with_heap(heap),
        }
    }
}

/// A single async task with its own execution context.
///
/// The main task (task 0) doesn't store its own frames/stack - it uses the VM's
/// directly. Spawned tasks store their execution context here so they can be
/// swapped in and out.
///
/// # Context Switching
///
/// When switching away from a non-main task, its context is saved here.
/// When switching to it, the context is loaded into the VM.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Task {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Serialized call frames for this task's execution.
    /// Empty for the main task (which uses VM's frames directly).
    pub frames: Vec<SerializedTaskFrame>,
    /// Operand stack for this task.
    /// Empty for the main task (which uses VM's stack directly).
    pub stack: Vec<Value>,
    /// Exception stack for nested except blocks.
    pub exception_stack: Vec<Value>,
    /// VM-level instruction_ip (for exception table lookup).
    pub instruction_ip: usize,
    /// Coroutine being executed by this task (if any).
    /// Used to mark the coroutine as Completed when the task finishes.
    pub coroutine_id: Option<HeapId>,
    /// GatherFuture this task belongs to (if spawned by gather).
    /// Used to cancel sibling tasks when this task fails. The gather itself
    /// stores the slot-index mapping under `AwaitedGather::pending_tasks`.
    pub gather_id: Option<HeapId>,
    /// Current execution state.
    pub state: TaskState,
    /// CallId that unblocked this task (set when task transitions from Blocked to Ready).
    /// Used to retrieve the resolved value when the task resumes.
    pub unblocked_by: Option<CallId>,
}

impl DropWithHeap for Task {
    fn drop_with_heap<H: ContainsHeap>(mut self, heap: &mut H) {
        for value in self.stack.drain(..) {
            value.drop_with_heap(heap);
        }
        for value in self.exception_stack.drain(..) {
            value.drop_with_heap(heap);
        }
        self.state.drop_with_heap(heap);
        if let Some(coro_id) = self.coroutine_id.take() {
            heap.heap_mut().dec_ref(coro_id);
        }
        if let Some(gid) = self.gather_id.take() {
            heap.heap_mut().dec_ref(gid);
        }
    }
}

/// Serialized call frame for task storage.
///
/// Similar to `SerializedFrame` but used within the scheduler for task context.
/// Cannot store `&Code` references - uses `FunctionId` to look up code on resume.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Call site position (for tracebacks).
    pub call_position: Option<CodeRange>,
}

impl Task {
    /// Creates a new task in the Ready state.
    ///
    /// # Arguments
    /// * `id` - Unique task identifier
    /// * `coroutine_id` - Optional HeapId of the coroutine being executed
    /// * `gather_id` - Optional HeapId of the GatherFuture this task belongs to
    pub fn new(id: TaskId, coroutine_id: Option<HeapId>, gather_id: Option<HeapId>) -> Self {
        Self {
            id,
            frames: Vec::new(),
            stack: Vec::new(),
            exception_stack: Vec::new(),
            instruction_ip: 0,
            coroutine_id,
            gather_id,
            state: TaskState::Ready,
            unblocked_by: None,
        }
    }

    /// Returns true if this task has completed (successfully or with failure).
    #[inline]
    pub fn is_finished(&self) -> bool {
        matches!(self.state, TaskState::Completed(_) | TaskState::Failed(_))
    }
}

/// Internal representation of a pending external call.
///
/// Stores the data needed to retry or resume an external function call,
/// along with tracking information for the task that created it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingCallData {
    /// Arguments for the function (includes both positional and keyword args).
    pub args: ArgValues,
    /// Task that created this call (for ignoring results if task is cancelled).
    pub creator_task: TaskId,
    /// If `Some`, the resolved value should be fanned into the named
    /// `GatherFuture` instead of directly unblocking `creator_task`.
    pub gather: Option<HeapId>,
}

/// Scheduler for managing call IDs, async tasks, and external call tracking.
///
/// Always present on the VM (not optional). Owns the `next_call_id` counter
/// used by both sync and async code paths, plus all async-related state:
/// - Task management (creation, scheduling, completion)
/// - External call tracking and resolution
///
/// # Main Task
///
/// Task 0 is the "main task" which executes using the VM's stack/frames directly.
/// It's always created at scheduler initialization but doesn't store its own context
/// (the VM holds it). Spawned tasks (1+) store their context in the Task struct.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Scheduler {
    /// All tasks keyed by their `TaskId`.
    tasks: AHashMap<TaskId, Task>,
    /// Queue of task IDs ready to execute.
    ready_queue: VecDeque<TaskId>,
    /// Currently executing task (None only during task switching).
    current_task: Option<TaskId>,
    /// Counter for generating new task IDs.
    next_task_id: u32,
    /// Counter for external call IDs (always incremented, even for sync resolution).
    next_call_id: u32,
    /// Maps CallId -> pending call data for unresolved external calls.
    /// Populated when host calls `run_pending()`.
    pending_calls: AHashMap<CallId, PendingCallData>,
    /// Maps CallId -> resolved Value for futures that have been resolved.
    /// Entry is removed when the value is consumed by awaiting.
    resolved: AHashMap<CallId, Value>,
    /// CallIds that have been awaited (to detect double-await).
    consumed: AHashSet<CallId>,
}

impl Scheduler {
    /// Creates a new scheduler with the main task (task 0) as current.
    ///
    /// The main task uses the VM's stack/frames directly and is always present.
    /// It starts as the current task (not in the ready queue) since it runs
    /// immediately without needing to be scheduled.
    pub fn new() -> Self {
        let main_task_id = TaskId::default();
        let mut main_task = Task::new(main_task_id, None, None);
        // Main task starts Running, not Ready (it's the current task, not waiting)
        main_task.state = TaskState::Ready; // Will be set properly when it blocks
        let mut tasks = AHashMap::new();
        tasks.insert(main_task_id, main_task);
        Self {
            tasks,
            ready_queue: VecDeque::new(), // Main task is current, not in ready queue
            current_task: Some(main_task_id),
            next_task_id: 1,
            next_call_id: 0,
            pending_calls: AHashMap::new(),
            resolved: AHashMap::new(),
            consumed: AHashSet::new(),
        }
    }

    /// Returns the currently executing task ID.
    ///
    /// Returns `None` only during task switching operations.
    #[inline]
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.current_task
    }

    /// Returns a reference to a task by ID.
    ///
    /// # Panics
    /// Panics if the task ID doesn't exist.
    #[inline]
    pub fn get_task(&self, task_id: TaskId) -> &Task {
        self.tasks.get(&task_id).expect("Scheduler::get_task: task not found")
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

    /// Stores pending call data for an external function call.
    ///
    /// Called when the host uses async resolution (`run_pending()`).
    pub fn add_pending_call(&mut self, call_id: CallId, data: PendingCallData) {
        self.pending_calls.insert(call_id, data);
    }

    /// Removes the pending-call entry for `call_id` and returns its data, if
    /// present.
    ///
    /// Callers that only want to remove the entry can ignore the return
    /// value (e.g. `HeapRead::fail` clearing every external the gather was
    /// waiting on).
    pub fn take_pending_call(&mut self, call_id: CallId) -> Option<PendingCallData> {
        self.pending_calls.remove(&call_id)
    }

    /// Returns true if a CallId has already been awaited (consumed).
    #[inline]
    pub fn is_consumed(&self, call_id: CallId) -> bool {
        self.consumed.contains(&call_id)
    }

    /// Marks a CallId as consumed (awaited).
    pub fn mark_consumed(&mut self, call_id: CallId) {
        self.consumed.insert(call_id);
    }

    /// Registers a gather as waiting on an external future.
    ///
    /// Mutates the existing `PendingCallData` entry to attach the gather
    /// pointer. The CallId must already be present in `pending_calls` — the
    /// host adds it via `add_pending_call` when it returns
    /// `ExtFunctionResult::Future` for the original call, and gather routing
    /// is registered later when the gather is awaited.
    ///
    /// The slot indices the resolved value should fan into live on the
    /// gather itself, under `AwaitedGather::pending_calls`.
    ///
    /// # Panics
    /// Panics if the CallId is not in `pending_calls` or is already routed
    /// to a gather (would indicate a bug in await-side bookkeeping).
    pub fn register_gather_for_call(&mut self, call_id: CallId, gather_id: HeapId) {
        let data = self
            .pending_calls
            .get_mut(&call_id)
            .expect("register_gather_for_call: CallId must already be a pending call");
        debug_assert!(
            data.gather.is_none(),
            "register_gather_for_call: CallId already routed to a gather",
        );
        data.gather = Some(gather_id);
    }

    /// Records a resolved value for `call_id`.
    pub fn record_resolved(&mut self, call_id: CallId, value: Value) {
        self.resolved.insert(call_id, value);
    }

    /// Takes the resolved value for a CallId, if available.
    ///
    /// Removes the value from the resolved map and returns it.
    /// Returns `None` if the call hasn't been resolved yet.
    pub fn take_resolved(&mut self, call_id: CallId) -> Option<Value> {
        self.resolved.remove(&call_id)
    }

    /// Takes the resolved value for a task that was unblocked.
    ///
    /// If the task has an `unblocked_by` CallId set, takes the resolved value
    /// for that call and clears the `unblocked_by` field.
    /// Returns `None` if the task wasn't unblocked by a resolved call.
    pub fn take_resolved_for_task(&mut self, task_id: TaskId) -> Option<Value> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .expect("Scheduler::take_resolved_for_task: task not found");
        if let Some(call_id) = task.unblocked_by.take() {
            self.resolved.remove(&call_id)
        } else {
            None
        }
    }

    /// Marks the current task as blocked on an external call.
    ///
    /// The task will be unblocked when `resolve()` is called with the matching CallId.
    pub fn block_current_on_call(&mut self, call_id: CallId) {
        if let Some(task_id) = self.current_task {
            let task = self.get_task_mut(task_id);
            task.state = TaskState::BlockedOnCall(call_id);
        }
    }

    /// Marks the current task as blocked on a GatherFuture.
    ///
    /// The task will be unblocked when all gathered tasks complete.
    pub fn block_current_on_gather(&mut self, gather_id: HeapId, heap: &Heap<impl ResourceTracker>) {
        if let Some(task_id) = self.current_task {
            let task = self.get_task_mut(task_id);
            heap.inc_ref(gather_id);
            task.state = TaskState::BlockedOnGather(gather_id);
        }
    }

    /// Returns all pending (unresolved) CallIds.
    pub fn pending_call_ids(&self) -> Vec<CallId> {
        self.pending_calls.keys().copied().collect()
    }

    /// Removes a task from the ready queue.
    ///
    /// Used when handling the main task directly (via `prepare_main_task_after_resolve`)
    /// instead of through the normal task switching mechanism.
    pub fn remove_from_ready_queue(&mut self, task_id: TaskId) {
        self.ready_queue.retain(|&id| id != task_id);
    }

    /// Spawns a new task from a coroutine.
    ///
    /// Creates a new task that will execute the given coroutine when scheduled.
    /// The task is added to the ready queue.
    ///
    /// Both `coroutine_id` and `gather_id` (when present) become **owning**
    /// references held by the new task — `inc_ref` is called on each before
    /// storing. The matching `dec_ref` happens in [`Scheduler::remove_task`]
    /// when the task is eventually removed (typically at gather finalization).
    ///
    /// # Arguments
    /// * `heap` - Heap to increment reference counts in
    /// * `coroutine_id` - HeapId of the coroutine to execute
    /// * `gather_id` - Optional HeapId of the GatherFuture this task belongs to
    ///
    /// # Returns
    /// The TaskId of the newly created task.
    pub fn spawn(
        &mut self,
        heap: &Heap<impl ResourceTracker>,
        coroutine_id: HeapId,
        gather_id: Option<HeapId>,
    ) -> TaskId {
        let task_id = TaskId::new(self.next_task_id);
        self.next_task_id += 1;

        // Take ownership of the heap references — the task now holds an inc_ref'd
        // pointer to its coroutine and (if applicable) its enclosing gather.
        heap.inc_ref(coroutine_id);
        if let Some(gid) = gather_id {
            heap.inc_ref(gid);
        }

        let task = Task::new(task_id, Some(coroutine_id), gather_id);
        self.tasks.insert(task_id, task);
        self.ready_queue.push_back(task_id);

        task_id
    }

    /// Gets the next ready task from the queue.
    ///
    /// Returns `None` if no tasks are ready.
    pub fn next_ready_task(&mut self) -> Option<TaskId> {
        self.ready_queue.pop_front()
    }

    /// Replaces a task's state, properly releasing any heap references owned
    /// by the previous state.
    pub fn set_state(&mut self, task_id: TaskId, new_state: TaskState, heap: &mut Heap<impl ResourceTracker>) {
        let task = self.get_task_mut(task_id);
        let old_state = mem::replace(&mut task.state, new_state);
        old_state.drop_with_heap(heap);
    }

    /// Adds a task back to the ready queue.
    pub fn make_ready(&mut self, task_id: TaskId, heap: &mut Heap<impl ResourceTracker>) {
        self.set_state(task_id, TaskState::Ready, heap);
        self.ready_queue.push_back(task_id);
    }

    /// Sets the current task.
    pub fn set_current_task(&mut self, task_id: Option<TaskId>) {
        self.current_task = task_id;
    }

    /// Marks a task as failed with an error.
    ///
    /// If the task is part of a gather, returns the gather_id so the caller
    /// can collect siblings from the gather on the heap.
    ///
    /// # Returns
    /// The gather_id if this task belongs to a gather (for sibling lookup).
    pub fn fail_task(
        &mut self,
        task_id: TaskId,
        error: RunError,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Option<HeapId> {
        let gather_id = self.get_task(task_id).gather_id;
        self.set_state(task_id, TaskState::Failed(error), heap);
        gather_id
    }

    /// Cancels a task, fully releasing its resources and removing it from the
    /// scheduler.
    ///
    /// Drops the task's stack, exception stack, any pending `Completed` result,
    /// and recursively cancels any inner gather it was blocked on. After this
    /// call the task no longer exists in `Scheduler::tasks`; its owning
    /// references to its coroutine and (outer) gather are released by
    /// [`Scheduler::remove_task`].
    pub fn cancel_task(&mut self, task_id: TaskId, heap: &mut Heap<impl ResourceTracker>) {
        // No-op if the task has already been removed (idempotent — finalization
        // sites may iterate task ids that include already-cancelled siblings).
        let Some(task) = self.tasks.remove(&task_id) else {
            return;
        };

        // If we're cancelling the current task, clear `current_task` so callers
        // don't try to look up a task that's about to be dropped (e.g.
        // `resume_with_resolved_futures` after `fail_for_call` tore down the
        // gather containing the previously-current task).
        if self.current_task == Some(task_id) {
            self.current_task = None;
        }

        if !task.is_finished() {
            // Remove from ready queue if present (do this before getting mutable task reference)
            self.ready_queue.retain(|&id| id != task_id);

            // Drop any *non-gather-routed* external calls this task was the
            // creator of. The host may still respond to them later; with the
            // entry gone, `take_pending_call` returns `None` and
            // `resolve_future` drops the resolved value instead of trying
            // to wake a removed task.
            //
            // Gather-routed entries are kept: even though `task_id` made the
            // call, ownership effectively transferred to the gather when
            // `register_gather_for_call` set `data.gather`. Dropping them
            // would orphan the gather (its own `pending_calls` map still
            // references the CallId, so it would wait forever for a
            // resolution that we'd silently drop). Gather-routed entries are
            // cleaned up either by the gather completing successfully
            // (`resolve_child` removes them on resolution) or by the gather
            // failing (`HeapRead::fail` drains them on tear-down).
            self.pending_calls
                .retain(|_, data| data.creator_task != task_id || data.gather.is_some());

            // If blocked on a nested gather, recursively cancel inner tasks first.
            // Only an `Awaited` gather has spawned tasks — a `Pending` gather
            // has never run, and `Completed`/`Failed` mean the gather has
            // already shed its child tasks.
            if let TaskState::BlockedOnGather(gather_id) = task.state {
                let HeapData::GatherFuture(gather) = heap.get(gather_id) else {
                    panic!("Scheduler::cancel_task: expected GatherFuture heap entry for gather_id {gather_id:?}");
                };
                let inner_task_ids: Vec<TaskId> = gather
                    .as_awaited()
                    .map(|awaited| awaited.pending_tasks.keys().copied().collect())
                    .unwrap_or_default();
                for inner_task_id in inner_task_ids {
                    self.cancel_task(inner_task_id, heap);
                }
            }
        }

        task.drop_with_heap(heap);
    }

    /// Fails the task blocked on a specific CallId with an error.
    ///
    /// For a gather-routed call, tears the gather down eagerly via
    /// [`HeapRead::fail`]. For an "indirect" failure (the call was made by a
    /// task that's a child of a gather), just sets the creator task's state
    /// to `Failed`; the failure is then surfaced when a sibling completes
    /// and `HeapRead::resolve_child`'s completion scan finds the Failed
    /// task. (The two-phase pattern keeps gather teardown anchored in the
    /// run-loop side, where exception propagation through the waiter's
    /// frame is straightforward.)
    pub fn fail_for_call(&mut self, call_id: CallId, error: RunError, heap: &mut HeapReader<'_, impl ResourceTracker>) {
        let Some(pending) = self.pending_calls.remove(&call_id) else {
            // Typically means the call was already resolved or cancelled; no task to fail.
            return;
        };
        if let Some(gather_id) = pending.gather {
            let HeapReadOutput::GatherFuture(mut gather) = heap.read(gather_id) else {
                panic!("gather_id doesn't point to a GatherFuture")
            };
            let waiter_id = gather.fail(self, heap, &error);
            drop(gather);
            self.fail_task(waiter_id, error, heap);
        } else {
            self.fail_task(pending.creator_task, error, heap);
        }
    }

    /// Returns true if a task has been cancelled or failed.
    #[inline]
    pub fn is_task_failed(&self, task_id: TaskId) -> bool {
        self.tasks
            .get(&task_id)
            .is_some_and(|task| matches!(task.state, TaskState::Failed(_)))
    }

    /// Cleans up all scheduler resources: pending calls, resolved values, and
    /// every remaining task (via [`Scheduler::remove_task`]).
    pub fn cleanup(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        // Drop pending call arguments
        for (_, data) in mem::take(&mut self.pending_calls) {
            data.args.drop_with_heap(heap);
        }
        // Drop resolved values
        for (_, value) in mem::take(&mut self.resolved) {
            value.drop_with_heap(heap);
        }
        // Remove every remaining task — drains the map and runs the per-task
        // cleanup uniformly via `remove_task`.
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
