//! Async execution support for the VM.
//!
//! This module contains all async-related methods for the VM including:
//! - Awaiting coroutines, external futures, and gather futures
//! - Task scheduling and context switching
//! - Task completion and failure handling
//! - External future resolution

use std::mem;

use ahash::AHashMap;

use super::{AwaitResult, CallFrame, FrameExit, VM};
use crate::{
    MontyException,
    args::ArgValues,
    asyncio::{CallId, Coroutine, CoroutineState, GatherFuture, GatherItem, TaskId},
    bytecode::vm::scheduler::{PendingCallData, SerializedTaskFrame, TaskState},
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{HeapData, HeapGuard, HeapId, HeapRead, HeapReadOutput},
    intern::FunctionId,
    resource::ResourceTracker,
    run_progress::ExtFunctionResult,
    types::{List, PyTrait},
    value::Value,
};

impl<'h, T: ResourceTracker> VM<'h, T> {
    /// Executes the Await opcode.
    ///
    /// Pops the awaitable from the stack and handles it based on its type:
    /// - `Coroutine`: validates state is New, then pushes a frame to execute it
    /// - `ExternalFuture`: blocks until resolved or yields if not ready
    /// - `GatherFuture`: spawns tasks for coroutines and tracks external futures
    ///
    /// Returns `AwaitResult` indicating what action the VM should take.
    pub(super) fn exec_get_awaitable(&mut self) -> Result<AwaitResult, RunError> {
        let this = self;
        let awaitable = this.pop();
        defer_drop!(awaitable, this);

        match awaitable {
            Value::Ref(heap_id) => {
                let heap_id = *heap_id;
                match this.heap.read(heap_id) {
                    HeapReadOutput::Coroutine(coro) => this.await_coroutine(coro),
                    HeapReadOutput::GatherFuture(gather) => this.await_gather_future(heap_id, gather),
                    _ => Err(ExcType::object_not_awaitable(awaitable.py_type(this))),
                }
            }
            &Value::ExternalFuture(call_id) => this.await_external_future(call_id),
            _ => Err(ExcType::object_not_awaitable(awaitable.py_type(this))),
        }
    }

    /// Awaits a coroutine by pushing a frame to execute it.
    ///
    /// Validates the coroutine is in `New` state, extracts its captured namespace
    /// and cells, marks it as `Running`, and pushes a frame to execute the coroutine body.
    fn await_coroutine(&mut self, mut coro: HeapRead<'h, Coroutine>) -> Result<AwaitResult, RunError> {
        // Check if coroutine can be awaited (must be New)
        if coro.get(self.heap).state != CoroutineState::New {
            return Err(
                SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited coroutine").into(),
            );
        }

        // Extract coroutine data before mutating
        let func_id = coro.get(self.heap).func_id;
        let namespace_values: Vec<Value> = coro
            .get(self.heap)
            .namespace
            .iter()
            .map(|v| v.clone_with_heap(self))
            .collect();

        // Mark coroutine as Running
        coro.get_mut(self.heap).state = CoroutineState::Running;

        // Create namespace and push frame (guard drops awaitable at scope exit)
        self.start_coroutine_frame(func_id, namespace_values)?;

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
    fn await_gather_future(
        &mut self,
        heap_id: HeapId,
        mut gather: HeapRead<'h, GatherFuture>,
    ) -> Result<AwaitResult, RunError> {
        // Check if already being waited on (double-await)
        if gather.get(self.heap).waiter.is_some() {
            return Err(SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited gather").into());
        }

        // If no items to gather, return empty list immediately
        if gather.get(self.heap).item_count() == 0 {
            let list_id = self.heap.allocate(HeapData::List(List::new(vec![])))?;
            return Ok(AwaitResult::ValueReady(Value::Ref(list_id)));
        }

        // Reject any external future that has already been awaited (directly or
        // via another gather). Without this check, sibling gathers sharing a
        // future would silently overwrite each other in `gather_waiters`,
        // leaving the first gather permanently blocked on a CallId that has
        // already been resolved or is registered against a different gather.
        // This mirrors the existing `is_consumed` check in `await_external_future`
        // so that direct double-await and gather-mediated double-await behave
        // consistently. The dedup pass below ensures intra-gather duplicates
        // (`gather(f, f)`) are not flagged: each unique CallId is only marked
        // consumed once, so the first await of a freshly-created future passes
        // even when it appears in multiple slots.
        for item in &gather.get(self.heap).items {
            if let GatherItem::ExternalFuture(call_id) = item
                && self.scheduler.is_consumed(*call_id)
            {
                return Err(
                    SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited future").into(),
                );
            }
        }

        // Set waiter and walk the items, deduplicating by identity so that the
        // same coroutine or external future passed multiple times runs once and
        // its result is fanned out to every gather slot. This matches CPython's
        // `arg_to_fut` mapping in `asyncio.tasks.gather`.
        //
        // Coroutine spawns are buffered and applied after the `gather_mut` borrow
        // ends, because `Scheduler::spawn` borrows the heap to call `inc_ref` on
        // the new task's owning references. Already-resolved external futures are
        // also fanned out after dropping the borrow because cloning their value
        // requires mutable heap access.
        let current_task = self.scheduler.current_task_id();
        let gather_mut = gather.get_mut(self.heap);
        gather_mut.waiter = current_task;

        // Per-unique entries with the list of gather slots they should fill.
        // Order of first occurrence is preserved so spawn order matches argument order.
        let mut coro_spawn_plan: Vec<(HeapId, Vec<usize>)> = Vec::new();
        let mut coro_seen: AHashMap<HeapId, usize> = AHashMap::new();
        let mut external_plan: Vec<(CallId, Vec<usize>)> = Vec::new();
        let mut external_seen: AHashMap<CallId, usize> = AHashMap::new();

        for (idx, item) in gather_mut.items.iter().enumerate() {
            match item {
                GatherItem::Coroutine(coro_id) => {
                    if let Some(&plan_idx) = coro_seen.get(coro_id) {
                        coro_spawn_plan[plan_idx].1.push(idx);
                    } else {
                        coro_seen.insert(*coro_id, coro_spawn_plan.len());
                        coro_spawn_plan.push((*coro_id, vec![idx]));
                    }
                }
                GatherItem::ExternalFuture(call_id) => {
                    if let Some(&plan_idx) = external_seen.get(call_id) {
                        external_plan[plan_idx].1.push(idx);
                    } else {
                        external_seen.insert(*call_id, external_plan.len());
                        external_plan.push((*call_id, vec![idx]));
                    }
                }
            }
        }

        // Process external futures: mark consumed, then either take an existing
        // resolved value or register the call as pending with all its indices.
        let mut pending_calls = Vec::new();
        let mut already_resolved: Vec<(Vec<usize>, Value)> = Vec::new();
        for (call_id, indices) in external_plan {
            self.scheduler.mark_consumed(call_id);
            if let Some(value) = self.scheduler.take_resolved(call_id) {
                already_resolved.push((indices, value));
            } else {
                pending_calls.push(call_id);
                self.scheduler.register_gather_for_call(call_id, heap_id, indices);
            }
        }

        // Spawn one task per unique coroutine; each spawn inc_refs the coroutine
        // and the gather. The task carries the full list of slot indices so that
        // its result is fanned out at completion time.
        let mut task_ids = Vec::with_capacity(coro_spawn_plan.len());
        for (coro_id, indices) in coro_spawn_plan {
            let task_id = self.scheduler.spawn(self.heap, coro_id, Some(heap_id), indices);
            task_ids.push(task_id);
        }

        // Fan out already-resolved external futures into gather.results. Each
        // duplicate slot needs an independent inc_ref via `clone_with_heap`; the
        // last slot moves the original value to avoid an unnecessary clone+drop.
        // Clones must happen while no `HeapRead<GatherFuture>` borrow is held.
        let mut resolved_writes: Vec<(usize, Value)> = Vec::new();
        for (indices, value) in already_resolved {
            let Some((last, init)) = indices.split_last() else {
                value.drop_with_heap(self.heap);
                continue;
            };
            for &idx in init {
                resolved_writes.push((idx, value.clone_with_heap(self.heap)));
            }
            resolved_writes.push((*last, value));
        }

        // Re-acquire mutable access to the gather to store pending calls, task
        // ids, and any already-resolved values fanned out above.
        let gather_mut = gather.get_mut(self.heap);
        gather_mut.pending_calls = pending_calls;
        gather_mut.task_ids = task_ids;
        for (idx, value) in resolved_writes {
            gather_mut.results[idx] = Some(value);
        }

        // Check if all items are already complete (only external futures, all resolved)
        let all_complete = gather_mut.task_ids.is_empty() && gather_mut.pending_calls.is_empty();

        if all_complete {
            // All external futures were already resolved - return results immediately
            let results: Vec<Value> = mem::take(&mut gather_mut.results)
                .into_iter()
                .map(|r| r.expect("all results should be filled"))
                .collect();

            let list_id = self.heap.allocate(HeapData::List(List::new(results)))?;
            return Ok(AwaitResult::ValueReady(Value::Ref(list_id)));
        }

        // Block current task on this gather
        self.scheduler.block_current_on_gather(heap_id, self.heap);

        // Switch to next ready task (spawned tasks) or yield for external futures
        self.switch_or_yield()
    }

    /// Awaits an external future by blocking until it's resolved.
    ///
    /// If the future is already resolved, returns the value immediately.
    /// Otherwise blocks the current task and switches to a ready task or yields to the host.
    fn await_external_future(&mut self, call_id: CallId) -> Result<AwaitResult, RunError> {
        // Check if already consumed (double-await error)
        if self.scheduler.is_consumed(call_id) {
            return Err(SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited future").into());
        }

        // Mark as consumed
        self.scheduler.mark_consumed(call_id);

        // Check if the future is already resolved
        if let Some(value) = self.scheduler.take_resolved(call_id) {
            Ok(AwaitResult::ValueReady(value))
        } else {
            // Block current task on this call
            self.scheduler.block_current_on_call(call_id);

            // Switch to next ready task or yield to host
            self.switch_or_yield()
        }
    }

    /// Starts execution of a coroutine by pushing its locals onto the stack.
    ///
    /// Extends the VM stack with the coroutine's pre-bound namespace values
    /// and pushes a new frame to execute the coroutine's function body.
    fn start_coroutine_frame(&mut self, func_id: FunctionId, namespace_values: Vec<Value>) -> Result<(), RunError> {
        let call_position = self.current_position();
        let func = self.interns.get_function(func_id);
        let locals_count = u16::try_from(namespace_values.len()).expect("coroutine namespace size exceeds u16");

        // Track memory for the locals
        let size = namespace_values.len() * mem::size_of::<Value>();
        self.heap.tracker_mut().on_allocate(|| size)?;

        // Extend the stack with the coroutine's pre-bound locals
        let stack_base = self.stack.len();
        self.stack.extend(namespace_values);

        // Push frame to execute the coroutine
        self.push_frame(CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            func_id,
            call_position,
        ))?;

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
        if let Some(next_task_id) = self.scheduler.next_ready_task() {
            // Save current task context ONLY when switching to another task.
            // This is critical: if we're about to yield (no ready tasks), the main task's
            // frames must stay in the VM so they're included in the snapshot.
            if let Some(current_task_id) = self.scheduler.current_task_id() {
                self.save_task_context(current_task_id);
            }

            self.scheduler.set_current_task(Some(next_task_id));

            // Load or initialize the next task's context
            self.load_or_init_task(next_task_id)?;

            // Continue execution - return FramePushed to reload cache and continue run loop
            Ok(AwaitResult::FramePushed)
        } else {
            // No ready tasks - yield control to host.
            // Don't save the main task's context - frames stay in VM for the snapshot.
            Ok(AwaitResult::Yield(self.scheduler.pending_call_ids()))
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
        // Get task info
        let task_id = self
            .scheduler
            .current_task_id()
            .expect("handle_task_completion called without current task");
        let task = self.scheduler.get_task_mut(task_id);
        let gather_id = task.gather_id;
        let gather_result_indices = mem::take(&mut task.gather_result_indices);
        let coroutine_id = task.coroutine_id;

        // Mark coroutine as completed
        if let Some(coro_id) = coroutine_id {
            let HeapReadOutput::Coroutine(mut coro) = self.heap.read(coro_id) else {
                panic!("task coroutine_id doesn't point to a Coroutine")
            };
            coro.get_mut(self.heap).state = CoroutineState::Completed;
        }

        // Mark task as completed and store result in task state
        let task_result = result.clone_with_heap(self.heap);
        self.scheduler.complete_task(task_id, task_result, self.heap);

        // When the same coroutine was passed multiple times to gather, this
        // task owns several slots; clone the result for all but the last and
        // move into the last so refcounts stay balanced.
        if let Some(gid) = gather_id {
            let mut writes: Vec<(usize, Value)> = Vec::with_capacity(gather_result_indices.len());
            if let Some((last, init)) = gather_result_indices.split_last() {
                for &idx in init {
                    writes.push((idx, result.clone_with_heap(self.heap)));
                }
                writes.push((*last, result));
            } else {
                result.drop_with_heap(self.heap);
            }

            let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gid) else {
                panic!("task gather_id doesn't point to a GatherFuture")
            };

            let gather_mut = gather.get_mut(self.heap);
            for (idx, value) in writes {
                gather_mut.results[idx] = Some(value);
            }

            // Check if all tasks are complete AND all external futures are resolved
            let all_tasks_complete = gather.get(self.heap).task_ids.iter().all(|tid| {
                matches!(
                    self.scheduler.get_task(*tid).state,
                    TaskState::Completed(_) | TaskState::Failed(_)
                )
            });
            let all_external_resolved = gather.get(self.heap).pending_calls.is_empty();
            let all_complete = all_tasks_complete && all_external_resolved;

            if all_complete {
                // Take the spawned task ids and waiter while gather is still
                // readable; we'll drop the gather and remove the tasks below.
                let Some(waiter_id) = gather.get(self.heap).waiter else {
                    panic!("gather future has no waiter when gather is complete")
                };
                let task_ids = mem::take(&mut gather.get_mut(self.heap).task_ids);
                let results = mem::take(&mut gather.get_mut(self.heap).results);
                let mut results_guard = HeapGuard::new(results, self);
                let this = results_guard.heap();

                // Drop the reader before any operations that may free heap objects (e.g., cancelling tasks)
                drop(gather);

                // First check if any task failed
                let failed_task = task_ids.iter().find_map(|tid| {
                    let task = this.scheduler.get_task_mut(*tid);
                    match &task.state {
                        TaskState::Failed(_) => {
                            let TaskState::Failed(err) = mem::replace(&mut task.state, TaskState::Ready) else {
                                unreachable!()
                            };
                            Some(err)
                        }
                        _ => None,
                    }
                });

                // Release every spawned task
                for tid in task_ids {
                    this.scheduler.cancel_task(tid, this.heap);
                }

                // Make waiter ready but don't add to ready queue since we're switching directly to it
                this.scheduler.set_state(waiter_id, TaskState::Ready, this.heap);
                this.cleanup_current_task();
                this.scheduler.set_current_task(Some(waiter_id));
                this.load_or_init_task(waiter_id)?;

                return if let Some(err) = failed_task {
                    // Error is raised in waiter context
                    Err(err)
                } else {
                    let results = results_guard.into_inner();
                    let results: Vec<Value> = results
                        .into_iter()
                        .map(|r| r.expect("all results should be filled when gather is complete"))
                        .collect();

                    // Create result list
                    let list_id = self.heap.allocate(HeapData::List(List::new(results)))?;

                    // Push the result onto the waiter's stack
                    self.push(Value::Ref(list_id));
                    Ok(AwaitResult::FramePushed)
                };
            }
        } else {
            // Drop the result (it's stored in the task state now)
            result.drop_with_heap(self.heap);
        }

        // Gather not complete or no gather - switch to next task
        self.cleanup_current_task();

        // Get next ready task
        self.scheduler.set_current_task(None);
        if let Some(next_task_id) = self.scheduler.next_ready_task() {
            self.scheduler.set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
            Ok(AwaitResult::FramePushed)
        } else {
            Ok(AwaitResult::Yield(self.scheduler.pending_call_ids()))
        }
    }

    /// Returns true if the current task is a spawned task (not main).
    ///
    /// Used by exception handling to determine if an unhandled exception
    /// should fail the task rather than propagate out.
    #[inline]
    pub(super) fn is_spawned_task(&self) -> bool {
        self.scheduler.current_task_id().is_some_and(|id| !id.is_main())
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
        // Get task info
        let task_id = self
            .scheduler
            .current_task_id()
            .expect("handle_task_failure called without current task");
        debug_assert!(!task_id.is_main(), "handle_task_failure called for main task");

        // Get task's gather_id before marking failed
        let gather_id = self.scheduler.get_task(task_id).gather_id;

        // If part of a gather, propagate error to waiter
        if let Some(gid) = gather_id {
            // Take task_ids from GatherFuture - gather is being destroyed anyway
            let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gid) else {
                panic!("task gather_id doesn't point to a GatherFuture")
            };
            let gather_mut = gather.get_mut(self.heap);
            let task_ids = mem::take(&mut gather_mut.task_ids);
            let Some(waiter_id) = gather_mut.waiter else {
                panic!("gather future has no waiter when handling task failure")
            };
            // Drop the reader before any dec_ref that could free the gather.
            drop(gather);

            // Release all tasks owned by this gather
            for tid in task_ids {
                self.scheduler.cancel_task(tid, self.heap);
            }

            // Switch to waiter and propagate the error
            self.scheduler.set_state(waiter_id, TaskState::Ready, self.heap);
            self.cleanup_current_task();
            self.scheduler.set_current_task(Some(waiter_id));
            self.load_or_init_task(waiter_id)?;
            return Err(error);
        }

        // No gather - just mark task as failed, switch to next task
        self.scheduler.fail_task(task_id, error, self.heap);
        self.cleanup_current_task();
        self.scheduler.set_current_task(None);
        if let Some(next_task_id) = self.scheduler.next_ready_task() {
            self.scheduler.set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
        }
        // If no ready tasks, frames will be empty and run loop will yield

        Ok(())
    }

    /// Saves the current VM context into the given task in the scheduler.
    ///
    /// Serializes frames, moves stack/exception_stack, stores instruction_ip,
    /// and adjusts the global recursion depth counter.
    fn save_task_context(&mut self, task_id: TaskId) {
        let frames: Vec<SerializedTaskFrame> = self
            .frames
            .drain(..)
            .map(|f| SerializedTaskFrame {
                function_id: f.function_id,
                ip: f.ip,
                stack_base: f.stack_base,
                locals_count: f.locals_count,
                call_position: f.call_position,
            })
            .collect();

        // Count this task's recursion depth contribution and subtract it from
        // the global counter so the next task gets a clean budget.
        let task_depth = frames.len().saturating_sub(1); // root frame doesn't contribute to recursion depth
        let global_depth = self.heap.get_recursion_depth();
        self.heap.set_recursion_depth(global_depth - task_depth);

        // Save VM state into the task
        let task = self.scheduler.get_task_mut(task_id);
        task.frames = frames;
        task.stack = mem::take(&mut self.stack);
        task.exception_stack = mem::take(&mut self.exception_stack);
        task.instruction_ip = self.instruction_ip;
    }

    /// Loads an existing task's context or initializes a new task from its coroutine.
    ///
    /// If the task has stored frames, restores them into the VM. If the task was
    /// unblocked by an external future resolution, pushes the resolved value onto
    /// the restored stack so execution can continue past the AWAIT opcode.
    /// If the task has a coroutine_id but no frames, starts the coroutine.
    ///
    /// Restores the task's recursion depth contribution to the global counter
    /// (balances the subtraction in `save_task_context`).
    fn load_or_init_task(&mut self, task_id: TaskId) -> Result<(), RunError> {
        let task = self.scheduler.get_task_mut(task_id);
        let frames = mem::take(&mut task.frames);
        let stack = mem::take(&mut task.stack);
        let exception_stack = mem::take(&mut task.exception_stack);
        let instruction_ip = task.instruction_ip;
        let coroutine_id = task.coroutine_id;

        // Restore this task's recursion depth contribution to the global counter
        let task_depth = frames.len().saturating_sub(1); // root frame doesn't contribute to recursion depth
        let global_depth = self.heap.get_recursion_depth();
        self.heap.set_recursion_depth(global_depth + task_depth);

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
                        locals_count: sf.locals_count,
                        function_id: sf.function_id,
                        call_position: sf.call_position,
                        should_return: false,
                    }
                })
                .collect();
        } else if let Some(coro_id) = coroutine_id {
            // New task: pre-check the coroutine state here rather than letting
            // `init_task_from_coroutine` raise. By this point the calling task's
            // frames have already been saved away, so any error raised from
            // inside `init_task_from_coroutine` would reach `handle_exception`
            // with no active frame and panic. Instead, route already-awaited
            // failures through `handle_task_failure`, which restores the waiter's
            // (or next task's) frames before the error propagates.
            let HeapReadOutput::Coroutine(coro) = self.heap.read(coro_id) else {
                panic!("task coroutine_id doesn't point to a Coroutine")
            };
            if coro.get(self.heap).state == CoroutineState::New {
                self.init_task_from_coroutine(coro_id)?;
            } else {
                let error: RunError =
                    SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited coroutine").into();
                return self.handle_task_failure(error);
            }
        } else {
            // This shouldn't happen - task with no frames and no coroutine
            panic!("task has no frames and no coroutine_id");
        }

        // If this task was unblocked by a resolved external future, push the
        // resolved value onto the stack. The AWAIT opcode already advanced the IP
        // past itself before the task was saved, so execution will continue with
        // the resolved value on top of the stack.
        if let Some(value) = self.scheduler.take_resolved_for_task(task_id) {
            self.push(value);
        }

        Ok(())
    }

    /// Initializes the VM state to run a coroutine for a spawned task.
    ///
    /// Similar to exec_get_awaitable's coroutine handling, but for task initialization.
    fn init_task_from_coroutine(&mut self, coroutine_id: HeapId) -> Result<(), RunError> {
        let HeapReadOutput::Coroutine(mut coro) = self.heap.read(coroutine_id) else {
            panic!("task coroutine_id doesn't point to a Coroutine")
        };

        // Check state
        if coro.get(self.heap).state != CoroutineState::New {
            return Err(
                SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited coroutine").into(),
            );
        }

        // Extract coroutine data
        let func_id = coro.get(self.heap).func_id;
        let namespace_values: Vec<Value> = coro
            .get(self.heap)
            .namespace
            .iter()
            .map(|v| v.clone_with_heap(self))
            .collect();

        // Mark coroutine as Running
        coro.get_mut(self.heap).state = CoroutineState::Running;

        // Push locals onto stack and push frame directly (can't use start_coroutine_frame
        // because that needs a current frame for call_position, but spawned tasks
        // don't have a parent frame — the coroutine is the root)
        let func = self.interns.get_function(func_id);
        let locals_count = u16::try_from(namespace_values.len()).expect("coroutine namespace size exceeds u16");

        // Track memory for the locals
        let size = namespace_values.len() * mem::size_of::<Value>();
        self.heap.tracker_mut().on_allocate(|| size)?;

        let stack_base = self.stack.len();
        self.stack.extend(namespace_values);

        self.push_frame(CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            func_id,
            None, // No call position — this is the root frame for a spawned task
        ))?;

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
    pub fn resolve_future(&mut self, call_id: u32, value: Value) -> RunResult<()> {
        let mut value_guard = HeapGuard::new(value, self);
        let this = value_guard.heap();

        let call_id = CallId::new(call_id);
        // Check if the creator task has been cancelled/failed
        if let Some(creator_task) = this.scheduler.get_pending_call_creator(call_id)
            && this.scheduler.is_task_failed(creator_task)
        {
            // Task was cancelled - silently ignore the result
            return Ok(());
        }

        // Check if a gather is waiting on this CallId
        if let Some((gather_id, result_indices)) = this.scheduler.take_gather_waiter(call_id) {
            this.scheduler.remove_pending_call(call_id);

            // Fan the resolved value out to every gather slot waiting on this
            // CallId. Each duplicate slot needs an independent inc_ref via
            // `clone_with_heap`; the last slot moves the original value.
            let mut writes: Vec<(usize, Value)> = Vec::with_capacity(result_indices.len());
            let value = value_guard.into_inner();
            if let Some((last, init)) = result_indices.split_last() {
                for &idx in init {
                    writes.push((idx, value.clone_with_heap(self.heap)));
                }
                writes.push((*last, value));
            } else {
                value.drop_with_heap(self.heap);
            }

            let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gather_id) else {
                panic!("gather_id doesn't point to a GatherFuture")
            };
            let gather_mut = gather.get_mut(self.heap);
            for (idx, v) in writes {
                gather_mut.results[idx] = Some(v);
            }

            // Remove from pending_calls
            gather_mut.pending_calls.retain(|&cid| cid != call_id);
            let pending_empty = gather_mut.pending_calls.is_empty();

            // Check if gather is now complete (all external futures resolved and all tasks complete)
            if pending_empty {
                let all_tasks_complete = gather.get(self.heap).task_ids.iter().all(|tid| {
                    matches!(
                        self.scheduler.get_task(*tid).state,
                        TaskState::Completed(_) | TaskState::Failed(_)
                    )
                });
                if all_tasks_complete {
                    // Gather is complete - build result and push to waiter's stack
                    let Some(waiter_id) = gather.get(self.heap).waiter else {
                        panic!("gather future has no waiter when gather is complete")
                    };
                    let task_ids = mem::take(&mut gather.get_mut(self.heap).task_ids);
                    // Steal results from gather using mem::take - avoids refcount dance
                    // (copy + inc_ref + dec_ref on gather drop). Since gather is being
                    // destroyed, we can take ownership of the values directly.
                    let results: Vec<Value> = mem::take(&mut gather.get_mut(self.heap).results)
                        .into_iter()
                        .map(|r| r.expect("all results should be filled when gather is complete"))
                        .collect();
                    // Drop the HeapRead before cancellation, which may free the gather
                    drop(gather);

                    // Release every child task
                    for tid in task_ids {
                        self.scheduler.cancel_task(tid, self.heap);
                    }

                    let list_id = self.heap.allocate(HeapData::List(List::new(results)))?;

                    // Push result onto waiter's stack and mark as ready.
                    // Check if the waiter's context is currently in the VM (frames not saved
                    // to the task). This is the case when the waiter is the current task
                    // and hasn't been switched away from (e.g., external-only gather).
                    let waiter_context_in_vm =
                        self.scheduler.current_task_id() == Some(waiter_id) && !self.frames.is_empty();

                    if waiter_context_in_vm {
                        // Waiter's frames are in the VM - push directly onto VM stack
                        self.stack.push(Value::Ref(list_id));
                        // Mark as ready but don't add to ready_queue.
                        self.scheduler.set_state(waiter_id, TaskState::Ready, self.heap);
                    } else {
                        // Waiter's context is saved in the task (either spawned task,
                        // or main task that was saved when switching to spawned tasks)
                        self.scheduler.get_task_mut(waiter_id).stack.push(Value::Ref(list_id));
                        self.scheduler.make_ready(waiter_id, self.heap);
                    }
                }
            }
        } else {
            // Normal resolution for single awaiter
            let value = value_guard.into_inner();
            self.scheduler.resolve(call_id, value);
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

        self.scheduler.fail_for_call(call_id, error, self.heap);
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
        let current_task = self.scheduler.current_task_id().unwrap_or_default();
        self.scheduler.add_pending_call(
            call_id,
            PendingCallData {
                args: ArgValues::Empty,
                creator_task: current_task,
            },
        );
    }

    /// Gets the pending call IDs from the scheduler.
    pub fn get_pending_call_ids(&self) -> Vec<CallId> {
        self.scheduler.pending_call_ids()
    }

    /// Resolves external futures and resumes execution.
    ///
    /// This is the standard sequence for resuming after a `FrameExit::ResolveFutures`:
    /// 1. Resolve or fail each future from the provided results
    /// 2. Attempt to resume the current task (or fail it if any future resolution caused it to fail)
    /// 3. Load a ready task if needed (current task still blocked)
    /// 4. If no task is ready, return `ResolveFutures` with remaining pending call IDs
    pub fn resume_with_resolved_futures(&mut self, results: Vec<(u32, ExtFunctionResult)>) -> RunResult<FrameExit> {
        for (call_id, ext_result) in results {
            match ext_result {
                ExtFunctionResult::Return(obj) => {
                    let value = obj.to_value(self).map_err(|e| {
                        RunError::from(MontyException::runtime_error(format!(
                            "Invalid return value for call {call_id}: {e}"
                        )))
                    })?;
                    self.resolve_future(call_id, value)?;
                }
                ExtFunctionResult::Error(exc) => self.fail_future(call_id, RunError::from(exc)),
                ExtFunctionResult::Future(_) => {}
                ExtFunctionResult::NotFound(function_name) => {
                    self.fail_future(call_id, ExtFunctionResult::not_found_exc(&function_name));
                }
            }
        }

        if let Some(current_task_id) = self.scheduler.current_task_id() {
            let task = self.scheduler.get_task_mut(current_task_id);

            match task.state {
                TaskState::Failed(_) => {
                    // Current task failed - propagate error to caller
                    let TaskState::Failed(err) = mem::replace(&mut task.state, TaskState::Ready) else {
                        unreachable!();
                    };
                    return Err(err);
                }
                TaskState::BlockedOnCall(_) | TaskState::BlockedOnGather(_) => {
                    // Current task is still blocked on unresolved futures.
                }
                TaskState::Ready => {
                    if let Some(value) = self.scheduler.take_resolved_for_task(current_task_id) {
                        self.push(value);
                    }
                    self.scheduler.remove_from_ready_queue(current_task_id);
                    return self.run();
                }
                TaskState::Completed(_) => {
                    // Should never have suspended if the task was completed
                    panic!(
                        "current task is in unexpected Completed state after resolving futures: {:?}",
                        task.state
                    );
                }
            }
        }

        // Current task was not able to resume, but there might be other ready tasks which can make
        // progress
        if let Some(next_task_id) = self.scheduler.next_ready_task() {
            if let Some(current_task_id) = self.scheduler.current_task_id() {
                self.save_task_context(current_task_id);
            }
            self.scheduler.set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
            return self.run();
        }

        let pending_call_ids = self.get_pending_call_ids();

        assert!(
            !pending_call_ids.is_empty(),
            "resume_with_resolved_futures called but no pending calls and no ready tasks"
        );

        Ok(FrameExit::ResolveFutures(pending_call_ids))
    }
}
