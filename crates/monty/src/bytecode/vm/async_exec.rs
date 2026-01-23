//! Async execution support for the VM.
//!
//! This module contains all async-related methods for the VM including:
//! - Awaiting coroutines, external futures, and gather futures
//! - Task scheduling and context switching
//! - Task completion and failure handling
//! - External future resolution

use super::{CallFrame, GetAwaitableResult, VM};
use crate::{
    args::ArgValues,
    asyncio::{CallId, CoroutineState, TaskId},
    bytecode::vm::scheduler::{PendingCallData, SerializedTaskFrame, TaskState},
    exception_private::{ExcType, RunError, SimpleException},
    heap::{HeapData, HeapId},
    intern::{ExtFunctionId, FunctionId},
    io::PrintWriter,
    resource::ResourceTracker,
    types::{List, PyTrait},
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Executes the GetAwaitable opcode.
    ///
    /// Pops the awaitable from the stack and handles it based on its type:
    /// - `Coroutine`: validates state is New, then pushes a frame to execute it
    /// - `ExternalFuture`: not yet implemented (requires scheduler from Phase 4)
    /// - `GatherFuture`: not yet implemented (requires scheduler from Phase 4)
    ///
    /// Returns `GetAwaitableResult` indicating what action the VM should take.
    pub(super) fn exec_get_awaitable(&mut self) -> Result<GetAwaitableResult, RunError> {
        #[cfg_attr(not(feature = "ref-count-panic"), expect(unused_mut))]
        let mut awaitable = self.pop();

        match awaitable {
            Value::Ref(heap_id) => {
                // Check what kind of heap object this is
                let heap_data = self.heap.get(heap_id);
                match heap_data {
                    HeapData::Coroutine(coro) => {
                        // Check if coroutine can be awaited (must be New)
                        if coro.state != CoroutineState::New {
                            awaitable.drop_with_heap(self.heap);
                            return Err(SimpleException::new_msg(
                                ExcType::RuntimeError,
                                "cannot reuse already awaited coroutine",
                            )
                            .into());
                        }

                        // Extract coroutine data before mutating
                        let func_id = coro.func_id;
                        let namespace_values: Vec<Value> = coro.namespace.iter().map(Value::copy_for_extend).collect();
                        let frame_cells: Vec<HeapId> = coro.frame_cells.clone();

                        // Increment refcounts for copied values
                        for value in &namespace_values {
                            if let Value::Ref(id) = value {
                                self.heap.inc_ref(*id);
                            }
                        }
                        for &cell_id in &frame_cells {
                            self.heap.inc_ref(cell_id);
                        }

                        // Mark coroutine as Running
                        if let HeapData::Coroutine(coro_mut) = self.heap.get_mut(heap_id) {
                            coro_mut.state = CoroutineState::Running;
                        }

                        // Create namespace and push frame
                        self.start_coroutine_frame(func_id, namespace_values, frame_cells)?;

                        // Drop the coroutine reference (we've extracted what we need)
                        awaitable.drop_with_heap(self.heap);

                        Ok(GetAwaitableResult::FramePushed)
                    }
                    HeapData::GatherFuture(gather) => {
                        // Check if already being waited on (double-await)
                        if gather.waiter.is_some() {
                            awaitable.drop_with_heap(self.heap);
                            return Err(SimpleException::new_msg(
                                ExcType::RuntimeError,
                                "cannot reuse already awaited gather",
                            )
                            .into());
                        }

                        // If no coroutines to gather, return empty list immediately
                        if gather.task_count() == 0 {
                            awaitable.drop_with_heap(self.heap);
                            let list_id = self.heap.allocate(HeapData::List(List::new(vec![])))?;
                            return Ok(GetAwaitableResult::ValueReady(Value::Ref(list_id)));
                        }

                        // Spawn tasks for each coroutine
                        let coroutine_ids: Vec<HeapId> = gather.coroutine_ids.clone();

                        // Set waiter before spawning (creates scheduler if needed)
                        let current_task = self.scheduler_mut().current_task_id();
                        if let HeapData::GatherFuture(gather_mut) = self.heap.get_mut(heap_id) {
                            gather_mut.waiter = current_task;
                        }

                        // Spawn all coroutines as tasks (track gather_id for cancellation)
                        let mut task_ids = Vec::with_capacity(coroutine_ids.len());
                        for coro_id in &coroutine_ids {
                            let task_id = self.scheduler_mut().spawn(*coro_id, Some(heap_id));
                            task_ids.push(task_id);
                        }

                        // Store task IDs in the gather
                        if let HeapData::GatherFuture(gather_mut) = self.heap.get_mut(heap_id) {
                            gather_mut.task_ids = task_ids;
                        }

                        // Block current task on this gather
                        self.scheduler_mut().block_current_on_gather(heap_id);

                        // Consume the awaitable without decrementing refcount - the GatherFuture
                        // must stay alive for result collection. It will be dec_ref'd when
                        // the gather completes (in handle_task_completion).
                        #[cfg(feature = "ref-count-panic")]
                        awaitable.dec_ref_forget();

                        // Switch to next ready task (spawned tasks) or yield
                        self.switch_or_yield()
                    }
                    _ => {
                        // Not an awaitable type
                        let type_name = awaitable.py_type(self.heap);
                        awaitable.drop_with_heap(self.heap);
                        Err(ExcType::type_error(format!(
                            "object {type_name} can't be used in 'await' expression"
                        )))
                    }
                }
            }
            Value::ExternalFuture(call_id) => {
                // Check if already consumed (double-await error)
                // If no scheduler exists, call can't have been consumed
                if self.scheduler.as_ref().is_some_and(|s| s.is_consumed(call_id)) {
                    return Err(
                        SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited future").into(),
                    );
                }

                // Mark as consumed (creates scheduler if needed)
                self.scheduler_mut().mark_consumed(call_id);

                // Check if the future is already resolved
                if let Some(value) = self.scheduler_mut().take_resolved(call_id) {
                    Ok(GetAwaitableResult::ValueReady(value))
                } else {
                    // Block current task on this call
                    self.scheduler_mut().block_current_on_call(call_id);

                    // Switch to next ready task or yield to host
                    self.switch_or_yield()
                }
            }
            _ => {
                // Not an awaitable type
                let type_name = awaitable.py_type(self.heap);
                awaitable.drop_with_heap(self.heap);
                Err(ExcType::type_error(format!(
                    "object {type_name} can't be used in 'await' expression"
                )))
            }
        }
    }

    /// Starts execution of a coroutine by pushing a new frame.
    ///
    /// Registers the pre-bound namespace with the VM's Namespaces and pushes
    /// a new frame to execute the coroutine's function body.
    fn start_coroutine_frame(
        &mut self,
        func_id: FunctionId,
        namespace_values: Vec<Value>,
        frame_cells: Vec<HeapId>,
    ) -> Result<(), RunError> {
        let call_position = self.current_position();
        let func = self.interns.get_function(func_id);

        // Register the pre-bound namespace
        let namespace_idx = self.namespaces.register_prebuilt(namespace_values, self.heap)?;

        // Push frame to execute the coroutine
        self.frames.push(CallFrame::new_function(
            &func.code,
            self.stack.len(),
            namespace_idx,
            func_id,
            frame_cells,
            Some(call_position),
        ));

        Ok(())
    }

    /// Attempts to switch to the next ready task or yields if all tasks are blocked.
    ///
    /// This method is called when the current task blocks (e.g., awaiting an unresolved
    /// future or gather). It performs task context switching:
    /// 1. Saves current VM context to the current task in the scheduler
    /// 2. Gets the next ready task from the scheduler
    /// 3. Loads that task's context into the VM (or initializes a new task from its coroutine)
    ///
    /// Returns `Yield(pending_calls)` if no ready tasks (all blocked), or continues
    /// the run loop if a task was switched to.
    fn switch_or_yield(&mut self) -> Result<GetAwaitableResult, RunError> {
        // Save current task context to scheduler
        if let Some(current_task_id) = self.scheduler().current_task_id() {
            self.save_task_context(current_task_id);
        }

        // Get next ready task
        if let Some(next_task_id) = self.scheduler_mut().next_ready_task() {
            self.scheduler_mut().set_current_task(Some(next_task_id));

            // Load or initialize the next task's context
            self.load_or_init_task(next_task_id)?;

            // Continue execution - return FramePushed to reload cache and continue run loop
            Ok(GetAwaitableResult::FramePushed)
        } else {
            // No ready tasks - yield control to host
            let pending = self.scheduler().pending_call_ids();
            Ok(GetAwaitableResult::Yield(pending))
        }
    }

    /// Handles completion of a spawned task.
    ///
    /// Called when a spawned task's coroutine returns. This:
    /// 1. Marks the task as completed in the scheduler
    /// 2. If the task belongs to a gather, stores the result and checks if gather is complete
    /// 3. If gather is complete, unblocks the waiter and provides the collected results
    /// 4. Otherwise, switches to the next ready task
    pub(super) fn handle_task_completion(&mut self, result: Value) -> Result<GetAwaitableResult, RunError> {
        let task_id = self
            .scheduler()
            .current_task_id()
            .expect("handle_task_completion called without current task");

        // Get task's gather_id and coroutine_id before marking complete
        let task = self.scheduler().get_task(task_id);
        let gather_id = task.gather_id;
        let coroutine_id = task.coroutine_id;

        // Mark coroutine as completed
        if let Some(coro_id) = coroutine_id
            && let HeapData::Coroutine(coro) = self.heap.get_mut(coro_id)
        {
            coro.state = CoroutineState::Completed;
        }

        // Mark task as completed and store result
        self.scheduler_mut().complete_task(task_id, result.copy_for_extend());
        if let Value::Ref(id) = &result {
            self.heap.inc_ref(*id);
        }

        // If task belongs to a gather, check if gather is complete
        if let Some(gid) = gather_id {
            // Extract gather data before doing any heap mutations
            let (task_ids, waiter) = if let HeapData::GatherFuture(gather) = self.heap.get(gid) {
                (gather.task_ids.clone(), gather.waiter)
            } else {
                (vec![], None)
            };

            if !task_ids.is_empty() {
                // Check if all tasks are complete
                let all_complete = task_ids.iter().all(|tid| {
                    matches!(
                        self.scheduler().get_task(*tid).state,
                        TaskState::Completed(_) | TaskState::Failed(_)
                    )
                });

                if all_complete {
                    // First check if any task failed
                    let failed_task = task_ids
                        .iter()
                        .find(|tid| matches!(self.scheduler().get_task(**tid).state, TaskState::Failed(_)));

                    if let Some(&failed_tid) = failed_task {
                        // Get the error from the failed task
                        let task = self.scheduler_mut().get_task_mut(failed_tid);
                        if let TaskState::Failed(err) = std::mem::replace(&mut task.state, TaskState::Ready) {
                            // Clean up resources before propagating error
                            result.drop_with_heap(self.heap);
                            self.heap.dec_ref(gid);

                            // Switch to waiter so error is raised in its context
                            if let Some(waiter_id) = waiter {
                                self.cleanup_current_frames();
                                self.stack.clear();
                                self.scheduler_mut().set_current_task(Some(waiter_id));
                                self.load_or_init_task(waiter_id)?;
                            }

                            return Err(err);
                        }
                    }

                    // Collect results in order (no failures) and increment refcounts
                    let mut results = Vec::with_capacity(task_ids.len());
                    let mut ref_ids_to_inc = Vec::new();
                    for tid in &task_ids {
                        let task_state = &self.scheduler().get_task(*tid).state;
                        match task_state {
                            TaskState::Completed(v) => {
                                results.push(v.copy_for_extend());
                                if let Value::Ref(id) = v {
                                    ref_ids_to_inc.push(*id);
                                }
                            }
                            _ => {
                                unreachable!("task not complete but all_complete is true and no failures")
                            }
                        }
                    }

                    // Now increment refcounts (after scheduler borrow ends)
                    for id in ref_ids_to_inc {
                        self.heap.inc_ref(id);
                    }

                    // Create result list
                    let list_id = self.heap.allocate(HeapData::List(List::new(results)))?;

                    // Clean up gather - drop the original result since we copied it
                    result.drop_with_heap(self.heap);

                    // Release the GatherFuture - this will cascade to release coroutines
                    self.heap.dec_ref(gid);

                    // Unblock waiter and switch to it
                    if let Some(waiter_id) = waiter {
                        self.scheduler_mut().make_ready(waiter_id);
                        // Clear current task's state since it's done
                        self.cleanup_current_frames();
                        self.stack.clear();
                        // Switch to waiter
                        self.scheduler_mut().set_current_task(Some(waiter_id));
                        self.load_or_init_task(waiter_id)?;
                        // Push the result onto the waiter's stack
                        self.push(Value::Ref(list_id));
                        return Ok(GetAwaitableResult::FramePushed);
                    }

                    // No waiter (shouldn't happen but handle gracefully)
                    return Ok(GetAwaitableResult::ValueReady(Value::Ref(list_id)));
                }
            }
        }

        // Drop the result (it's stored in the task state now)
        result.drop_with_heap(self.heap);

        // Gather not complete or no gather - switch to next task
        // Clear current task's state since it's done
        self.cleanup_current_frames();
        self.stack.clear();
        self.scheduler_mut().set_current_task(None);

        // Get next ready task
        if let Some(next_task_id) = self.scheduler_mut().next_ready_task() {
            self.scheduler_mut().set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
            Ok(GetAwaitableResult::FramePushed)
        } else {
            // No ready tasks - yield to host
            let pending = self.scheduler().pending_call_ids();
            Ok(GetAwaitableResult::Yield(pending))
        }
    }

    /// Returns true if the current task is a spawned task (not main).
    ///
    /// Used by exception handling to determine if an unhandled exception
    /// should fail the task rather than propagate out.
    #[inline]
    pub(super) fn is_spawned_task(&self) -> bool {
        self.scheduler
            .as_ref()
            .and_then(super::scheduler::Scheduler::current_task_id)
            .is_some_and(|id: TaskId| !id.is_main())
    }

    /// Handles failure of a spawned task due to an unhandled exception.
    ///
    /// Called when an exception escapes all frames in a spawned task. This:
    /// 1. Marks the task as failed in the scheduler
    /// 2. If the task belongs to a gather, cleans up and propagates to waiter
    /// 3. Otherwise, switches to the next ready task
    ///
    /// # Returns
    /// - `Ok(())` - Switched to next task, continue execution
    /// - `Err(error)` - Switched to waiter, handle error in waiter's context
    ///
    /// # Panics
    /// Panics if called for the main task.
    pub(super) fn handle_task_failure(&mut self, error: RunError) -> Result<(), RunError> {
        let task_id = self
            .scheduler()
            .current_task_id()
            .expect("handle_task_failure called without current task");
        debug_assert!(!task_id.is_main(), "handle_task_failure called for main task");

        // Get task's gather_id before marking failed
        let gather_id = self.scheduler().get_task(task_id).gather_id;

        // If part of a gather, propagate error to waiter
        if let Some(gid) = gather_id {
            let waiter = if let HeapData::GatherFuture(gather) = self.heap.get(gid) {
                gather.waiter
            } else {
                None
            };

            // Mark task as failed (need to do this before getting siblings)
            let (_, siblings) = self.scheduler_mut().fail_task(task_id, error);

            // Cancel sibling tasks
            // Use direct field access to avoid borrow conflicts with heap/namespaces
            for sibling_id in siblings {
                self.scheduler.as_mut().expect("scheduler must exist").cancel_task(
                    sibling_id,
                    self.heap,
                    self.namespaces,
                );
            }

            // Clean up the gather
            self.heap.dec_ref(gid);

            // Switch to waiter and propagate the error
            if let Some(waiter_id) = waiter {
                // Properly clean up current task's frames (namespaces and cells)
                self.cleanup_current_frames();
                self.stack.clear();
                self.scheduler_mut().set_current_task(Some(waiter_id));
                self.load_or_init_task(waiter_id)?;
                // Get error back from task state to return
                let task = self.scheduler_mut().get_task_mut(task_id);
                if let TaskState::Failed(err) = std::mem::replace(&mut task.state, TaskState::Ready) {
                    return Err(err);
                }
            }
        } else {
            // No gather - just mark task as failed
            self.scheduler_mut().fail_task(task_id, error);
        }

        // No gather or no waiter - switch to next task
        self.cleanup_current_frames();
        self.stack.clear();
        self.scheduler_mut().set_current_task(None);

        if let Some(next_task_id) = self.scheduler_mut().next_ready_task() {
            self.scheduler_mut().set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
        }
        // If no ready tasks, frames will be empty and run loop will yield

        Ok(())
    }

    /// Saves the current VM context into the given task in the scheduler.
    ///
    /// Serializes frames, moves stack/exception_stack, and stores instruction_ip.
    fn save_task_context(&mut self, task_id: TaskId) {
        // Collect data before borrowing scheduler to avoid borrow conflicts
        let frames: Vec<SerializedTaskFrame> = self
            .frames
            .drain(..)
            .map(|f| SerializedTaskFrame {
                function_id: f.function_id,
                ip: f.ip,
                stack_base: f.stack_base,
                namespace_idx: f.namespace_idx,
                cells: f.cells,
                call_position: f.call_position,
            })
            .collect();
        let stack = std::mem::take(&mut self.stack);
        let exception_stack = std::mem::take(&mut self.exception_stack);
        let instruction_ip = self.instruction_ip;

        // Now assign to task
        let task = self.scheduler_mut().get_task_mut(task_id);
        task.frames = frames;
        task.stack = stack;
        task.exception_stack = exception_stack;
        task.instruction_ip = instruction_ip;
    }

    /// Loads an existing task's context or initializes a new task from its coroutine.
    ///
    /// If the task has stored frames, restores them into the VM.
    /// If the task has a coroutine_id but no frames, starts the coroutine.
    fn load_or_init_task(&mut self, task_id: TaskId) -> Result<(), RunError> {
        // Extract data from task before assigning to self to avoid borrow conflicts
        let (frames, stack, exception_stack, instruction_ip, coroutine_id) = {
            let task = self.scheduler_mut().get_task_mut(task_id);
            (
                std::mem::take(&mut task.frames),
                std::mem::take(&mut task.stack),
                std::mem::take(&mut task.exception_stack),
                task.instruction_ip,
                task.coroutine_id,
            )
        };

        if !frames.is_empty() {
            // Task has existing context - restore it
            self.stack = stack;
            self.exception_stack = exception_stack;
            self.instruction_ip = instruction_ip;

            // Reconstruct CallFrames from serialized form
            self.frames = frames
                .into_iter()
                .map(|sf| {
                    let code = match sf.function_id {
                        Some(func_id) => &self.interns.get_function(func_id).code,
                        None => {
                            // This happens for the main task's module-level code
                            self.module_code.expect("module_code not set for main task frame")
                        }
                    };
                    CallFrame {
                        code,
                        ip: sf.ip,
                        stack_base: sf.stack_base,
                        namespace_idx: sf.namespace_idx,
                        function_id: sf.function_id,
                        cells: sf.cells,
                        call_position: sf.call_position,
                    }
                })
                .collect();
        } else if let Some(coro_id) = coroutine_id {
            // New task - start from coroutine
            self.init_task_from_coroutine(coro_id)?;
        } else {
            // This shouldn't happen - task with no frames and no coroutine
            panic!("task has no frames and no coroutine_id");
        }

        Ok(())
    }

    /// Initializes the VM state to run a coroutine for a spawned task.
    ///
    /// Similar to exec_get_awaitable's coroutine handling, but for task initialization.
    fn init_task_from_coroutine(&mut self, coroutine_id: HeapId) -> Result<(), RunError> {
        // Get coroutine data
        let heap_data = self.heap.get(coroutine_id);
        let HeapData::Coroutine(coro) = heap_data else {
            panic!("task coroutine_id doesn't point to a Coroutine")
        };

        // Check state
        if coro.state != CoroutineState::New {
            return Err(
                SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited coroutine").into(),
            );
        }

        // Extract coroutine data
        let func_id = coro.func_id;
        let namespace_values: Vec<Value> = coro.namespace.iter().map(Value::copy_for_extend).collect();
        let frame_cells: Vec<HeapId> = coro.frame_cells.clone();

        // Increment refcounts for copied values
        for value in &namespace_values {
            if let Value::Ref(id) = value {
                self.heap.inc_ref(*id);
            }
        }
        for &cell_id in &frame_cells {
            self.heap.inc_ref(cell_id);
        }

        // Mark coroutine as Running
        if let HeapData::Coroutine(coro_mut) = self.heap.get_mut(coroutine_id) {
            coro_mut.state = CoroutineState::Running;
        }

        // Create namespace and push frame directly (can't use start_coroutine_frame
        // because that needs a current frame for call_position, but spawned tasks
        // don't have a parent frame - the coroutine is the root)
        let func = self.interns.get_function(func_id);
        let namespace_idx = self.namespaces.register_prebuilt(namespace_values, self.heap)?;
        self.frames.push(CallFrame::new_function(
            &func.code,
            self.stack.len(),
            namespace_idx,
            func_id,
            frame_cells,
            None, // No call position - this is the root frame for a spawned task
        ));

        Ok(())
    }

    /// Resolves an external future with a value.
    ///
    /// Called by the host when an async external call completes.
    /// Stores the result in the scheduler, which will unblock any task
    /// waiting on this CallId.
    ///
    /// If the task that created this call has been cancelled or failed,
    /// the result is silently ignored and the value is dropped.
    pub fn resolve_future(&mut self, call_id: CallId, value: Value) {
        // Check if the creator task has been cancelled/failed
        if let Some(creator_task) = self.scheduler().get_pending_call_creator(call_id)
            && self.scheduler().is_task_failed(creator_task)
        {
            // Task was cancelled - silently ignore the result and drop the value
            value.drop_with_heap(self.heap);
            return;
        }
        self.scheduler_mut().resolve(call_id, value);
    }

    /// Fails an external future with an error.
    ///
    /// Called by the host when an async external call fails with an exception.
    /// Finds the task blocked on this CallId and fails it with the error.
    /// If the task is part of a gather, cancels sibling tasks.
    ///
    /// # Returns
    /// `true` if a task was found and failed, `false` if no task was blocked on this CallId.
    pub fn fail_future(&mut self, call_id: CallId, error: RunError) -> bool {
        if let Some((_, _, siblings)) = self.scheduler_mut().fail_for_call(call_id, error) {
            // Cancel sibling tasks if this task was part of a gather
            // Use direct field access to avoid borrow conflicts with heap/namespaces
            for sibling_id in siblings {
                self.scheduler.as_mut().expect("scheduler must exist").cancel_task(
                    sibling_id,
                    self.heap,
                    self.namespaces,
                );
            }
            true
        } else {
            false
        }
    }

    /// Adds pending call data for an external function call.
    ///
    /// Called by `run_pending()` when the host chooses async resolution.
    /// This stores the call data in the scheduler so we can:
    /// 1. Track which task created the call (to ignore results if cancelled)
    /// 2. Return pending call info when all tasks are blocked
    ///
    /// Note: The args are empty because the host already has them from the
    /// `FunctionCall` return value. We only need to track the creator task.
    pub fn add_pending_call(&mut self, call_id: CallId, ext_function_id: ExtFunctionId) {
        let current_task = self
            .scheduler
            .as_ref()
            .and_then(super::scheduler::Scheduler::current_task_id)
            .unwrap_or(TaskId::new(0));
        self.scheduler_mut().add_pending_call(
            call_id,
            PendingCallData {
                ext_function_id,
                args: ArgValues::Empty,
                creator_task: current_task,
            },
        );
    }
}
