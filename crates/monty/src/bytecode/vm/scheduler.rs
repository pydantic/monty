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
    /// Used to cancel sibling tasks when this task fails.
    pub gather_id: Option<HeapId>,
    /// Indices in the gather's results where this task's result should be stored.
    ///
    /// A single task may map to multiple gather slots when the same coroutine
    /// is passed to `asyncio.gather` more than once: e.g. `gather(c, c)` spawns
    /// one task that fills slots `[0, 1]`. Empty for non-gather tasks.
    pub gather_result_indices: Vec<usize>,
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
    /// * `gather_result_indices` - Slots in the gather's results this task fills
    ///   (empty for non-gather tasks; multiple when the same coroutine appears
    ///   more than once in the gather)
    pub fn new(
        id: TaskId,
        coroutine_id: Option<HeapId>,
        gather_id: Option<HeapId>,
        gather_result_indices: Vec<usize>,
    ) -> Self {
        Self {
            id,
            frames: Vec::new(),
            stack: Vec::new(),
            exception_stack: Vec::new(),
            instruction_ip: 0,
            coroutine_id,
            gather_id,
            gather_result_indices,
            state: TaskState::Ready,
            unblocked_by: None,
        }
    }

    /// Returns true if this task has completed (successfully or with failure).
    #[inline]
    pub fn is_finished(&self) -> bool {
        matches!(self.state, TaskState::Completed(_) | TaskState::Failed(_))
    }

    /// Appends every heap reference owned by this parked task to `roots`.
    ///
    /// Suspended tasks keep their operand stack and exception stack outside the
    /// live VM state. GC must therefore walk both the saved values and the task's
    /// scheduler metadata, otherwise reachable heap entries can be swept while the
    /// task is blocked. `coroutine_id` and `gather_id` are owning references
    /// (inc_ref'd in [`Scheduler::spawn`], dec_ref'd in [`Scheduler::remove_task`])
    /// so they participate in the root set.
    fn extend_gc_roots(&self, roots: &mut Vec<HeapId>) {
        roots.extend(self.stack.iter().filter_map(Value::ref_id));
        roots.extend(self.exception_stack.iter().filter_map(Value::ref_id));
        roots.extend(self.coroutine_id);
        roots.extend(self.gather_id);
        self.state.extend_gc_roots(roots);
    }
}

impl TaskState {
    /// Appends every heap reference stored in this task state to `roots`.
    ///
    /// Most task states are pure metadata, but completed tasks retain their return
    /// values and gather-blocked tasks retain the gather heap object that will wake
    /// them later.
    fn extend_gc_roots(&self, roots: &mut Vec<HeapId>) {
        match self {
            Self::Ready | Self::BlockedOnCall(_) | Self::Failed(_) => {}
            Self::BlockedOnGather(gather_id) => roots.push(*gather_id),
            Self::Completed(value) => roots.extend(value.ref_id()),
        }
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
}

impl PendingCallData {
    /// Appends every heap reference owned by this pending call entry to `roots`.
    ///
    /// External-call state can outlive the active VM stack while the host resolves
    /// the call, so GC must treat any stored argument values as roots.
    fn extend_gc_roots(&self, roots: &mut Vec<HeapId>) {
        self.args.extend_gc_roots(roots);
    }
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
    /// Maps CallId -> (gather_heap_id, result_indices) for gathers waiting on external futures.
    ///
    /// When a CallId is resolved, the result is stored in the gather's results at every
    /// listed index. Multiple indices arise when the same external future is passed to
    /// `asyncio.gather` more than once, e.g. `gather(f, f)`.
    gather_waiters: AHashMap<CallId, (HeapId, Vec<usize>)>,
}

impl Scheduler {
    /// Creates a new scheduler with the main task (task 0) as current.
    ///
    /// The main task uses the VM's stack/frames directly and is always present.
    /// It starts as the current task (not in the ready queue) since it runs
    /// immediately without needing to be scheduled.
    pub fn new() -> Self {
        let main_task_id = TaskId::default();
        let mut main_task = Task::new(main_task_id, None, None, Vec::new());
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
            gather_waiters: AHashMap::new(),
        }
    }

    /// Appends every scheduler-owned heap reference to `roots`.
    ///
    /// The VM's live stack only covers the currently executing task. Spawned or
    /// blocked tasks, resolved futures, and gather bookkeeping all live in the
    /// scheduler and must therefore participate in the GC root set.
    pub(crate) fn extend_gc_roots(&self, roots: &mut Vec<HeapId>) {
        for task in self.tasks.values() {
            task.extend_gc_roots(roots);
        }
        for data in self.pending_calls.values() {
            data.extend_gc_roots(roots);
        }
        roots.extend(self.resolved.values().filter_map(Value::ref_id));
        roots.extend(self.gather_waiters.values().map(|(gather_id, _)| *gather_id));
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

    /// Removes a call_id from the pending_calls map.
    ///
    /// Called when resolving a gather's external future - the call is no longer
    /// pending once the result has been stored in the gather's results.
    pub fn remove_pending_call(&mut self, call_id: CallId) {
        self.pending_calls.remove(&call_id);
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
    /// When the CallId is resolved, the result will be stored in the gather's results
    /// at every index in `result_indices`. Multiple indices arise when the same
    /// external future is passed to `gather` more than once.
    pub fn register_gather_for_call(&mut self, call_id: CallId, gather_id: HeapId, result_indices: Vec<usize>) {
        self.gather_waiters.insert(call_id, (gather_id, result_indices));
    }

    /// Returns gather info if a gather is waiting on this CallId.
    ///
    /// Returns `(gather_heap_id, result_indices)` if found, `None` otherwise.
    /// Removes the entry from `gather_waiters`.
    pub fn take_gather_waiter(&mut self, call_id: CallId) -> Option<(HeapId, Vec<usize>)> {
        self.gather_waiters.remove(&call_id)
    }

    /// Resolves a CallId with a value.
    ///
    /// Stores the value for later retrieval when the future is awaited.
    /// If a task is blocked on this call, it will be unblocked.
    ///
    /// Uses `pending_calls` for O(1) lookup of the blocked task instead of
    /// scanning all tasks.
    pub fn resolve(&mut self, call_id: CallId, value: Value) {
        // Get blocked task from pending_calls before removing (O(1) lookup)
        let blocked_task = self.pending_calls.remove(&call_id).map(|data| data.creator_task);

        // Store the resolved value
        self.resolved.insert(call_id, value);

        // Unblock the task if found
        if let Some(task_id) = blocked_task {
            let task = self.get_task_mut(task_id);
            if matches!(task.state, TaskState::BlockedOnCall(cid) if cid == call_id) {
                task.state = TaskState::Ready;
                task.unblocked_by = Some(call_id);
                self.ready_queue.push_back(task_id);
            }
        }
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
    /// * `gather_result_indices` - Indices in the gather's results for this task
    ///   (multiple when the same coroutine appears more than once in the gather)
    ///
    /// # Returns
    /// The TaskId of the newly created task.
    pub fn spawn(
        &mut self,
        heap: &Heap<impl ResourceTracker>,
        coroutine_id: HeapId,
        gather_id: Option<HeapId>,
        gather_result_indices: Vec<usize>,
    ) -> TaskId {
        let task_id = TaskId::new(self.next_task_id);
        self.next_task_id += 1;

        // Take ownership of the heap references — the task now holds an inc_ref'd
        // pointer to its coroutine and (if applicable) its enclosing gather.
        heap.inc_ref(coroutine_id);
        if let Some(gid) = gather_id {
            heap.inc_ref(gid);
        }

        let task = Task::new(task_id, Some(coroutine_id), gather_id, gather_result_indices);
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

    /// Marks a task as completed with a result value.
    ///
    /// If the task is part of a gather, updates the gather's results.
    /// If this completes the gather, unblocks the waiting task.
    pub fn complete_task(&mut self, task_id: TaskId, result: Value, heap: &mut Heap<impl ResourceTracker>) {
        self.set_state(task_id, TaskState::Completed(result), heap);
    }

    /// Marks a task as failed with an error.
    ///
    /// If the task is part of a gather, returns the gather_id so the caller
    /// can collect siblings from `GatherFuture.task_ids` on the heap.
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

        if !task.is_finished() {
            // Remove from ready queue if present (do this before getting mutable task reference)
            self.ready_queue.retain(|&id| id != task_id);

            // If blocked on a nested gather, recursively cancel inner tasks first.
            if let TaskState::BlockedOnGather(gather_id) = task.state {
                let HeapData::GatherFuture(gather) = heap.get(gather_id) else {
                    panic!("Scheduler::cancel_task: expected GatherFuture heap entry for gather_id {gather_id:?}");
                };
                let inner_task_ids = gather.task_ids.clone();
                for inner_task_id in inner_task_ids {
                    self.cancel_task(inner_task_id, heap);
                }
            }
        }

        task.drop_with_heap(heap);
    }

    /// Fails the task blocked on a specific CallId with an error.
    pub fn fail_for_call(&mut self, call_id: CallId, error: RunError, heap: &mut HeapReader<'_, impl ResourceTracker>) {
        // Get blocked task from pending_calls (O(1) lookup)
        let Some(pending_call) = self.pending_calls.remove(&call_id) else {
            // No pending call found - nothing to fail. Possibly cancelled by a sibling task failure.
            return;
        };

        let task_id = pending_call.creator_task;

        // Check if a gather is waiting on this CallId
        if let Some((gather_id, _result_indices)) = self.take_gather_waiter(call_id) {
            self.remove_pending_call(call_id);

            // Get the gather's waiter, task_ids, and OTHER pending calls
            // We need to remove all pending calls for this gather from gather_waiters
            // before we dec_ref the gather, otherwise subsequent errors for the same
            // gather would try to access a freed heap object.
            // Use get_mut and take to avoid allocations - gather is being destroyed anyway.
            let HeapReadOutput::GatherFuture(mut gather) = heap.read(gather_id) else {
                panic!("gather_id doesn't point to a GatherFuture")
            };
            let gather_mut = gather.get_mut(heap);
            let mut other_pending_calls = mem::take(&mut gather_mut.pending_calls);
            other_pending_calls.retain(|&cid| cid != call_id);
            let Some(waiter_id) = gather_mut.waiter else {
                panic!("gather has no waiter task")
            };
            let task_ids = mem::take(&mut gather_mut.task_ids);
            // Drop the HeapRead before operations that may free heap objects
            drop(gather);

            // Remove all other pending calls for this gather from gather_waiters and pending_calls
            // This prevents subsequent errors from trying to access the freed gather
            for other_call_id in other_pending_calls {
                self.take_gather_waiter(other_call_id);
                self.remove_pending_call(other_call_id);
            }

            // Cancel all sibling tasks in the gather
            for sibling_id in task_ids {
                self.cancel_task(sibling_id, heap);
            }

            // Fail the waiter task (the task that awaited the gather)
            self.fail_task(waiter_id, error, heap);
        } else {
            // Not a gather-related error - just fail the blocked task.
            self.fail_task(task_id, error, heap);
        }
    }

    /// Returns the task that created a specific pending call.
    ///
    /// Used to check if a pending call's creator task has been cancelled.
    #[inline]
    pub fn get_pending_call_creator(&self, call_id: CallId) -> Option<TaskId> {
        self.pending_calls.get(&call_id).map(|data| data.creator_task)
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
