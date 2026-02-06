//! Async execution support for the VM.
//!
//! This module contains all async-related methods for the VM including:
//! - Awaiting coroutines, external futures, and gather futures
//! - Task scheduling and context switching
//! - Task completion and failure handling
//! - External future resolution

use super::{AwaitResult, CallFrame, VM};
use crate::{
    InvalidInputError, MontyObject,
    args::ArgValues,
    asyncio::{CallId, CoroutineState, GatherItem, TaskId},
    bytecode::vm::scheduler::{PendingCallData, Scheduler, SerializedTaskFrame, TaskState},
    exception_private::{ExcType, RunError, SimpleException},
    heap::{HeapData, HeapId},
    intern::FunctionId,
    io::PrintWriter,
    resource::ResourceTracker,
    types::{List, PyTrait},
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Gets or creates the scheduler for async operations.
    ///
    /// The scheduler is created lazily on first use to avoid allocations for
    /// synchronous code paths. If a scheduler already exists, returns it.
    /// If one doesn't exist, creates a new one, transferring the `next_call_id`
    /// counter so that call IDs remain unique.
    #[inline]
    pub(super) fn get_or_create_scheduler(&mut self) -> &mut Scheduler {
        let next_call_id = self.next_call_id;
        self.scheduler.get_or_insert_with(|| {
            let mut scheduler = Scheduler::new();
            // Transfer the call ID counter to maintain uniqueness
            scheduler.set_next_call_id(next_call_id);
            scheduler
        })
    }

    /// Returns a mutable reference to the scheduler.
    ///
    /// # Panics
    /// Panics if the scheduler hasn't been created yet. Only use this in code
    /// paths where async operations have already been initiated (after
    /// `get_or_create_scheduler` has been called at least once).
    #[inline]
    pub(super) fn scheduler_mut(&mut self) -> &mut Scheduler {
        self.scheduler.as_mut().expect("scheduler must exist in async context")
    }

    /// Returns a reference to the scheduler (read-only access).
    ///
    /// # Panics
    /// Panics if the scheduler hasn't been created yet. Only use this in code
    /// paths where async operations have already been initiated.
    #[inline]
    pub(super) fn scheduler(&self) -> &Scheduler {
        self.scheduler.as_ref().expect("scheduler must exist in async context")
    }

    /// Executes the Await opcode.
    ///
    /// Pops the awaitable from the stack and handles it based on its type:
    /// - `Coroutine`: validates state is New, then pushes a frame to execute it
    /// - `ExternalFuture`: blocks until resolved or yields if not ready
    /// - `GatherFuture`: spawns tasks for coroutines and tracks external futures
    ///
    /// Returns `AwaitResult` indicating what action the VM should take.
    pub(super) fn exec_get_awaitable(&mut self) -> Result<AwaitResult, RunError> {
        let awaitable = self.pop();

        match awaitable {
            Value::Ref(heap_id) => {
                // Check what kind of heap object this is and dispatch to handler
                // We check the type first, then call the handler with just the heap_id
                // to avoid borrow conflicts between heap access and &mut self
                let heap_data_type = match self.heap.get(heap_id) {
                    HeapData::Coroutine(_) => Some(AwaitableType::Coroutine),
                    HeapData::GatherFuture(_) => Some(AwaitableType::GatherFuture),
                    _ => None,
                };

                match heap_data_type {
                    Some(AwaitableType::Coroutine) => self.await_coroutine(heap_id, awaitable),
                    Some(AwaitableType::GatherFuture) => self.await_gather_future(heap_id, awaitable),
                    None => {
                        // Not an awaitable type
                        let type_name = awaitable.py_type(self.heap);
                        awaitable.drop_with_heap(self.heap);
                        Err(ExcType::object_not_awaitable(type_name))
                    }
                }
            }
            Value::ExternalFuture(call_id) => self.await_external_future(call_id),
            _ => {
                // Not an awaitable type
                let type_name = awaitable.py_type(self.heap);
                awaitable.drop_with_heap(self.heap);
                Err(ExcType::object_not_awaitable(type_name))
            }
        }
    }

    /// Awaits a coroutine by pushing a frame to execute it.
    ///
    /// Validates the coroutine is in `New` state, extracts its captured namespace
    /// and cells, marks it as `Running`, and pushes a frame to execute the coroutine body.
    fn await_coroutine(&mut self, heap_id: HeapId, awaitable: Value) -> Result<AwaitResult, RunError> {
        let HeapData::Coroutine(coro) = self.heap.get(heap_id) else {
            unreachable!("await_coroutine called with non-coroutine heap_id")
        };

        // Check if coroutine can be awaited (must be New)
        if coro.state != CoroutineState::New {
            awaitable.drop_with_heap(self.heap);
            return Err(
                SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited coroutine").into(),
            );
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

        Ok(AwaitResult::FramePushed)
    }

    /// Awaits a gather future by spawning tasks for coroutines and tracking external futures.
    ///
    /// For each item in the gather:
    /// - Coroutines are spawned as tasks
    /// - External futures are checked for resolution or registered for tracking
    ///
    /// If all items are already resolved, returns immediately. Otherwise blocks
    /// the current task and switches to a ready task or yields to the host.
    #[cfg_attr(not(feature = "ref-count-panic"), expect(unused_mut))]
    fn await_gather_future(&mut self, heap_id: HeapId, mut awaitable: Value) -> Result<AwaitResult, RunError> {
        let HeapData::GatherFuture(gather) = self.heap.get(heap_id) else {
            unreachable!("await_gather_future called with non-gather heap_id")
        };

        // Check if already being waited on (double-await)
        if gather.waiter.is_some() {
            awaitable.drop_with_heap(self.heap);
            return Err(SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited gather").into());
        }

        // If no items to gather, return empty list immediately
        if gather.item_count() == 0 {
            awaitable.drop_with_heap(self.heap);
            let list_id = self.heap.allocate(HeapData::List(List::new(vec![])))?;
            return Ok(AwaitResult::ValueReady(Value::Ref(list_id)));
        }

        // Set waiter and clone items to process
        // Note: We clone instead of mem::take because GatherItem::Coroutine holds HeapIds
        // that need to stay in gather.items for proper ref counting when the gather is dropped.
        let current_task = self.get_or_create_scheduler().current_task_id();
        let items: Vec<GatherItem> = if let HeapData::GatherFuture(gather_mut) = self.heap.get_mut(heap_id) {
            gather_mut.waiter = current_task;
            gather_mut.items.clone()
        } else {
            vec![]
        };

        // Process each item
        let mut task_ids = Vec::new();
        let mut pending_calls = Vec::new();

        for (idx, item) in items.iter().enumerate() {
            match item {
                GatherItem::Coroutine(coro_id) => {
                    // Spawn as task with the item index as result index
                    let task_id = self.scheduler_mut().spawn(*coro_id, Some(heap_id), Some(idx));
                    task_ids.push(task_id);
                }
                GatherItem::ExternalFuture(call_id) => {
                    // Check if already resolved
                    let scheduler = self.get_or_create_scheduler();
                    scheduler.mark_consumed(*call_id);

                    if let Some(value) = self.scheduler_mut().take_resolved(*call_id) {
                        // Already resolved - store result immediately
                        if let HeapData::GatherFuture(gather_mut) = self.heap.get_mut(heap_id) {
                            gather_mut.results[idx] = Some(value);
                        }
                    } else {
                        // Not resolved yet - track it
                        pending_calls.push(*call_id);
                        // Register gather as waiting on this call
                        self.scheduler_mut().register_gather_for_call(*call_id, heap_id, idx);
                    }
                }
            }
        }

        // Store task IDs and pending calls in the gather
        if let HeapData::GatherFuture(gather_mut) = self.heap.get_mut(heap_id) {
            gather_mut.task_ids = task_ids;
            gather_mut.pending_calls.clone_from(&pending_calls);
        }

        // Check if all items are already complete (only external futures, all resolved)
        let all_complete = {
            if let HeapData::GatherFuture(gather) = self.heap.get(heap_id) {
                gather.task_ids.is_empty() && gather.pending_calls.is_empty()
            } else {
                false
            }
        };

        if all_complete {
            // All external futures were already resolved - return results immediately
            // Steal results using mem::take - avoids refcount dance since we're dropping
            // the GatherFuture anyway via awaitable.drop_with_heap below
            let results: Vec<Value> = if let HeapData::GatherFuture(gather) = self.heap.get_mut(heap_id) {
                std::mem::take(&mut gather.results)
                    .into_iter()
                    .map(|r| r.expect("all results should be filled"))
                    .collect()
            } else {
                vec![]
            };

            awaitable.drop_with_heap(self.heap);
            let list_id = self.heap.allocate(HeapData::List(List::new(results)))?;
            return Ok(AwaitResult::ValueReady(Value::Ref(list_id)));
        }

        // Block current task on this gather
        self.scheduler_mut().block_current_on_gather(heap_id);

        // Consume the awaitable without decrementing refcount - the GatherFuture
        // must stay alive for result collection. It will be dec_ref'd when
        // the gather completes (in handle_task_completion).
        #[cfg(feature = "ref-count-panic")]
        awaitable.dec_ref_forget();

        // Switch to next ready task (spawned tasks) or yield for external futures
        self.switch_or_yield()
    }

    /// Awaits an external future by blocking until it's resolved.
    ///
    /// If the future is already resolved, returns the value immediately.
    /// Otherwise blocks the current task and switches to a ready task or yields to the host.
    fn await_external_future(&mut self, call_id: CallId) -> Result<AwaitResult, RunError> {
        // Check if already consumed (double-await error)
        // If no scheduler exists, call can't have been consumed
        if self.scheduler.as_ref().is_some_and(|s| s.is_consumed(call_id)) {
            return Err(SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited future").into());
        }

        // Mark as consumed (creates scheduler if needed)
        let scheduler = self.get_or_create_scheduler();
        scheduler.mark_consumed(call_id);

        // Check if the future is already resolved
        if let Some(value) = scheduler.take_resolved(call_id) {
            Ok(AwaitResult::ValueReady(value))
        } else {
            // Block current task on this call
            self.scheduler_mut().block_current_on_call(call_id);

            // Switch to next ready task or yield to host
            self.switch_or_yield()
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
    fn switch_or_yield(&mut self) -> Result<AwaitResult, RunError> {
        // Get next ready task (scheduler must exist - we're in async context)
        let scheduler = self.scheduler_mut();
        if let Some(next_task_id) = scheduler.next_ready_task() {
            // Save current task context ONLY when switching to another task.
            // This is critical: if we're about to yield (no ready tasks), the main task's
            // frames must stay in the VM so they're included in the snapshot.
            if let Some(current_task_id) = scheduler.current_task_id() {
                self.save_task_context(current_task_id);
            }

            self.scheduler_mut().set_current_task(Some(next_task_id));

            // Load or initialize the next task's context
            self.load_or_init_task(next_task_id)?;

            // Continue execution - return FramePushed to reload cache and continue run loop
            Ok(AwaitResult::FramePushed)
        } else {
            // No ready tasks - yield control to host.
            // Don't save the main task's context - frames stay in VM for the snapshot.
            let pending = self.scheduler().pending_call_ids();
            Ok(AwaitResult::Yield(pending))
        }
    }

    /// Handles completion of a spawned task.
    ///
    /// Called when a spawned task's coroutine returns. This:
    /// 1. Marks the task as completed in the scheduler
    /// 2. If the task belongs to a gather, stores the result and checks if gather is complete
    /// 3. If gather is complete, unblocks the waiter and provides the collected results
    /// 4. Otherwise, switches to the next ready task
    pub(super) fn handle_task_completion(&mut self, result: Value) -> Result<AwaitResult, RunError> {
        // Get task info (scheduler must exist - we're in async context)
        let scheduler = self.scheduler_mut();
        let task_id = scheduler
            .current_task_id()
            .expect("handle_task_completion called without current task");
        let task = scheduler.get_task(task_id);
        let gather_id = task.gather_id;
        let gather_result_idx = task.gather_result_idx;
        let coroutine_id = task.coroutine_id;

        // Mark coroutine as completed
        if let Some(coro_id) = coroutine_id
            && let HeapData::Coroutine(coro) = self.heap.get_mut(coro_id)
        {
            coro.state = CoroutineState::Completed;
        }

        // Mark task as completed and store result in task state
        self.scheduler_mut().complete_task(task_id, result.copy_for_extend());
        if let Value::Ref(id) = &result {
            self.heap.inc_ref(*id);
        }

        // If task belongs to a gather, store result and check if gather is complete
        if let Some(gid) = gather_id {
            // Store result in gather.results at the correct index
            if let Some(idx) = gather_result_idx
                && let HeapData::GatherFuture(gather) = self.heap.get_mut(gid)
            {
                gather.results[idx] = Some(result.copy_for_extend());
                if let Value::Ref(id) = &result {
                    self.heap.inc_ref(*id);
                }
            }

            // Extract gather metadata - clone task_ids since we need to check completion
            // but gather might not be complete yet. We only take task_ids later when
            // we know gather is complete and will be destroyed.
            let (task_ids, waiter, pending_calls_empty) = if let HeapData::GatherFuture(gather) = self.heap.get(gid) {
                (gather.task_ids.clone(), gather.waiter, gather.pending_calls.is_empty())
            } else {
                (vec![], None, true)
            };

            // Check if all tasks are complete AND all external futures are resolved
            let all_tasks_complete = task_ids.iter().all(|tid| {
                matches!(
                    self.scheduler().get_task(*tid).state,
                    TaskState::Completed(_) | TaskState::Failed(_)
                )
            });
            let all_external_resolved = pending_calls_empty;
            let all_complete = all_tasks_complete && all_external_resolved;

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

                // Steal results from gather using mem::take - avoids refcount dance
                // (copy + inc_ref + dec_ref on gather drop). Since gather is being
                // destroyed, we can take ownership of the values directly.
                let results: Vec<Value> = if let HeapData::GatherFuture(gather) = self.heap.get_mut(gid) {
                    std::mem::take(&mut gather.results)
                        .into_iter()
                        .map(|r| r.expect("all results should be filled when gather is complete"))
                        .collect()
                } else {
                    vec![]
                };

                // Create result list
                let list_id = self.heap.allocate(HeapData::List(List::new(results)))?;

                // Clean up gather - drop the original result since we copied it
                result.drop_with_heap(self.heap);

                // Release the GatherFuture - this will cascade to release coroutines
                self.heap.dec_ref(gid);

                // Unblock waiter and switch to it
                if let Some(waiter_id) = waiter {
                    let scheduler = self.scheduler_mut();
                    scheduler.make_ready(waiter_id);
                    // Remove from ready queue since we're switching directly to it
                    scheduler.remove_from_ready_queue(waiter_id);
                    // Clear current task's state since it's done
                    self.cleanup_current_frames();
                    self.stack.clear();
                    // Switch to waiter
                    self.scheduler_mut().set_current_task(Some(waiter_id));
                    self.load_or_init_task(waiter_id)?;
                    // Push the result onto the waiter's stack
                    self.push(Value::Ref(list_id));
                    return Ok(AwaitResult::FramePushed);
                }

                // No waiter (shouldn't happen but handle gracefully)
                return Ok(AwaitResult::ValueReady(Value::Ref(list_id)));
            }
        }

        // Drop the result (it's stored in the task state now)
        result.drop_with_heap(self.heap);

        // Gather not complete or no gather - switch to next task
        // Clear current task's state since it's done
        self.cleanup_current_frames();
        self.stack.clear();

        // Get next ready task
        let scheduler = self.scheduler_mut();
        scheduler.set_current_task(None);
        if let Some(next_task_id) = scheduler.next_ready_task() {
            self.scheduler_mut().set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
            Ok(AwaitResult::FramePushed)
        } else {
            // No ready tasks - yield to host
            let pending = self.scheduler().pending_call_ids();
            Ok(AwaitResult::Yield(pending))
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
        // Get task info (scheduler must exist - we're in async context)
        let scheduler = self.scheduler_mut();
        let task_id = scheduler
            .current_task_id()
            .expect("handle_task_failure called without current task");
        debug_assert!(!task_id.is_main(), "handle_task_failure called for main task");

        // Get task's gather_id before marking failed
        let gather_id = scheduler.get_task(task_id).gather_id;

        // If part of a gather, propagate error to waiter
        if let Some(gid) = gather_id {
            // Get waiter and take task_ids from GatherFuture - gather is being destroyed anyway
            let (waiter, task_ids) = if let HeapData::GatherFuture(gather) = self.heap.get_mut(gid) {
                (gather.waiter, std::mem::take(&mut gather.task_ids))
            } else {
                (None, vec![])
            };

            // Mark task as failed
            self.scheduler_mut().fail_task(task_id, error);

            // Cancel sibling tasks (filter out self and already-finished tasks inline)
            for sibling_id in task_ids {
                if sibling_id != task_id && !self.scheduler().get_task(sibling_id).is_finished() {
                    self.scheduler.as_mut().expect("scheduler must exist").cancel_task(
                        sibling_id,
                        self.heap,
                        self.namespaces,
                    );
                }
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
            // No gather - just mark task as failed (ignore returned gather_id which is None)
            let _ = self.scheduler_mut().fail_task(task_id, error);
        }

        // No gather or no waiter - switch to next task
        self.cleanup_current_frames();
        self.stack.clear();

        let scheduler = self.scheduler_mut();
        scheduler.set_current_task(None);
        if let Some(next_task_id) = scheduler.next_ready_task() {
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

        // Now assign to task (scheduler must exist - we're in async context)
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
        // (scheduler must exist - we're in async context)
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
    pub fn resolve_future(&mut self, call_id: u32, obj: MontyObject) -> Result<(), InvalidInputError> {
        let call_id = CallId::new(call_id);
        // Check if the creator task has been cancelled/failed
        // (scheduler must exist if we're resolving futures)
        let scheduler = self.scheduler_mut();
        if let Some(creator_task) = scheduler.get_pending_call_creator(call_id)
            && scheduler.is_task_failed(creator_task)
        {
            // Task was cancelled - silently ignore the result
            return Ok(());
        }
        let value = obj.to_value(self.heap, self.interns)?;

        // Check if a gather is waiting on this CallId
        if let Some((gather_id, result_idx)) = self.scheduler_mut().take_gather_waiter(call_id) {
            // Remove from scheduler's pending_calls so it doesn't appear in get_pending_call_ids()
            self.scheduler_mut().remove_pending_call(call_id);
            // Store result directly in gather (move, not clone) and check completion
            let (pending_empty, task_ids, waiter) = if let HeapData::GatherFuture(gather) = self.heap.get_mut(gather_id)
            {
                gather.results[result_idx] = Some(value); // Move value directly, no clone needed
                // Remove from pending_calls
                gather.pending_calls.retain(|&cid| cid != call_id);
                // Take task_ids to avoid clone - we're checking completion so gather may be destroyed
                (
                    gather.pending_calls.is_empty(),
                    std::mem::take(&mut gather.task_ids),
                    gather.waiter,
                )
            } else {
                (true, vec![], None)
            };

            // Check if gather is now complete (all external futures resolved and all tasks complete)
            if pending_empty {
                let all_tasks_complete = task_ids.is_empty()
                    || task_ids.iter().all(|tid| {
                        matches!(
                            self.scheduler().get_task(*tid).state,
                            TaskState::Completed(_) | TaskState::Failed(_)
                        )
                    });
                if all_tasks_complete {
                    // Gather is complete - build result and push to waiter's stack
                    if let Some(waiter_id) = waiter {
                        // Steal results from gather using mem::take - avoids refcount dance
                        // (copy + inc_ref + dec_ref on gather drop). Since gather is being
                        // destroyed, we can take ownership of the values directly.
                        let results: Vec<Value> = if let HeapData::GatherFuture(gather) = self.heap.get_mut(gather_id) {
                            std::mem::take(&mut gather.results)
                                .into_iter()
                                .map(|r| r.expect("all results should be filled when gather is complete"))
                                .collect()
                        } else {
                            vec![]
                        };

                        // Create result list - if this fails, we can't do much, just skip
                        if let Ok(list_id) = self.heap.allocate(HeapData::List(List::new(results))) {
                            // Release the GatherFuture (results already taken, so no double-drop)
                            self.heap.dec_ref(gather_id);

                            // Push result onto waiter's stack and mark as ready
                            // Check if the waiter's context is saved in the task or in the VM
                            let waiter_context_in_vm = waiter_id.is_main() && !self.frames.is_empty();

                            if waiter_context_in_vm {
                                // Main task's frames are in VM (external-only gather, no task switching)
                                self.stack.push(Value::Ref(list_id));
                                // Mark main task as ready but don't add to ready_queue
                                self.scheduler_mut().get_task_mut(waiter_id).state = TaskState::Ready;
                            } else {
                                // Waiter's context is saved in the task (either spawned task,
                                // or main task that was saved when switching to spawned tasks)
                                let scheduler = self.scheduler_mut();
                                scheduler.get_task_mut(waiter_id).stack.push(Value::Ref(list_id));
                                scheduler.make_ready(waiter_id);
                            }
                        }
                    }
                }
            }
        } else {
            // Normal resolution for single awaiter
            self.scheduler_mut().resolve(call_id, value);
        }
        Ok(())
    }

    /// Fails an external future with an error.
    ///
    /// Called by the host when an async external call fails with an exception.
    /// Finds the task blocked on this CallId and fails it with the error.
    /// If the task is part of a gather, cancels sibling tasks.
    pub fn fail_future(&mut self, call_id: u32, error: RunError) {
        let call_id = CallId::new(call_id);

        // Check if a gather is waiting on this CallId
        if let Some((gather_id, _result_idx)) = self.get_or_create_scheduler().take_gather_waiter(call_id) {
            // Remove from pending_calls so it doesn't appear in get_pending_call_ids()
            // (fail_for_call handles this for the non-gather case)
            self.scheduler_mut().remove_pending_call(call_id);

            // Get the gather's waiter, task_ids, and OTHER pending calls
            // We need to remove all pending calls for this gather from gather_waiters
            // before we dec_ref the gather, otherwise subsequent errors for the same
            // gather would try to access a freed heap object.
            // Use get_mut and take to avoid allocations - gather is being destroyed anyway.
            let (waiter, task_ids, other_pending_calls) =
                if let HeapData::GatherFuture(gather) = self.heap.get_mut(gather_id) {
                    let mut other_calls = std::mem::take(&mut gather.pending_calls);
                    other_calls.retain(|&cid| cid != call_id);
                    (gather.waiter, std::mem::take(&mut gather.task_ids), other_calls)
                } else {
                    (None, vec![], vec![])
                };

            // Remove all other pending calls for this gather from gather_waiters and pending_calls
            // This prevents subsequent errors from trying to access the freed gather
            let scheduler = self.scheduler_mut();
            for other_call_id in other_pending_calls {
                scheduler.take_gather_waiter(other_call_id);
                scheduler.remove_pending_call(other_call_id);
            }

            // Cancel all sibling tasks in the gather
            for sibling_id in task_ids {
                self.scheduler.as_mut().expect("scheduler must exist").cancel_task(
                    sibling_id,
                    self.heap,
                    self.namespaces,
                );
            }

            // Fail the waiter task (the task that awaited the gather)
            if let Some(waiter_id) = waiter {
                // Mark the waiter task as failed
                self.scheduler_mut().fail_task(waiter_id, error);
                // Release the GatherFuture
                self.heap.dec_ref(gather_id);
            }
        } else if let Some((task_id, Some(gid))) = self.scheduler_mut().fail_for_call(call_id, error) {
            // Original path: task is directly BlockedOnCall and part of a gather
            // Take task_ids from GatherFuture - gather is being destroyed anyway
            let task_ids: Vec<TaskId> = if let HeapData::GatherFuture(gather) = self.heap.get_mut(gid) {
                std::mem::take(&mut gather.task_ids)
            } else {
                vec![]
            };

            // Cancel sibling tasks (filter out self and already-finished tasks)
            for sibling_id in task_ids {
                if sibling_id != task_id && !self.scheduler().get_task(sibling_id).is_finished() {
                    self.scheduler.as_mut().expect("scheduler must exist").cancel_task(
                        sibling_id,
                        self.heap,
                        self.namespaces,
                    );
                }
            }
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
    pub fn add_pending_call(&mut self, call_id: CallId) {
        let scheduler = self.get_or_create_scheduler();
        let current_task = scheduler.current_task_id().unwrap_or_default();
        scheduler.add_pending_call(
            call_id,
            PendingCallData {
                args: ArgValues::Empty,
                creator_task: current_task,
            },
        );
    }

    /// Prepares the main task to continue after futures are resolved.
    ///
    /// When the main task was blocked on an external future and that future is now
    /// resolved, this method takes the resolved value from the scheduler and pushes
    /// it onto the VM's stack so execution can continue.
    ///
    /// This is called by `FutureSnapshot::resume()` after resolving futures but before
    /// calling `vm.run()`. For non-main tasks, the value is handled during task switching
    /// in `load_or_init_task`.
    ///
    /// # Returns
    /// `true` if a value was pushed, `false` if no task was ready to continue.
    pub fn prepare_main_task_after_resolve(&mut self) -> bool {
        let Some(scheduler) = &mut self.scheduler else {
            return false;
        };

        // Check if there's a current task (main or spawned)
        let Some(current_task_id) = scheduler.current_task_id() else {
            return false;
        };

        // Take the resolved value for the current task (if it was unblocked)
        if let Some(value) = scheduler.take_resolved_for_task(current_task_id) {
            // Remove task from ready_queue since we're handling it directly.
            // resolve() added it to ready_queue, but since frames are already
            // in the VM (not saved/restored), we handle it here instead of via task switching.
            scheduler.remove_from_ready_queue(current_task_id);
            self.push(value);
            true
        } else {
            false
        }
    }

    /// Loads a ready task if the VM has no frames.
    ///
    /// This is called by `FutureSnapshot::resume()` after resolving futures but before
    /// calling `vm.run()`. When all external futures for a gather are resolved while
    /// tasks were running (mixed coroutines + external futures), the task context
    /// may need to be loaded from the scheduler.
    ///
    /// # Returns
    /// - `Ok(true)` if a task was loaded and execution can continue
    /// - `Ok(false)` if frames already exist (nothing to do)
    /// - `Err(error)` if loading the task failed
    pub fn load_ready_task_if_needed(&mut self) -> Result<bool, RunError> {
        // If we have frames, nothing to do
        if !self.frames.is_empty() {
            return Ok(false);
        }

        // Check if there's a ready task to load
        let Some(scheduler) = &mut self.scheduler else {
            return Ok(false);
        };

        if let Some(next_task_id) = scheduler.next_ready_task() {
            scheduler.set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Gets the pending call IDs from the scheduler.
    ///
    /// Returns an empty vec if no scheduler exists.
    pub fn get_pending_call_ids(&self) -> Vec<CallId> {
        self.scheduler
            .as_ref()
            .map_or_else(Vec::new, Scheduler::pending_call_ids)
    }

    /// Takes the error from a failed task if the main task (or current task) has failed.
    ///
    /// Returns `Some(error)` if the current task is in `TaskState::Failed`, `None` otherwise.
    /// Used by `FutureSnapshot::resume` to propagate errors after resolving futures.
    pub fn take_failed_task_error(&mut self) -> Option<RunError> {
        let scheduler = self.scheduler.as_mut()?;
        let current_task_id = scheduler.current_task_id()?;
        let task = scheduler.get_task_mut(current_task_id);

        if let TaskState::Failed(error) = std::mem::replace(&mut task.state, TaskState::Ready) {
            Some(error)
        } else {
            None
        }
    }
}

/// Internal enum for dispatching await operations by heap data type.
///
/// Used in `exec_get_awaitable` to determine which handler to call after
/// inspecting the heap data type. This avoids borrow conflicts between
/// the heap reference and `&mut self` needed by the handler methods.
enum AwaitableType {
    Coroutine,
    GatherFuture,
}
