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

use std::collections::VecDeque;

use ahash::{AHashMap, AHashSet};

use crate::{
    args::ArgValues,
    asyncio::{CallId, TaskId},
    exception_private::RunError,
    heap::{DropWithHeap, HeapId},
    namespace::{GLOBAL_NS_IDX, NamespaceId, Namespaces},
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
    /// Index in the gather's results where this task's result should be stored.
    /// Only set for tasks spawned by gather.
    pub gather_result_idx: Option<usize>,
    /// Current execution state.
    pub state: TaskState,
    /// CallId that unblocked this task (set when task transitions from Blocked to Ready).
    /// Used to retrieve the resolved value when the task resumes.
    pub unblocked_by: Option<CallId>,
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

impl Task {
    /// Creates a new task in the Ready state.
    ///
    /// # Arguments
    /// * `id` - Unique task identifier
    /// * `coroutine_id` - Optional HeapId of the coroutine being executed
    /// * `gather_id` - Optional HeapId of the GatherFuture this task belongs to
    pub fn new(
        id: TaskId,
        coroutine_id: Option<HeapId>,
        gather_id: Option<HeapId>,
        gather_result_idx: Option<usize>,
    ) -> Self {
        Self {
            id,
            frames: Vec::new(),
            stack: Vec::new(),
            exception_stack: Vec::new(),
            instruction_ip: 0,
            coroutine_id,
            gather_id,
            gather_result_idx,
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
    /// The external function being called.
    // pub ext_function_id: ExtFunctionId,
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
    pending_calls: AHashMap<CallId, PendingCallData>,
    /// Maps CallId -> resolved Value for futures that have been resolved.
    /// Entry is removed when the value is consumed by awaiting.
    resolved: AHashMap<CallId, Value>,
    /// CallIds that have been awaited (to detect double-await).
    consumed: AHashSet<CallId>,
    /// Maps CallId -> (gather_heap_id, result_index) for gathers waiting on external futures.
    /// When a CallId is resolved, the result is stored in the gather's results at the given index.
    gather_waiters: AHashMap<CallId, (HeapId, usize)>,
}

impl Scheduler {
    /// Creates a new scheduler with the main task (task 0) as current.
    ///
    /// The main task uses the VM's stack/frames directly and is always present.
    /// It starts as the current task (not in the ready queue) since it runs
    /// immediately without needing to be scheduled.
    pub fn new() -> Self {
        let mut main_task = Task::new(TaskId::default(), None, None, None);
        // Main task starts Running, not Ready (it's the current task, not waiting)
        main_task.state = TaskState::Ready; // Will be set properly when it blocks
        Self {
            tasks: vec![main_task],
            ready_queue: VecDeque::new(), // Main task is current, not in ready queue
            current_task: Some(TaskId::default()),
            next_task_id: 1,
            next_call_id: 0,
            pending_calls: AHashMap::new(),
            resolved: AHashMap::new(),
            consumed: AHashSet::new(),
            gather_waiters: AHashMap::new(),
        }
    }

    /// Returns the currently executing task ID.
    ///
    /// Returns `None` only during task switching operations.
    #[inline]
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.current_task
    }

    /// Returns the total number of tasks (including main task).
    #[inline]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
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

    /// Sets the next call ID counter.
    ///
    /// Used when lazily creating the scheduler to inherit the call ID counter
    /// from the VM, ensuring call IDs remain unique across the transition.
    pub fn set_next_call_id(&mut self, id: u32) {
        self.next_call_id = id;
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
    /// at the specified index.
    pub fn register_gather_for_call(&mut self, call_id: CallId, gather_id: HeapId, result_index: usize) {
        self.gather_waiters.insert(call_id, (gather_id, result_index));
    }

    /// Returns gather info if a gather is waiting on this CallId.
    ///
    /// Returns (gather_heap_id, result_index) if found, None otherwise.
    /// Removes the entry from gather_waiters.
    pub fn take_gather_waiter(&mut self, call_id: CallId) -> Option<(HeapId, usize)> {
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
        let task = &mut self.tasks[task_id.raw() as usize];
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
    /// # Arguments
    /// * `coroutine_id` - HeapId of the coroutine to execute
    /// * `gather_id` - Optional HeapId of the GatherFuture this task belongs to
    /// * `gather_result_idx` - Optional index in the gather's results for this task
    ///
    /// # Returns
    /// The TaskId of the newly created task.
    pub fn spawn(
        &mut self,
        coroutine_id: HeapId,
        gather_id: Option<HeapId>,
        gather_result_idx: Option<usize>,
    ) -> TaskId {
        let task_id = TaskId::new(self.next_task_id);
        self.next_task_id += 1;

        let task = Task::new(task_id, Some(coroutine_id), gather_id, gather_result_idx);
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
    /// If the task is part of a gather, returns the gather_id so the caller
    /// can collect siblings from `GatherFuture.task_ids` on the heap.
    ///
    /// # Returns
    /// The gather_id if this task belongs to a gather (for sibling lookup).
    pub fn fail_task(&mut self, task_id: TaskId, error: RunError) -> Option<HeapId> {
        let task = self.get_task_mut(task_id);
        let gather_id = task.gather_id;
        task.state = TaskState::Failed(error);
        gather_id
    }

    /// Cancels a task, cleaning up its resources.
    ///
    /// This marks the task as Failed with a cancellation error and cleans up:
    /// - Stack values
    /// - Exception stack values
    /// - Frame cell references
    /// - Frame namespaces
    /// - Nested gathers (if the task was blocked on one)
    /// - Completed task results (if task finished before cancellation)
    ///
    /// The caller is responsible for cleaning up the task's coroutine on the heap.
    ///
    /// # Arguments
    /// * `task_id` - ID of the task to cancel
    /// * `heap` - Heap for dropping values
    /// * `namespaces` - VM namespaces for cleaning up frame namespaces
    pub fn cancel_task(
        &mut self,
        task_id: TaskId,
        heap: &mut crate::heap::Heap<impl crate::resource::ResourceTracker>,
        namespaces: &mut Namespaces,
    ) {
        // If task already finished, clean up its result value and return
        if self.get_task(task_id).is_finished() {
            let task = self.get_task_mut(task_id);
            if let TaskState::Completed(value) = std::mem::replace(&mut task.state, TaskState::Ready) {
                value.drop_with_heap(heap);
            }
            // Note: Failed tasks don't have values to clean up (RunError doesn't contain Values)
            return;
        }

        // Remove from ready queue if present (do this before getting mutable task reference)
        self.ready_queue.retain(|&id| id != task_id);

        // Check if task is blocked on a gather and get the gather info before mutating task
        let inner_gather_info = {
            let task = self.get_task(task_id);
            if let TaskState::BlockedOnGather(gather_id) = task.state {
                // Get inner gather's task IDs from heap
                if let crate::heap::HeapData::GatherFuture(gather) = heap.get(gather_id) {
                    Some((gather_id, gather.task_ids.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        };

        // Recursively cancel inner gather's tasks first
        if let Some((inner_gather_id, inner_task_ids)) = inner_gather_info {
            for inner_task_id in inner_task_ids {
                self.cancel_task(inner_task_id, heap, namespaces);
            }

            // Cleanup the inner GatherFuture - extract data first to avoid borrow conflict
            let (items, results) = if let crate::heap::HeapData::GatherFuture(gather) = heap.get_mut(inner_gather_id) {
                (std::mem::take(&mut gather.items), std::mem::take(&mut gather.results))
            } else {
                (vec![], vec![])
            };

            // Now cleanup the extracted data with mutable heap access
            for item in items {
                if let crate::asyncio::GatherItem::Coroutine(coro_id) = item {
                    heap.dec_ref(coro_id);
                }
            }
            for value in results.into_iter().flatten() {
                value.drop_with_heap(heap);
            }

            // Dec_ref the gather itself
            heap.dec_ref(inner_gather_id);
        }

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

        // Restore this task's depth contribution before dropping namespaces,
        // since save_task_context subtracted it and drop_with_heap will decrement.
        let task_depth = task.frames.len();
        let global_depth = heap.get_recursion_depth();
        heap.set_recursion_depth(global_depth + task_depth);

        // Clean up frame cell references and namespaces
        for frame in std::mem::take(&mut task.frames) {
            for cell_id in frame.cells {
                heap.dec_ref(cell_id);
            }
            // Clean up the namespace (but not the global namespace)
            if frame.namespace_idx != GLOBAL_NS_IDX {
                namespaces.drop_with_heap(frame.namespace_idx, heap);
            }
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
    /// Uses `pending_calls` for O(1) lookup of the blocked task.
    ///
    /// # Returns
    /// A tuple of (task_id, gather_id) if a task was found,
    /// or None if no task was blocked on this CallId.
    /// Callers should get siblings from `GatherFuture.task_ids` if gather_id is Some.
    pub fn fail_for_call(&mut self, call_id: CallId, error: RunError) -> Option<(TaskId, Option<HeapId>)> {
        // Get blocked task from pending_calls (O(1) lookup)
        let task_id = self.pending_calls.remove(&call_id)?.creator_task;
        let gather_id = self.fail_task(task_id, error);
        Some((task_id, gather_id))
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
    /// Drops any pending call arguments, resolved values, and task state.
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
