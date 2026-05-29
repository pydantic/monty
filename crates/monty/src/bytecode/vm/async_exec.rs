//! Async execution support for the VM.
//!
//! This module contains all async-related methods for the VM including:
//! - Awaiting coroutines, external futures, and gather futures
//! - Task scheduling and context switching
//! - Task completion and failure handling
//! - External future resolution

use std::{collections::hash_map::Entry, mem, task::Poll};

use ahash::AHashMap;
use smallvec::{SmallVec, smallvec};

use super::{AwaitResult, CallFrame, FrameExit, VM};
use crate::{
    MontyException,
    asyncio::{
        AwaitedGather, Awaiter, CallId, Coroutine, CoroutineState, ExternalFuture, ExternalFutureState, GatherFuture,
        GatherState, TaskId,
    },
    bytecode::vm::scheduler::{Scheduler, SerializedTaskFrame, TaskState},
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

        let awaiter = Awaiter::Task(
            this.scheduler
                .current_task_id()
                .expect("exec_get_awaitable called without a current task"),
        );

        match awaitable {
            Value::Ref(heap_id) => {
                let heap_id = *heap_id;
                let poll = match this.heap.read(heap_id) {
                    HeapReadOutput::Coroutine(coro) => return this.await_coroutine(coro),
                    HeapReadOutput::GatherFuture(gather) => this.await_gather_future(heap_id, gather, awaiter)?,
                    HeapReadOutput::ExternalFuture(mut fut) => this.await_external_future(&mut fut, awaiter)?,
                    _ => return Err(ExcType::object_not_awaitable(awaitable.py_type(this))),
                };
                match poll {
                    Poll::Ready(value) => Ok(AwaitResult::ValueReady(value)),
                    Poll::Pending => {
                        this.scheduler.block_current_on(heap_id, this.heap);
                        this.switch_or_yield()
                    }
                }
            }
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

    /// Awaits a gather future from the user's `await gather` site.
    fn await_gather_future(
        &mut self,
        gather_id: HeapId,
        mut gather: HeapRead<'h, GatherFuture>,
        awaiter: Awaiter,
    ) -> Result<Poll<Value>, RunError> {
        let mut awaiter_guard = HeapGuard::new(awaiter, self);
        let this = awaiter_guard.heap();
        match &gather.get(this.heap).state {
            GatherState::Pending => {}
            GatherState::Completed(value) => {
                return Ok(Poll::Ready(value.clone_with_heap(this.heap)));
            }
            GatherState::Failed(err) => {
                return Err(err.clone());
            }
            // TODO: support concurrent re-await (CPython does).
            GatherState::Awaited(_) => {
                return Err(SimpleException::new_msg(
                    ExcType::RuntimeError,
                    "cannot reuse gather that is currently being awaited",
                )
                .into());
            }
        }

        // Empty gather shortcut. Allocate the empty list, store it as the
        // cached `Completed` result, and return an inc_ref'd reference.
        let item_count = gather.get(this.heap).item_count();
        if item_count == 0 {
            let list_id = this.heap.allocate(HeapData::List(List::new(vec![])))?;
            gather.cache_result(this.heap, list_id);
            return Ok(Poll::Ready(Value::Ref(list_id)));
        }

        // Await all items, storing already resolved state and tracking the rest in `pending_children`.

        let mut pending_children: AHashMap<HeapId, SmallVec<[usize; 1]>> = AHashMap::new();
        let results: Vec<Option<Value>> = (0..item_count).map(|_| None).collect();
        let mut results_guard = HeapGuard::new(results, this);
        let (results, this) = results_guard.as_parts_mut();

        for (idx, result) in results.iter_mut().enumerate() {
            let item_id = gather.get(this.heap).items[idx];
            let vacant_entry = match pending_children.entry(item_id) {
                Entry::Occupied(occ) => {
                    // Dedup: We've already registered this item in this commit pass —
                    // this is a duplicate item (e.g. `gather(coro, coro)`). Just
                    // append the new slot index to the existing entry.
                    occ.into_mut().push(idx);
                    continue;
                }
                Entry::Vacant(vacant) => vacant,
            };

            let poll = match this.heap.read(item_id) {
                HeapReadOutput::Coroutine(_) => {
                    this.scheduler.spawn(this.heap, item_id, Some(gather_id));
                    Poll::Pending
                }
                HeapReadOutput::ExternalFuture(mut fut) => {
                    this.heap.inc_ref(gather_id);
                    let sub_awaiter = Awaiter::GatherSlot {
                        gather: gather_id,
                        source: item_id,
                    };
                    this.await_external_future(&mut fut, sub_awaiter)?
                }
                HeapReadOutput::GatherFuture(child_gather) => {
                    this.heap.inc_ref(gather_id);
                    let sub_awaiter = Awaiter::GatherSlot {
                        gather: gather_id,
                        source: item_id,
                    };
                    this.await_gather_future(item_id, child_gather, sub_awaiter)?
                }
                _ => panic!("gather item is not a Coroutine, ExternalFuture, or GatherFuture"),
            };

            match poll {
                Poll::Ready(value) => {
                    *result = Some(value);
                }
                Poll::Pending => {
                    vacant_entry.insert(smallvec![idx]);
                }
            }
        }

        if pending_children.is_empty() {
            // All items resolved synchronously — skip straight to Completed with the result list.
            let (results, this) = results_guard.into_parts();
            let results: Vec<Value> = results
                .into_iter()
                .map(|r| r.expect("all results filled for synchronous gather completion"))
                .collect();
            let list_id = this.heap.allocate(HeapData::List(List::new(results)))?;
            gather.cache_result(this.heap, list_id);
            return Ok(Poll::Ready(Value::Ref(list_id)));
        }

        let results = results_guard.into_inner();
        let (awaiter, this) = awaiter_guard.into_parts();
        gather.get_mut(this.heap).state = GatherState::Awaited(AwaitedGather {
            awaiter,
            pending_children,
            results,
        });

        Ok(Poll::Pending)
    }

    /// Awaits an external future by inspecting its heap state.
    ///
    /// - `Resolved(v)` returns a clone of `v` immediately.
    /// - `Failed(e)` re-raises a clone of `e`.
    /// - `Pending { awaiter: None }` installs the current task as the awaiter,
    ///   and returns `Poll::Pending`.
    /// - `Pending { awaiter: Some(_) }` is rejected as a double-await — a
    ///   single-awaiter restriction we keep until multi-awaiter wake/raise
    ///   plumbing lands.
    fn await_external_future(
        &mut self,
        fut: &mut HeapRead<'h, ExternalFuture>,
        awaiter: Awaiter,
    ) -> Result<Poll<Value>, RunError> {
        let mut awaiter_guard = HeapGuard::new(awaiter, self);
        let this = awaiter_guard.heap();
        match &fut.get(this.heap).state {
            ExternalFutureState::Resolved(value) => {
                let value = value.clone_with_heap(this);
                Ok(Poll::Ready(value))
            }
            ExternalFutureState::Failed(err) => Err(err.clone()),
            ExternalFutureState::Pending { awaiter: Some(_) } => {
                Err(SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited future").into())
            }
            ExternalFutureState::Pending { awaiter: None } => {
                let awaiter = awaiter_guard.into_inner();
                fut.get_mut(self.heap).state = ExternalFutureState::Pending { awaiter: Some(awaiter) };
                Ok(Poll::Pending)
            }
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

        // Track memory for the locals region. Symmetric with
        // `cleanup_frame_state`. Comprehension variables live on the operand
        // stack (pushed per-comp).
        let size = namespace_values.len() * mem::size_of::<Value>();
        self.heap.tracker_mut().on_allocate(|| size)?;

        // Extend the stack with the coroutine's pre-bound locals.
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
        let coroutine_id = task
            .coroutine_id
            .expect("handle_task_completion: spawned task without a coroutine");

        // Mark the coroutine as Completed before the task is cancelled —
        // direct `await` of this coroutine elsewhere needs to see the new
        // state, not the `Running` it had until now.
        let HeapReadOutput::Coroutine(mut coro) = self.heap.read(coroutine_id) else {
            panic!("task coroutine_id doesn't point to a Coroutine")
        };
        coro.get_mut(self.heap).state = CoroutineState::Completed;
        drop(coro);

        // Record the result on the gather and check whether it's now complete.
        // `resolve_child` does the fan-out for duplicate slots (`gather(c, c)`)
        // and the final state transition; it must run BEFORE we release any
        // inc_refs the gather is holding (cancelling children, dropping the
        // waiter's `Blocked` ref) — otherwise the gather can be freed while
        // we're still about to write its cached state. The gather keys by
        // item HeapId, so we pass the coroutine's id rather than the
        // (kind-specific) task id.
        let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gid) else {
            panic!("task gather_id doesn't point to a GatherFuture")
        };
        let resolution = gather.resolve_child(self, coroutine_id, result)?;
        drop(gather);

        // The just-completed task is no longer in the gather's
        // `pending_tasks` map. Cancel it now to release its inc_refs on the
        // coroutine and gather; otherwise it would linger in the scheduler.
        self.scheduler.cancel_task(task_id, self.heap);

        let delivery = match resolution {
            Some(success) => self.deliver_awaiter_success(success.awaiter, Value::Ref(success.list_id))?,
            None => None,
        };

        let next_task_id = if let Some(waiter_id) = delivery {
            // `deliver_awaiter_success` already pushed the result onto the
            // waiter's stack and queued it Ready. Switch directly into the
            // waiter — `remove_from_ready_queue` cancels the queue entry
            // since we're not going through the run loop's scheduler pop.
            self.scheduler.remove_from_ready_queue(waiter_id);
            Some(waiter_id)
        } else {
            self.scheduler.next_ready_task()
        };

        self.cleanup_current_task();

        if let Some(next_id) = next_task_id {
            self.scheduler.set_current_task(Some(next_id));
            self.load_or_init_task(next_id)?;
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

        // If part of a gather, tear the gather down (caches the error,
        // cancels siblings, clears pending external routing) and walk the
        // awaiter chain (which may go through outer nested gathers) to reach
        // the task that should resume with the exception.
        if let Some(gid) = gather_id {
            let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(gid) else {
                panic!("task gather_id doesn't point to a GatherFuture")
            };
            let awaiter = gather.fail(&mut self.scheduler, self.heap, &error);
            drop(gather);
            if let Some(waiter_id) = self.deliver_awaiter_failure(awaiter, error.clone()) {
                // `deliver_awaiter_failure` set the waiter to `Failed`, but
                // we propagate the exception via `Err` (the run loop's
                // `handle_exception` raises in the waiter's frame), so the
                // task should be running. Override to `Ready` before
                // switching in.
                self.scheduler.set_state(waiter_id, TaskState::Ready, self.heap);
                self.cleanup_current_task();
                self.scheduler.set_current_task(Some(waiter_id));
                self.load_or_init_task(waiter_id)?;
            }
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

        // Resolutions that landed while this task was parked already pushed
        // their value onto `task.stack` (via `deliver_value_to_task` or
        // `handle_task_completion`'s waiter-handoff branch), so the restored
        // stack above is already in the post-AWAIT shape.

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

        // Track memory for the locals region. Symmetric with
        // `cleanup_frame_state`. Comprehension variables live on the operand
        // stack (pushed per-comp).
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
    /// Called by the host when an async external call completes. Looks up
    /// the `ExternalFuture` heap entry for `call_id`, transitions it to
    /// `Resolved(value)`, and delivers `value` to the awaiter (if any).
    pub fn resolve_future(&mut self, call_id: u32, value: Value) -> RunResult<()> {
        let call_id = CallId::new(call_id);

        let Some(future_id) = self.scheduler.take_pending_external(call_id) else {
            value.drop_with_heap(self);
            return Ok(());
        };

        // Ensure future cleaned up on all paths
        let fut_val = Value::Ref(future_id);
        let this = self;
        defer_drop!(fut_val, this);

        let mut value_guard = HeapGuard::new(value, this);
        let (value, this) = value_guard.as_parts_mut();

        let HeapReadOutput::ExternalFuture(mut fut) = this.heap.read(future_id) else {
            panic!("pending_externals entry doesn't point to an ExternalFuture")
        };

        let awaiter_and_value = match &mut fut.get_mut(this.heap).state {
            ExternalFutureState::Pending { awaiter } => awaiter.take().map(|a| (a, value.clone_with_heap(this.heap))),
            ExternalFutureState::Resolved(_) | ExternalFutureState::Failed(_) => {
                panic!("resolve_future: future was already resolved")
            }
        };

        let (value, this) = value_guard.into_parts();
        fut.get_mut(this.heap).state = ExternalFutureState::Resolved(value);

        if let Some((awaiter, value)) = awaiter_and_value {
            this.deliver_awaiter_success(awaiter, value)?;
        }

        Ok(())
    }

    /// Pushes `value` onto `task_id`'s stack and marks it ready. If the task
    /// has already been cancelled (no longer in the scheduler) or failed,
    /// drops `value` instead — the resolution still gets cached on the future,
    /// but the (now-gone) awaiter doesn't receive it.
    fn deliver_value_to_task(&mut self, task_id: TaskId, value: Value) {
        if !self.scheduler.has_task(task_id) || self.scheduler.is_task_failed(task_id) {
            value.drop_with_heap(self);
            return;
        }

        let task_is_current = self.scheduler.current_task_id() == Some(task_id) && !self.frames.is_empty();
        if task_is_current {
            self.stack.push(value);
        } else {
            self.scheduler.get_task_mut(task_id).stack.push(value);
        }
        self.scheduler.make_ready(task_id, self.heap);
    }

    /// Delivers `value` along the awaiter chain starting at `awaiter`.
    ///
    /// At each `Awaiter::GatherSlot` link, the value is fanned into the
    /// outer gather via [`HeapRead::resolve_child`]; if that completes the
    /// outer, the chain continues with the outer's own awaiter and result
    /// list. At an `Awaiter::Task` terminal, the value is delivered via
    /// [`Self::deliver_value_to_task`] (push to the task's stack, transition
    /// to `Ready`, push to ready-queue).
    ///
    /// Returns:
    /// - `Some(task_id)` if delivery reached a live task — the caller may
    ///   optionally switch VM context into `task_id` (calling
    ///   `remove_from_ready_queue` first since `deliver_value_to_task`
    ///   already queued it).
    /// - `None` if the chain was consumed by an intermediate gather that's
    ///   still in flight, or if the terminal task is gone (in which case
    ///   the value is dropped).
    fn deliver_awaiter_success(&mut self, mut awaiter: Awaiter, mut value: Value) -> RunResult<Option<TaskId>> {
        let this = self;
        loop {
            match awaiter {
                Awaiter::Task(t) => {
                    this.deliver_value_to_task(t, value);
                    return Ok(Some(t));
                }
                Awaiter::GatherSlot { gather, source } => {
                    let gather_val = Value::Ref(gather);
                    defer_drop!(gather_val, this);
                    let HeapReadOutput::GatherFuture(mut outer) = this.heap.read(gather) else {
                        panic!("Awaiter::GatherSlot gather id is not a GatherFuture")
                    };
                    let next = outer.resolve_child(this, source, value)?;
                    match next {
                        Some(success) => {
                            awaiter = success.awaiter;
                            value = Value::Ref(success.list_id);
                        }
                        None => return Ok(None),
                    }
                }
            }
        }
    }

    /// Walks the awaiter chain starting at `awaiter`, tearing each
    /// intermediate gather down with `error`, and fails the terminal task.
    ///
    /// Returns:
    /// - `Some(task_id)` if failure reached a live task — the caller may
    ///   optionally switch VM context into it; the task's state is already
    ///   `Failed(error)` so `resume_with_resolved_futures`'s post-loop check
    ///   will raise the exception when control returns. (Callers that need
    ///   the task in `Ready` instead — `handle_task_failure` — should
    ///   `set_state(t, Ready)` before switching, since the exception is
    ///   propagated by the `Err` return rather than the state check.)
    /// - `None` if the terminal task is gone.
    fn deliver_awaiter_failure(&mut self, mut awaiter: Awaiter, error: RunError) -> Option<TaskId> {
        let target = loop {
            match awaiter {
                Awaiter::Task(t) => break t,
                Awaiter::GatherSlot { gather, .. } => {
                    let HeapReadOutput::GatherFuture(mut outer) = self.heap.read(gather) else {
                        panic!("Awaiter::GatherSlot gather id is not a GatherFuture")
                    };
                    let next = outer.fail(&mut self.scheduler, self.heap, &error);
                    drop(outer);
                    // Release the inc_ref the destructured awaiter owned on
                    // `gather`; reassign to the next link in the chain.
                    self.heap.dec_ref(gather);
                    awaiter = next;
                }
            }
        };
        if !self.scheduler.has_task(target) {
            return None;
        }
        self.scheduler.fail_task(target, error, self.heap);
        Some(target)
    }

    /// Fails an external future with an error.
    ///
    /// Called by the host when an async external call fails with an
    /// exception. Asks the scheduler for the awaiter that should receive
    /// the failure (see `Scheduler::fail_for_call`), walks it via
    /// [`Self::deliver_awaiter_failure`] (which fails the terminal task),
    /// and switches VM context into that task if it isn't already current —
    /// `resume_with_resolved_futures`'s post-loop check then surfaces the
    /// error through its frame.
    pub fn fail_future(&mut self, call_id: u32, error: RunError) -> RunResult<()> {
        let call_id = CallId::new(call_id);
        if let Some(awaiter) = self.scheduler.fail_for_call(call_id, &error, self.heap)
            && let Some(waiter_id) = self.deliver_awaiter_failure(awaiter, error)
            && self.scheduler.current_task_id() != Some(waiter_id)
        {
            self.cleanup_current_task();
            self.scheduler.set_current_task(Some(waiter_id));
            self.load_or_init_task(waiter_id)?;
        }
        Ok(())
    }

    /// Allocates an `ExternalFuture` for `call_id` and pushes a `Value::Ref`
    /// to it on the VM stack.
    ///
    /// The scheduler indexes `call_id -> future_id` (with its own inc_ref) so
    /// host resolutions can find the heap entry; the `Value::Ref` pushed onto
    /// the stack is the user's reference, which travels with the value until
    /// it's awaited or dropped.
    pub fn add_pending_call(&mut self, call_id: CallId) -> RunResult<()> {
        let future_id = self
            .heap
            .allocate(HeapData::ExternalFuture(ExternalFuture::new_pending(call_id)))?;
        self.scheduler.add_pending_external(call_id, future_id, self.heap);
        self.push(Value::Ref(future_id));
        Ok(())
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
                ExtFunctionResult::Error(exc) => self.fail_future(call_id, RunError::from(exc))?,
                ExtFunctionResult::Future(_) => {}
                ExtFunctionResult::NotFound(function_name) => {
                    self.fail_future(call_id, ExtFunctionResult::not_found_exc(&function_name))?;
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
                TaskState::Blocked(_) => {
                    // Current task is still blocked on unresolved futures.
                }
                TaskState::Ready => {
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
/// its children successfully.
///
/// `list_id` is the cached result list to hand back; `waiter` is the
/// downstream that should receive it.
pub(crate) struct GatherSuccess {
    pub list_id: HeapId,
    pub awaiter: Awaiter,
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
        self.get_mut(heap).state = GatherState::Completed(Value::Ref(list_id));
    }

    /// Records one child's resolution on this gather and, if everything has
    /// now settled, transitions the gather to `Completed`.
    ///
    /// The child's slot-index mapping is removed from the gather's
    /// `pending_children` map. Membership in that map is the "still in
    /// flight" signal.
    ///
    /// Failure cases never reach this method — sibling failures are routed
    /// through [`HeapRead::fail`] at the failure site
    /// (`Scheduler::fail_for_call` for external rejections,
    /// `VM::handle_task_failure` for in-frame exceptions). Both eagerly tear
    /// the gather down before any other sibling has a chance to resolve.
    ///
    /// Returns `None` while children are still in flight; otherwise
    /// `Some(GatherResolution::Success)` with the cached result list.
    fn resolve_child(
        &mut self,
        vm: &mut VM<'h, impl ResourceTracker>,
        child_id: HeapId,
        value: Value,
    ) -> RunResult<Option<GatherSuccess>> {
        // Remove this child's slot-index mapping.
        let indices: SmallVec<[usize; 1]> = self
            .get_mut(vm.heap)
            .as_awaited_mut()
            .expect("resolve_child called on non-Awaited gather")
            .pending_children
            .remove(&child_id)
            .expect("resolve_child: child not registered with this gather");

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

        // Restore results and check completion.
        let awaited = self
            .get_mut(vm.heap)
            .as_awaited_mut()
            .expect("gather still Awaited after recording child resolution");
        awaited.results = results;

        if !awaited.pending_children.is_empty() {
            return Ok(None);
        }

        // All children resolved successfully — build the result list.
        // Extract this gather's awaiter (transferred into the returned
        // `GatherSuccess`); the `Awaited` state remains in place until
        // `cache_result` overwrites it, but its `awaiter` field is now the
        // placeholder so dropping the `Awaited` payload won't double-drop
        // the owned `Awaiter`.
        let results = mem::take(&mut awaited.results);
        let awaiter = mem::replace(&mut awaited.awaiter, Awaiter::Task(TaskId::default()));
        let results: Vec<Value> = results
            .into_iter()
            .map(|r| r.expect("all results filled when gather is complete"))
            .collect();
        let list_id = vm.heap.allocate(HeapData::List(List::new(results)))?;
        self.cache_result(vm.heap, list_id);
        Ok(Some(GatherSuccess { list_id, awaiter }))
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
    ) -> Awaiter {
        // Take the Awaited bookkeeping. The state stays `Awaited` (with
        // placeholder fields) until the state replace below commits the
        // transition. The extracted `awaiter` is transferred to the caller
        // — it owns any `GatherSlot` inc_ref it carried.
        let (waiter, pending_children, results) = {
            let awaited = self
                .get_mut(heap)
                .as_awaited_mut()
                .expect("fail called on non-Awaited gather");
            (
                mem::replace(&mut awaited.awaiter, Awaiter::Task(TaskId::default())),
                mem::take(&mut awaited.pending_children),
                mem::take(&mut awaited.results),
            )
        };

        // Cache a clone so re-awaits replay the same exception.
        self.get_mut(heap).state = GatherState::Failed(error.clone());

        // Drop fanned-out result Values that won't reach the waiter.
        results.drop_with_heap(heap);

        // Walk every child the gather was waiting on and clean it up
        // appropriately. `heap.read(id)` recovers the kind:
        // - Coroutine: find its driving task via the scheduler's
        //   `task_for_coroutine` index and cancel it, releasing the task's
        //   inc_refs on the gather and coroutine.
        // - ExternalFuture: clear the awaiter slot so a late host resolution
        //   doesn't fan into the now-failed gather. The future stays in
        //   `pending_externals` and a `Pending { awaiter: None }` resolution
        //   simply caches the value (and a re-await replays it).
        // - GatherFuture (nested): recurse — tear it down with the same
        //   error so its own tasks/externals/gathers are cleaned up too.
        for child_id in pending_children.into_keys() {
            match heap.read(child_id) {
                HeapReadOutput::Coroutine(_) => {
                    if let Some(tid) = scheduler.task_for_coroutine(child_id) {
                        scheduler.cancel_task(tid, heap);
                    }
                }
                HeapReadOutput::ExternalFuture(mut fut) => {
                    // Take the awaiter out so the inc_ref it holds on this
                    // (failing) gather is released.
                    if let ExternalFutureState::Pending { awaiter } = &mut fut.get_mut(heap).state
                        && let Some(old) = awaiter.take()
                    {
                        old.drop_with_heap(heap);
                    }
                }
                HeapReadOutput::GatherFuture(mut nested) => {
                    // The nested gather's waiter is `Awaiter::GatherSlot {
                    // gather: self, .. }` — we own its routing and are now
                    // gone, so drop the returned awaiter (releasing its
                    // inc_ref back on us) rather than forwarding the chain.
                    let nested_awaiter = nested.fail(scheduler, heap, error);
                    nested_awaiter.drop_with_heap(heap);
                }
                _ => panic!("gather pending_children key is not a Coroutine, ExternalFuture, or GatherFuture"),
            }
        }

        waiter
    }
}
