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
    asyncio::{AwaitedGather, CallId, Coroutine, CoroutineState, GatherFuture, GatherItem, GatherState, TaskId},
    bytecode::vm::scheduler::{PendingCallData, Scheduler, SerializedTaskFrame, TaskState},
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropWithHeap, HeapData, HeapGuard, HeapId, HeapRead, HeapReadOutput, HeapReader},
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
        // Re-await fast paths
        match &gather.get(self.heap).state {
            GatherState::Pending => {}
            GatherState::Completed(list_id) => {
                let id = *list_id;
                self.heap.inc_ref(id);
                return Ok(AwaitResult::ValueReady(Value::Ref(id)));
            }
            GatherState::Failed(err) => return Err(err.clone()),
            // TODO: needs proper support for this case, CPython allows it
            GatherState::Awaited(_) => {
                return Err(SimpleException::new_msg(
                    ExcType::RuntimeError,
                    "cannot reuse gather that is currently being awaited",
                )
                .into());
            }
        }

        // Empty gather shortcut. Allocate the empty list, store it as the
        // cached `Completed` result, and return an inc_ref'd reference. Future
        // awaits will hit the `Completed` fast path above.
        let item_count = gather.get(self.heap).item_count();
        if item_count == 0 {
            let list_id = self.heap.allocate(HeapData::List(List::new(vec![])))?;
            gather.cache_result(self.heap, list_id);
            return Ok(AwaitResult::ValueReady(Value::Ref(list_id)));
        }

        // Reject any external future that has already been awaited (directly or
        // via another gather). Without this check, sibling gathers sharing a
        // future would silently overwrite each other in the pending-call gather
        // routing, leaving the first gather permanently blocked on a CallId
        // that has already been resolved or is registered against a different
        // gather. This mirrors the existing `is_consumed` check in
        // `await_external_future` so direct double-await and gather-mediated
        // double-await behave consistently. The dedup pass below ensures
        // intra-gather duplicates (`gather(f, f)`) are not flagged: each unique
        // CallId is only marked consumed once, so the first await of a freshly
        // created future passes even when it appears in multiple slots.
        for item in &gather.get(self.heap).items {
            if let GatherItem::ExternalFuture(call_id) = item
                && self.scheduler.is_consumed(*call_id)
            {
                return Err(
                    SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited future").into(),
                );
            }
        }

        let current_task = self
            .scheduler
            .current_task_id()
            .expect("await_gather_future called without a current task");

        // Single pass over items: spawn / register / write resolved values
        // into `results` directly. Dedup by identity so `gather(c, c)` runs
        // the coroutine once and fans its result out to every matching slot.
        let mut pending_tasks: AHashMap<TaskId, Vec<usize>> = AHashMap::new();
        let mut coro_to_task: AHashMap<HeapId, TaskId> = AHashMap::new();
        let mut pending_calls: AHashMap<CallId, Vec<usize>> = AHashMap::new();
        // For an external that resolved synchronously, remember the first
        // slot that received the moved-in value; later duplicate slots clone
        // from there.
        let mut resolved_first_slot: AHashMap<CallId, usize> = AHashMap::new();
        let mut results: Vec<Option<Value>> = (0..item_count).map(|_| None).collect();

        for (idx, item) in gather.get(self.heap).items.iter().enumerate() {
            match *item {
                GatherItem::Coroutine(coro_id) => {
                    let task_id = *coro_to_task
                        .entry(coro_id)
                        .or_insert_with(|| self.scheduler.spawn(self.heap, coro_id, Some(heap_id)));
                    pending_tasks.entry(task_id).or_default().push(idx);
                }
                GatherItem::ExternalFuture(call_id) => {
                    if let Some(indices) = pending_calls.get_mut(&call_id) {
                        // Duplicate of a still-pending external — append.
                        indices.push(idx);
                    } else if let Some(&first_idx) = resolved_first_slot.get(&call_id) {
                        // Duplicate of an already-resolved external — clone
                        // from the slot that owns the original.
                        let cloned = results[first_idx]
                            .as_ref()
                            .expect("first occurrence holds the resolved value")
                            .clone_with_heap(self.heap);
                        results[idx] = Some(cloned);
                    } else {
                        // First time seeing this CallId.
                        self.scheduler.mark_consumed(call_id);
                        if let Some(value) = self.scheduler.take_resolved(call_id) {
                            results[idx] = Some(value);
                            resolved_first_slot.insert(call_id, idx);
                        } else {
                            self.scheduler.register_gather_for_call(call_id, heap_id);
                            pending_calls.insert(call_id, vec![idx]);
                        }
                    }
                }
            }
        }

        // Synchronous-completion shortcut: external-only gather where every
        // call was already resolved. Allocate the result list and cache it on
        // the gather (Pending → Completed) so re-awaits replay the same list.
        if pending_tasks.is_empty() && pending_calls.is_empty() {
            let results: Vec<Value> = results
                .into_iter()
                .map(|r| r.expect("all results filled for synchronous gather completion"))
                .collect();
            let list_id = self.heap.allocate(HeapData::List(List::new(results)))?;
            gather.cache_result(self.heap, list_id);
            return Ok(AwaitResult::ValueReady(Value::Ref(list_id)));
        }

        // Transition Pending → Awaited.
        gather.get_mut(self.heap).state = GatherState::Awaited(AwaitedGather {
            waiter: current_task,
            pending_tasks,
            pending_calls,
            results,
        });
        drop(gather);

        // Block current task on this gather, then switch to a spawned task or
        // yield to the host for external futures.
        self.scheduler.block_current_on_gather(heap_id, self.heap);
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
        let exc_stack_base = self.exception_stack.len();
        self.push_frame(CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            exc_stack_base,
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
        // Get task info. Every spawned task belongs to a gather (the only
        // call site of `Scheduler::spawn` is `await_gather_future`), so
        // `gather_id` is unconditionally `Some`.
        let task_id = self
            .scheduler
            .current_task_id()
            .expect("handle_task_completion called without current task");
        let task = self.scheduler.get_task(task_id);
        let gid = task
            .gather_id
            .expect("handle_task_completion: spawned task without a gather");
        let coroutine_id = task.coroutine_id;

        // Mark the coroutine as Completed before the task is cancelled —
        // direct `await` of this coroutine elsewhere needs to see the new
        // state, not the `Running` it had until now.
        if let Some(coro_id) = coroutine_id {
            let HeapReadOutput::Coroutine(mut coro) = self.heap.read(coro_id) else {
                panic!("task coroutine_id doesn't point to a Coroutine")
            };
            coro.get_mut(self.heap).state = CoroutineState::Completed;
        }

        // Record the result on the gather and check whether it's now complete.
        // `resolve_child` does the fan-out for duplicate slots (`gather(c, c)`)
        // and the final state transition; it must run BEFORE we release any
        // inc_refs the gather is holding (cancelling children, dropping the
        // waiter's `BlockedOnGather`) — otherwise the gather can be freed
        // while we're still about to write its cached state.
        let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gid) else {
            panic!("task gather_id doesn't point to a GatherFuture")
        };
        let resolution = gather.resolve_child(self, ChildSource::Task(task_id), result)?;
        drop(gather);

        // The just-completed task is no longer in the gather's
        // `pending_tasks` map. Cancel it now to release its inc_refs on the
        // coroutine and gather; otherwise it would linger in the scheduler.
        self.scheduler.cancel_task(task_id, self.heap);

        match resolution {
            Some(GatherResolution::Success { list_id, waiter_id }) => {
                // Make waiter ready but don't add to ready queue — we're
                // switching directly into its context. Replacing
                // `BlockedOnGather` here releases the last gather inc_ref;
                // the cached state is what keeps the result list alive.
                self.scheduler.set_state(waiter_id, TaskState::Ready, self.heap);
                self.cleanup_current_task();
                self.scheduler.set_current_task(Some(waiter_id));
                self.load_or_init_task(waiter_id)?;
                self.push(Value::Ref(list_id));
                Ok(AwaitResult::FramePushed)
            }
            Some(GatherResolution::Failure { error, waiter_id }) => {
                // Switch into the waiter's frame and surface the error
                // there. The run loop's `catch_sync!` will dispatch through
                // the waiter's exception table — this is how a sibling's
                // failure ends up at the user's `try` / `except` around the
                // gather await.
                self.scheduler.set_state(waiter_id, TaskState::Ready, self.heap);
                self.cleanup_current_task();
                self.scheduler.set_current_task(Some(waiter_id));
                self.load_or_init_task(waiter_id)?;
                Err(error)
            }
            None => {
                // Gather not complete — switch to next ready task.
                self.cleanup_current_task();
                self.scheduler.set_current_task(None);
                if let Some(next_task_id) = self.scheduler.next_ready_task() {
                    self.scheduler.set_current_task(Some(next_task_id));
                    self.load_or_init_task(next_task_id)?;
                    Ok(AwaitResult::FramePushed)
                } else {
                    Ok(AwaitResult::Yield(self.scheduler.pending_call_ids()))
                }
            }
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

        // If part of a gather, tear the gather down (caches the error,
        // cancels siblings, clears pending external routing) and switch into
        // the waiter to propagate the error.
        if let Some(gid) = gather_id {
            let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gid) else {
                panic!("task gather_id doesn't point to a GatherFuture")
            };
            let waiter_id = gather.fail(&mut self.scheduler, self.heap, &error);
            drop(gather);

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
                exception_stack_base: f.exception_stack_base,
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
                        exception_stack_base: sf.exception_stack_base,
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

        let exc_stack_base = self.exception_stack.len();
        self.push_frame(CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            exc_stack_base,
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

        // Pop the pending entry. A `None` here means the gather that was
        // routing this call has been torn down (`HeapRead::fail` clears its
        // pending entries) — drop the value, since `mark_consumed` was set
        // when the gather awaited so no future `await` of the same call can
        // pick it up. (Pre-refactor stored these in `resolved`; the value
        // was unreachable and only released at `Scheduler::cleanup`.)
        let Some(pending) = this.scheduler.take_pending_call(call_id) else {
            return Ok(());
        };

        // If the creator task of a non-gather call has failed, also drop the
        // result. (Gather-routed calls don't reach here in the failure case —
        // teardown removes the pending entry first.)
        if pending.gather.is_none() && this.scheduler.is_task_failed(pending.creator_task) {
            return Ok(());
        }

        if let Some(gather_id) = pending.gather {
            let value = value_guard.into_inner();
            let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gather_id) else {
                panic!("gather_id doesn't point to a GatherFuture")
            };

            // Record the resolution on the gather. `resolve_child` fans out to
            // duplicate slots, clears the resolved CallId from `pending_calls`,
            // and finalizes the gather (Success or Failure) if it's now done.
            match gather.resolve_child(self, ChildSource::ExternalCall(call_id), value)? {
                Some(GatherResolution::Success { list_id, waiter_id }) => {
                    drop(gather);

                    // External-future resolution can fire while either the
                    // waiter is itself the current task (frames live in the
                    // VM — e.g. external-only gather where the waiter never
                    // switched away) or the waiter's context is parked. We
                    // pick the right push target based on that.
                    let waiter_context_in_vm =
                        self.scheduler.current_task_id() == Some(waiter_id) && !self.frames.is_empty();

                    if waiter_context_in_vm {
                        self.stack.push(Value::Ref(list_id));
                        self.scheduler.set_state(waiter_id, TaskState::Ready, self.heap);
                    } else {
                        self.scheduler.get_task_mut(waiter_id).stack.push(Value::Ref(list_id));
                        self.scheduler.make_ready(waiter_id, self.heap);
                    }
                }
                Some(GatherResolution::Failure { error, waiter_id }) => {
                    drop(gather);

                    // Switch VM context into the waiter so the post-loop check
                    // in `resume_with_resolved_futures` sees `current=waiter`
                    // with state `Failed`, then calls `resume_with_exception`
                    // — which raises in the waiter's frame and lets the
                    // user's `try` / `except` catch it.
                    self.cleanup_current_task();
                    self.scheduler.set_current_task(Some(waiter_id));
                    self.load_or_init_task(waiter_id)?;
                    self.scheduler.set_state(waiter_id, TaskState::Failed(error), self.heap);
                }
                None => {}
            }
        } else {
            // Normal resolution for a single awaiter.
            let value = value_guard.into_inner();
            self.scheduler.record_resolved(call_id, value);
            // Unblock the waiting task, if it's still waiting.
            let task = self.scheduler.get_task_mut(pending.creator_task);
            if matches!(task.state, TaskState::BlockedOnCall(cid) if cid == call_id) {
                task.unblocked_by = Some(call_id);
            }
            self.scheduler.make_ready(pending.creator_task, self.heap);
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
                gather: None,
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
                    // Current task failed - resume with exception so it can be caught by
                    // surrounding `try/except`.
                    let TaskState::Failed(err) = mem::replace(&mut task.state, TaskState::Ready) else {
                        unreachable!();
                    };
                    return self.resume_with_exception(err);
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

/// Outcome of [`HeapRead::resolve_child`] when a gather has finished driving
/// its children.
///
/// - `Success`: every child resolved with a value. `list_id` is the cached
///   result list to hand back; `waiter_id` is the task that awaited the
///   gather.
/// - `Failure`: a sibling task ended up `TaskState::Failed` (typically
///   because an external it was waiting on was rejected by the host —
///   `Scheduler::fail_for_call`'s "indirect" branch sets that state). The
///   gather has been torn down via [`HeapRead::fail`]; the caller is
///   responsible for switching VM context to `waiter_id` and propagating
///   `error` through its frame's exception handler.
pub(crate) enum GatherResolution {
    Success { list_id: HeapId, waiter_id: TaskId },
    Failure { error: RunError, waiter_id: TaskId },
}

/// Identifies which child of a gather just produced a value.
///
/// Used by [`HeapRead::resolve_child`] to look up the child's slot indices
/// from the gather's own `pending_tasks` / `pending_calls` maps and, in the
/// external-future case, to clear the resolved entry.
#[derive(Clone, Copy)]
pub(crate) enum ChildSource {
    /// A spawned coroutine task completed with a value.
    Task(TaskId),
    /// An external future was resolved with a value by the host.
    ExternalCall(CallId),
}

impl<'h> HeapRead<'h, GatherFuture> {
    /// Caches `list_id` as the gather's successful result.
    ///
    /// Inc_refs `list_id` so the cached state and the caller both own a ref
    /// to the resulting list, then overwrites the state with
    /// `GatherState::Completed(list_id)`. Used directly by
    /// [`VM::await_gather_future`] for the synchronous-completion paths
    /// (empty gather, all externals already resolved); on the async path the
    /// transition happens inside [`Self::resolve_child`].
    pub(crate) fn cache_result(&mut self, heap: &mut HeapReader<'h, impl ResourceTracker>, list_id: HeapId) {
        heap.inc_ref(list_id);
        self.get_mut(heap).state = GatherState::Completed(list_id);
    }

    /// Records one child's resolution on this gather and, if everything has
    /// now settled, transitions the gather to `Completed` (success) or
    /// `Failed` (sibling failure detected).
    ///
    /// The child's slot-index mapping is removed from the gather's own
    /// `pending_tasks` / `pending_calls` map. Membership in those maps is
    /// the "still in flight" signal.
    ///
    /// Failure detection: a child task can end up in `TaskState::Failed`
    /// without the gather being torn down — this happens when an external
    /// call inside a gather child raises (`Scheduler::fail_for_call`'s
    /// indirect branch). Such Failed siblings linger in `pending_tasks`
    /// until *another* child completes and brings us through this scan,
    /// which then propagates via `self.fail`.
    ///
    /// Returns `None` while children are still in flight; otherwise
    /// `Some(GatherResolution::Success | Failure)`. In both cases the
    /// gather has been transitioned to a terminal state.
    fn resolve_child(
        &mut self,
        vm: &mut VM<'h, impl ResourceTracker>,
        source: ChildSource,
        value: Value,
    ) -> RunResult<Option<GatherResolution>> {
        // Remove this child's slot-index mapping.
        let indices: Vec<usize> = {
            let awaited = self
                .get_mut(vm.heap)
                .as_awaited_mut()
                .expect("resolve_child called on non-Awaited gather");
            match source {
                ChildSource::Task(tid) => awaited
                    .pending_tasks
                    .remove(&tid)
                    .expect("resolve_child: task not registered with this gather"),
                ChildSource::ExternalCall(cid) => awaited
                    .pending_calls
                    .remove(&cid)
                    .expect("resolve_child: external call not registered with this gather"),
            }
        };

        // Take `results` out so the writes can do their clones (which need
        // `&Heap` access) without fighting the `&mut`-chain that
        // `as_awaited_mut` requires. We put it back into the gather right
        // after, before the completion scan.
        let mut results = mem::take(
            &mut self
                .get_mut(vm.heap)
                .as_awaited_mut()
                .expect("resolve_child called on non-Awaited gather")
                .results,
        );
        if let Some((last, init)) = indices.split_last() {
            for &idx in init {
                results[idx] = Some(value.clone_with_heap(vm.heap));
            }
            results[*last] = Some(value);
        } else {
            value.drop_with_heap(vm.heap);
        }

        // Restore results and look for a Failed sibling or check completion.
        let awaited = self
            .get_mut(vm.heap)
            .as_awaited_mut()
            .expect("gather still Awaited after recording child resolution");
        awaited.results = results;

        // Check for failed siblings after recording the result.
        if let Some(&tid) = awaited
            .pending_tasks
            .keys()
            .find(|tid| matches!(vm.scheduler.get_task(**tid).state, TaskState::Failed(_)))
        {
            // Take the error out of the failed task's state, then tear
            // the gather down. `self.fail` cancels every remaining
            // child (including `tid` itself) and caches the error on
            // the gather for replay on re-await.
            let task = vm.scheduler.get_task_mut(tid);
            let TaskState::Failed(error) = mem::replace(&mut task.state, TaskState::Ready) else {
                unreachable!("scanned Failed above")
            };
            let waiter_id = self.fail(&mut vm.scheduler, vm.heap, &error);
            return Ok(Some(GatherResolution::Failure { error, waiter_id }));
        } else if !awaited.pending_tasks.is_empty() || !awaited.pending_calls.is_empty() {
            return Ok(None);
        }

        // All children resolved successfully — build the result list.
        let results = mem::take(&mut awaited.results);
        let waiter_id = awaited.waiter;
        let results: Vec<Value> = results
            .into_iter()
            .map(|r| r.expect("all results filled when gather is complete"))
            .collect();
        let list_id = vm.heap.allocate(HeapData::List(List::new(results)))?;
        self.cache_result(vm.heap, list_id);
        Ok(Some(GatherResolution::Success { list_id, waiter_id }))
    }

    /// Tear the gather down with `error` and return its waiter.
    ///
    /// Takes `&mut Scheduler` + `&mut HeapReader` rather than `&mut VM` so
    /// this works from both `VM::handle_task_failure` (has a VM, splits
    /// borrows on its fields) and `Scheduler::fail_for_call` (only has a
    /// scheduler + heap reader).
    pub(crate) fn fail(
        &mut self,
        scheduler: &mut Scheduler,
        heap: &mut HeapReader<'h, impl ResourceTracker>,
        error: &RunError,
    ) -> TaskId {
        // Take the Awaited bookkeeping. The state stays `Awaited` (with empty
        // fields) until the state replace below commits the transition.
        let (waiter_id, pending_tasks, pending_calls, results) = {
            let awaited = self
                .get_mut(heap)
                .as_awaited_mut()
                .expect("fail called on non-Awaited gather");
            (
                awaited.waiter,
                mem::take(&mut awaited.pending_tasks),
                mem::take(&mut awaited.pending_calls),
                mem::take(&mut awaited.results),
            )
        };

        // Cache a clone so re-awaits replay the same exception.
        self.get_mut(heap).state = GatherState::Failed(error.clone());

        // Drop fanned-out result Values that won't reach the waiter.
        results.drop_with_heap(heap);

        // Drop pending-call routing so late host-side resolutions don't reach
        // a stale gather entry. `take_pending_call` is a no-op when the entry
        // has already been cleared (e.g. by `Scheduler::fail_for_call` having
        // already removed the entry it was driving).
        for cid in pending_calls.into_keys() {
            scheduler.take_pending_call(cid);
        }

        // Release all tasks owned by this gather.
        for tid in pending_tasks.into_keys() {
            scheduler.cancel_task(tid, heap);
        }

        waiter_id
    }
}
