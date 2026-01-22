//! Task scheduler for async execution.
//!
//! This module implements the scheduler for managing concurrent async tasks
//! and tracking external function calls. The scheduler is always present
//! (created at VM initialization) to maintain separation of concerns.
//!
//! # Task Model
//!
//! - Task 0 is the "main task" which uses the VM's stack/frames directly
//! - Spawned tasks (1+) store their own execution context in the Task struct
//! - When switching tasks, the scheduler swaps contexts with the VM

use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    args::ArgValues,
    asyncio::{CallId, TaskId},
    exception_private::RunError,
    heap::HeapId,
    intern::ExtFunctionId,
    namespace::NamespaceId,
    parse::CodeRange,
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
    /// Current execution state.
    pub state: TaskState,
}

/// Serialized call frame for task storage.
///
/// Similar to `SerializedFrame` but used within the scheduler for task context.
/// Cannot store `&Code` references - uses `FunctionId` to look up code on resume.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SerializedTaskFrame {
    /// Which function's code this frame executes (None = module-level).
    pub function_id: Option<crate::intern::FunctionId>,
    /// Instruction pointer within this frame's bytecode.
    pub ip: usize,
    /// Base index into operand stack for this frame.
    pub stack_base: usize,
    /// Namespace index for this frame's locals.
    pub namespace_idx: NamespaceId,
    /// Captured cells for closures.
    pub cells: Vec<HeapId>,
    /// Call site position (for tracebacks).
    pub call_position: Option<CodeRange>,
}

#[expect(dead_code, reason = "methods used in Phase 5 when scheduler is integrated")]
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
        }
    }

    /// Returns true if this task is ready to execute.
    #[inline]
    pub fn is_ready(&self) -> bool {
        matches!(self.state, TaskState::Ready)
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
    /// The external function being called.
    pub ext_function_id: ExtFunctionId,
    /// Arguments for the function (includes both positional and keyword args).
    pub args: ArgValues,
    /// Task that created this call (for ignoring results if task is cancelled).
    pub creator_task: TaskId,
}

/// Scheduler for managing concurrent async tasks and external call tracking.
///
/// The scheduler is always present (created at VM initialization) to maintain
/// separation of concerns. All async-related state lives here:
/// - Task management (creation, scheduling, completion)
/// - External call ID allocation and tracking
/// - Resolution of pending futures
///
/// # Main Task
///
/// Task 0 is the "main task" which executes using the VM's stack/frames directly.
/// It's always created at scheduler initialization but doesn't store its own context
/// (the VM holds it). Spawned tasks (1+) store their context in the Task struct.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Scheduler {
    /// All tasks (main task at index 0, spawned tasks follow).
    tasks: Vec<Task>,
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
    pending_calls: HashMap<CallId, PendingCallData>,
    /// Maps CallId -> resolved Value for futures that have been resolved.
    /// Entry is removed when the value is consumed by awaiting.
    resolved: HashMap<CallId, Value>,
    /// CallIds that have been awaited (to detect double-await).
    consumed: HashSet<CallId>,
}

#[expect(dead_code, reason = "methods used in Phase 5 when scheduler is integrated")]
impl Scheduler {
    /// Creates a new scheduler with the main task (task 0) in Ready state.
    ///
    /// The main task uses the VM's stack/frames directly and is always present.
    pub fn new() -> Self {
        let main_task = Task::new(TaskId::new(0), None, None);
        Self {
            tasks: vec![main_task],
            ready_queue: VecDeque::from([TaskId::new(0)]),
            current_task: Some(TaskId::new(0)),
            next_task_id: 1,
            next_call_id: 0,
            pending_calls: HashMap::new(),
            resolved: HashMap::new(),
            consumed: HashSet::new(),
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
        &self.tasks[task_id.raw() as usize]
    }

    /// Returns a mutable reference to a task by ID.
    ///
    /// # Panics
    /// Panics if the task ID doesn't exist.
    #[inline]
    pub fn get_task_mut(&mut self, task_id: TaskId) -> &mut Task {
        &mut self.tasks[task_id.raw() as usize]
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

    /// Returns true if a CallId has already been awaited (consumed).
    #[inline]
    pub fn is_consumed(&self, call_id: CallId) -> bool {
        self.consumed.contains(&call_id)
    }

    /// Marks a CallId as consumed (awaited).
    pub fn mark_consumed(&mut self, call_id: CallId) {
        self.consumed.insert(call_id);
    }

    /// Resolves a CallId with a value.
    ///
    /// Stores the value for later retrieval when the future is awaited.
    /// If a task is blocked on this call, it will be unblocked.
    pub fn resolve(&mut self, call_id: CallId, value: Value) {
        // Remove from pending calls
        self.pending_calls.remove(&call_id);

        // Store the resolved value
        self.resolved.insert(call_id, value);

        // Find and unblock any task waiting on this call
        for task in &mut self.tasks {
            if let TaskState::BlockedOnCall(blocked_call_id) = task.state
                && blocked_call_id == call_id
            {
                task.state = TaskState::Ready;
                self.ready_queue.push_back(task.id);
                break;
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
    pub fn block_current_on_gather(&mut self, gather_id: HeapId) {
        if let Some(task_id) = self.current_task {
            let task = self.get_task_mut(task_id);
            task.state = TaskState::BlockedOnGather(gather_id);
        }
    }

    /// Returns all pending (unresolved) CallIds.
    pub fn pending_call_ids(&self) -> Vec<CallId> {
        self.pending_calls.keys().copied().collect()
    }

    /// Returns true if all tasks are blocked or completed.
    ///
    /// This indicates that control should return to the host to resolve
    /// pending external calls.
    pub fn all_tasks_blocked(&self) -> bool {
        self.ready_queue.is_empty()
    }

    /// Returns the number of active (non-completed) tasks.
    #[expect(dead_code, reason = "useful for debugging and future phases")]
    pub fn active_task_count(&self) -> usize {
        self.tasks.iter().filter(|t| !t.is_finished()).count()
    }

    /// Spawns a new task from a coroutine.
    ///
    /// Creates a new task that will execute the given coroutine when scheduled.
    /// The task is added to the ready queue.
    ///
    /// # Arguments
    /// * `coroutine_id` - HeapId of the coroutine to execute
    /// * `gather_id` - Optional HeapId of the GatherFuture this task belongs to
    ///
    /// # Returns
    /// The TaskId of the newly created task.
    pub fn spawn(&mut self, coroutine_id: HeapId, gather_id: Option<HeapId>) -> TaskId {
        let task_id = TaskId::new(self.next_task_id);
        self.next_task_id += 1;

        let task = Task::new(task_id, Some(coroutine_id), gather_id);
        self.tasks.push(task);
        self.ready_queue.push_back(task_id);

        task_id
    }

    /// Gets the next ready task from the queue.
    ///
    /// Returns `None` if no tasks are ready.
    pub fn next_ready_task(&mut self) -> Option<TaskId> {
        self.ready_queue.pop_front()
    }

    /// Adds a task back to the ready queue.
    pub fn make_ready(&mut self, task_id: TaskId) {
        let task = self.get_task_mut(task_id);
        task.state = TaskState::Ready;
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
    pub fn complete_task(&mut self, task_id: TaskId, result: Value) {
        let task = self.get_task_mut(task_id);
        task.state = TaskState::Completed(result);
        // Note: gather wake-up logic will be implemented when gather is fully integrated
    }

    /// Marks a task as failed with an error.
    ///
    /// If the task is part of a gather, collects sibling task IDs for cancellation.
    /// The caller should then call `cancel_task` for each sibling.
    ///
    /// # Returns
    /// A tuple of:
    /// - The gather_id if this task belongs to a gather
    /// - Task IDs of sibling tasks that should be cancelled
    pub fn fail_task(&mut self, task_id: TaskId, error: RunError) -> (Option<HeapId>, Vec<TaskId>) {
        let task = self.get_task_mut(task_id);
        let gather_id = task.gather_id;
        task.state = TaskState::Failed(error);

        // Collect sibling task IDs for cancellation
        let mut siblings = Vec::new();
        if let Some(gid) = gather_id {
            for task in &self.tasks {
                if task.gather_id == Some(gid) && task.id != task_id && !task.is_finished() {
                    siblings.push(task.id);
                }
            }
        }

        (gather_id, siblings)
    }

    /// Cancels a task, cleaning up its resources.
    ///
    /// This marks the task as Failed with a cancellation error and cleans up:
    /// - Stack values
    /// - Exception stack values
    ///
    /// The caller is responsible for cleaning up the task's coroutine on the heap.
    ///
    /// # Arguments
    /// * `task_id` - ID of the task to cancel
    /// * `heap` - Heap for dropping values
    pub fn cancel_task(
        &mut self,
        task_id: TaskId,
        heap: &mut crate::heap::Heap<impl crate::resource::ResourceTracker>,
    ) {
        // Only cancel if not already finished (check before mutating)
        if self.get_task(task_id).is_finished() {
            return;
        }

        // Remove from ready queue if present (do this before getting mutable task reference)
        self.ready_queue.retain(|&id| id != task_id);

        // Now get mutable reference to the task for cleanup
        let task = self.get_task_mut(task_id);

        // Clean up stack values
        for value in std::mem::take(&mut task.stack) {
            value.drop_with_heap(heap);
        }

        // Clean up exception stack values
        for value in std::mem::take(&mut task.exception_stack) {
            value.drop_with_heap(heap);
        }

        // Mark as failed with a cancellation error
        task.state = TaskState::Failed(
            crate::exception_private::SimpleException::new_msg(
                crate::exception_private::ExcType::RuntimeError,
                "task was cancelled",
            )
            .into(),
        );
    }

    /// Fails the task blocked on a specific CallId with an error.
    ///
    /// Used when an external function returns an error via `FutureSnapshot::resume`.
    /// Finds the task blocked on this CallId and fails it with the given error.
    ///
    /// # Returns
    /// A tuple of (task_id, gather_id, sibling_task_ids) if a task was found,
    /// or None if no task was blocked on this CallId.
    pub fn fail_for_call(&mut self, call_id: CallId, error: RunError) -> Option<(TaskId, Option<HeapId>, Vec<TaskId>)> {
        // Find the task blocked on this call
        let task_id = self.tasks.iter().find_map(|task| {
            if let TaskState::BlockedOnCall(blocked_call_id) = task.state
                && blocked_call_id == call_id
            {
                return Some(task.id);
            }
            None
        })?;

        // Fail the task and get sibling info
        let (gather_id, siblings) = self.fail_task(task_id, error);
        Some((task_id, gather_id, siblings))
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
        matches!(self.tasks.get(task_id.raw() as usize), Some(task) if matches!(task.state, TaskState::Failed(_)))
    }

    /// Cleans up resources when dropping the scheduler.
    ///
    /// Drops any pending call arguments and resolved values.
    #[expect(dead_code, reason = "used when ref-count-panic is enabled")]
    pub fn cleanup(&mut self, heap: &mut crate::heap::Heap<impl crate::resource::ResourceTracker>) {
        // Drop pending call arguments
        for (_, data) in std::mem::take(&mut self.pending_calls) {
            data.args.drop_with_heap(heap);
        }
        // Drop resolved values
        for (_, value) in std::mem::take(&mut self.resolved) {
            value.drop_with_heap(heap);
        }
        // Drop task stack/exception values
        for task in &mut self.tasks {
            for value in std::mem::take(&mut task.stack) {
                value.drop_with_heap(heap);
            }
            for value in std::mem::take(&mut task.exception_stack) {
                value.drop_with_heap(heap);
            }
            // Drop completed task results
            if let TaskState::Completed(value) = std::mem::replace(&mut task.state, TaskState::Ready) {
                value.drop_with_heap(heap);
            }
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
