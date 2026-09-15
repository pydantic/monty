//! Async execution support for the VM.
//!
//! This module contains all async-related methods for the VM including:
//! - Awaiting coroutines, external futures, and gather futures
//! - Task scheduling and context switching
//! - Task completion and failure handling
//! - External future resolution

use std::{mem, task::Poll};

use monty_types::{InvalidInputError, MontyException, ResourceError, ResourceTracker};
use smallvec::{SmallVec, smallvec};

use super::{AwaitResult, CallFrame, FrameExit, Opcode, VM};
use crate::{
    asyncio::{
        AwaitedGather, Awaiter, CallId, Coroutine, CoroutineState, ExternalFuture, ExternalFutureState, GatherFuture,
        GatherState, PendingChildren, TaskId,
    },
    bytecode::vm::scheduler::SerializedTaskFrame,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{
        ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapObjectRead, HeapRead, HeapReadOutput,
        HeapReader,
    },
    intern::FunctionId,
    object_bridge::MontyValueExt,
    run_progress::{ExtFunctionResult, ExtFunctionResultExt},
    types::List,
    value::Value,
};

impl<'h> VM<'h> {
    /// Allows eager host resolution only for an immediate await with no competing work.
    pub(crate) fn allow_eager_await(&self) -> bool {
        let frame = self.current_frame();
        frame.bytecode.get(frame.ip) == Some(&(Opcode::Await as u8)) && self.scheduler.can_await_eagerly()
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
                    HeapReadOutput::GatherFuture(gather) => this.await_gather_future(gather, awaiter)?,
                    HeapReadOutput::ExternalFuture(mut fut) => this.await_external_future(&mut fut, awaiter)?,
                    _ => return Err(ExcType::object_not_awaitable(&awaitable.py_type_name(this))),
                };
                match poll {
                    Poll::Ready(value) => Ok(AwaitResult::ValueReady(value)),
                    Poll::Pending => {
                        this.scheduler.block_current_on(heap_id, this.heap);
                        this.switch_or_yield()
                    }
                }
            }
            _ => Err(ExcType::object_not_awaitable(&awaitable.py_type_name(this))),
        }
    }

    /// Awaits a coroutine by pushing a frame to execute it.
    ///
    /// Validates the coroutine is in `New` state, extracts its captured namespace
    /// and cells, marks it as `Running`, and pushes a frame to execute the coroutine body.
    fn await_coroutine(&mut self, mut coro: HeapObjectRead<'h, Coroutine>) -> Result<AwaitResult, RunError> {
        // Check if coroutine can be awaited (must be New)
        if coro.get(self.heap).state != CoroutineState::New {
            return Err(ExcType::cannot_reuse_already_awaited_coroutine());
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
    ///
    /// A settled gather replays its cached result; a `Pending` one is handed to
    /// [`Self::commit_gather_tree`], which commits it along with every gather
    /// nested inside it.
    fn await_gather_future(
        &mut self,
        gather: HeapObjectRead<'h, GatherFuture>,
        awaiter: Awaiter,
    ) -> Result<Poll<Value>, RunError> {
        let mut awaiter_guard = DropGuard::new(awaiter, self);
        let this = awaiter_guard.ctx();
        if let Some(value) = poll_settled_gather(&gather, this.heap)? {
            Ok(Poll::Ready(value))
        } else {
            // The walk re-reads each gather by id, so hand the handle back
            // before it starts.
            let gather_id = gather.id();
            drop(gather);
            let (awaiter, this) = awaiter_guard.into_parts();
            this.commit_gather_tree(gather_id, awaiter)
        }
    }

    /// Commits `root` and every gather nested inside it, reporting whether the
    /// root settled synchronously.
    ///
    /// Nesting a gather costs no Python frames (`g = asyncio.gather(g)` in a
    /// loop), so the recursion limit never saw the commit walk that descends
    /// through it — a recursive walk turned heap-held nesting back into native
    /// frames and aborted the process. The frames it needed now live in
    /// `stack`, one [`GatherCommit`] per level, which makes nesting depth a
    /// matter of memory rather than of native stack, as it is for CPython's
    /// loop-driven futures.
    fn commit_gather_tree(&mut self, root: HeapId, awaiter: Awaiter) -> Result<Poll<Value>, RunError> {
        let mut stack = vec![self.open_gather_commit(root, awaiter, None)];
        let mut steps = 0;
        let outcome = loop {
            // Each level costs a frame on the way down and a result list on the
            // way back up, and the whole walk runs no bytecode, so
            // `dispatch_checkpoint` never sees how far it has grown. Without
            // this check a nest that fits under `max_memory` commits its way
            // past the allocator's hard ceiling, taking the worker down instead
            // of ending the run.
            if let Err(err) = self.heap.tracker.check_memory_time_every(steps) {
                break Err(err.into());
            }
            steps += 1;

            let frame = stack.last_mut().expect("the commit stack is emptied only by returning");
            match self.step_gather_commit(frame) {
                // Descend: the nested gather's own items must be committed
                // before the frame that owns its result slot can continue.
                Ok(Some(nested)) => {
                    if let Err(err) = check_commit_stack_growth(stack.len(), stack.capacity(), &self.heap.tracker) {
                        nested.drop_with(self.heap);
                        break Err(err.into());
                    }
                    stack.push(nested);
                }
                Ok(None) => {
                    let frame = stack.pop().expect("the frame just stepped is still on the stack");
                    let (child_id, slot) = (frame.gather, frame.parent_slot);
                    let poll = self.settle_gather_commit(frame);
                    match stack.last_mut() {
                        // The root settled: `poll` is what the `await` site sees.
                        None => break Ok(poll),
                        Some(parent) => {
                            let slot = slot.expect("only the root commit frame has no parent slot");
                            parent.resume_after_child(child_id, slot, poll);
                        }
                    }
                }
                Err(err) => break Err(err),
            }
        };

        match outcome {
            Ok(poll) => Ok(poll),
            Err(err) => {
                self.unwind_gather_commits(stack, &err);
                Err(err)
            }
        }
    }

    /// Opens a commit frame for the `Pending` gather `gather`, sized to its items.
    fn open_gather_commit(&mut self, gather: HeapId, awaiter: Awaiter, parent_slot: Option<usize>) -> GatherCommit {
        let HeapReadOutput::GatherFuture(item_source) = self.heap.read(gather) else {
            panic!("gather commit frame id is not a GatherFuture")
        };
        let item_count = item_source.get(self.heap).item_count();
        GatherCommit {
            gather,
            awaiter,
            parent_slot,
            next: 0,
            results: (0..item_count).map(|_| None).collect(),
            pending_children: PendingChildren::new(),
        }
    }

    /// Commits `frame`'s items left-to-right into result slots or pending
    /// children, spawning coroutine children and installing awaiters on
    /// external futures as it goes.
    ///
    /// Stops early at a `Pending` nested gather, returning the frame the caller
    /// must commit before this one can continue; `Ok(None)` means every item is
    /// committed. Any error leaves the items handled so far in `frame`, which
    /// [`Self::unwind_gather_commits`] settles.
    fn step_gather_commit(&mut self, frame: &mut GatherCommit) -> Result<Option<GatherCommit>, RunError> {
        let HeapReadOutput::GatherFuture(gather) = self.heap.read(frame.gather) else {
            panic!("gather commit frame id is not a GatherFuture")
        };
        let gather_id = frame.gather;

        while frame.next < frame.results.len() {
            let idx = frame.next;
            let item_id = gather.get(self.heap).items[idx];
            if let Some(slots) = frame.pending_children.get_mut(&item_id) {
                // Dedup: We've already registered this item in this commit pass —
                // this is a duplicate item (e.g. `gather(coro, coro)`). Just
                // append the new slot index to the existing entry.
                slots.push(idx);
                frame.next += 1;
                continue;
            }

            let poll = match self.heap.read(item_id) {
                HeapReadOutput::Coroutine(coro) => {
                    // Reject reuse up-front: either the coroutine is no longer
                    // `New`, or another gather already spawned it (`spawn`
                    // returns `Ok(None)`).
                    if coro.get(self.heap).state != CoroutineState::New
                        || self.scheduler.spawn(self.heap, item_id, Some(gather_id)).is_none()
                    {
                        return Err(ExcType::cannot_reuse_already_awaited_coroutine());
                    }
                    Poll::Pending
                }
                HeapReadOutput::ExternalFuture(mut fut) => {
                    self.heap.inc_ref(gather_id);
                    let sub_awaiter = Awaiter::GatherSlot {
                        gather: gather_id,
                        source: item_id,
                    };
                    self.await_external_future(&mut fut, sub_awaiter)?
                }
                HeapReadOutput::GatherFuture(child_gather) => {
                    if let Some(value) = poll_settled_gather(&child_gather, self.heap)? {
                        Poll::Ready(value)
                    } else {
                        drop(child_gather);
                        // Both the inc_ref and the awaiter that owns it are
                        // handed to the nested frame, which releases them when
                        // it settles; until then nothing is committed for this
                        // slot, so an error above needs no cleanup here.
                        self.heap.inc_ref(gather_id);
                        let sub_awaiter = Awaiter::GatherSlot {
                            gather: gather_id,
                            source: item_id,
                        };
                        return Ok(Some(self.open_gather_commit(item_id, sub_awaiter, Some(idx))));
                    }
                }
                _ => panic!("gather item is not a Coroutine, ExternalFuture, or GatherFuture"),
            };

            match poll {
                Poll::Ready(value) => frame.results[idx] = Some(value),
                Poll::Pending => {
                    frame.pending_children.insert(item_id, smallvec![idx]);
                }
            }
            frame.next += 1;
        }

        Ok(None)
    }

    /// Settles a fully-committed frame.
    ///
    /// With nothing left in flight the gather goes straight to `Completed` with
    /// its result list (this covers the empty `gather()` too); otherwise it
    /// parks in `Awaited`, taking over the frame's bookkeeping, and later
    /// resolutions drive it from there.
    fn settle_gather_commit(&mut self, frame: GatherCommit) -> Poll<Value> {
        let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(frame.gather) else {
            panic!("gather commit frame id is not a GatherFuture")
        };

        if frame.pending_children.is_empty() {
            let results: Vec<Value> = frame
                .results
                .into_iter()
                .map(|r| r.expect("all results filled for synchronous gather completion"))
                .collect();
            let list_id = self.heap.allocate(HeapData::List(List::new(results)));
            gather.cache_result(self.heap, list_id);
            frame.awaiter.drop_with(self.heap);
            Poll::Ready(Value::Ref(list_id))
        } else {
            gather.get_mut(self.heap).state = GatherState::Awaited(AwaitedGather {
                awaiter: frame.awaiter,
                pending_children: frame.pending_children,
                results: frame.results,
            });
            Poll::Pending
        }
    }

    /// Fails every frame left on the commit stack with `err`, innermost first.
    ///
    /// This is the unwind the recursive walk got from `?`: each level caches the
    /// error so re-awaits replay it, and drops the result slots and awaiter the
    /// frame was holding. The children it had already committed keep running and
    /// keep pointing at their now-`Failed` gather, as they do for a failure that
    /// arrives after the commit pass (see [`HeapRead::fail`]).
    fn unwind_gather_commits(&mut self, stack: Vec<GatherCommit>, err: &RunError) {
        for frame in stack.into_iter().rev() {
            let HeapReadOutput::GatherFuture(mut gather) = self.heap.read(frame.gather) else {
                panic!("gather commit frame id is not a GatherFuture")
            };
            gather.get_mut(self.heap).state = GatherState::Failed(err.clone());
            drop(gather);

            frame.drop_with(self.heap);
        }
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
        let mut awaiter_guard = DropGuard::new(awaiter, self);
        let this = awaiter_guard.ctx();
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
        let call_offset = self.current_offset();
        let func = self.interns.get_function(func_id);
        let locals_count = u16::try_from(namespace_values.len()).expect("coroutine namespace size exceeds u16");

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
            call_offset,
        ))?;

        Ok(())
    }

    /// Selects another task after blocking, or yields to the host with the current context intact.
    fn switch_or_yield(&mut self) -> Result<AwaitResult, RunError> {
        if let Some(next_task_id) = self.scheduler.next_ready_task() {
            self.activate_task(next_task_id)?;
            Ok(AwaitResult::FramePushed)
        } else {
            Ok(AwaitResult::Yield(self.scheduler.pending_call_ids()))
        }
    }

    /// Removes a completed child and delivers its result, discarding it if its gather already settled.
    /// Resumes the gather's waiter when available, otherwise the next queued task.
    pub(super) fn handle_task_completion(&mut self, result: Value) -> Result<AwaitResult, RunError> {
        let task_id = self
            .scheduler
            .current_task_id()
            .expect("handle_task_completion called without current task");
        // The awaiter keeps the gather alive across task teardown.
        let task = self.scheduler.get_task_mut(task_id);
        let awaiter = task.awaiter.take();
        let coroutine_id = task
            .coroutine_id
            .expect("handle_task_completion: spawned task without a coroutine");

        // Re-awaiting the coroutine must see that its execution has ended.
        let HeapReadOutput::Coroutine(mut coro) = self.heap.read(coroutine_id) else {
            panic!("task coroutine_id doesn't point to a Coroutine")
        };
        coro.get_mut(self.heap).state = CoroutineState::Completed;
        drop(coro);

        self.scheduler.cancel_task(task_id, self.heap);

        let delivery = if let Some(awaiter) = awaiter {
            self.deliver_awaiter_success(awaiter, result)
        } else {
            result.drop_with(self);
            None
        };

        self.cleanup_current_task();

        if self.resume_after_task_exit(delivery)? {
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

    /// Removes a child whose exception escaped its frames and notifies its gather.
    /// A settled gather discards the error. Any returned error belongs to the newly active task
    /// and must go through exception handling before that task executes bytecode.
    pub(super) fn handle_task_failure(&mut self, error: RunError) -> Result<(), RunError> {
        let task_id = self
            .scheduler
            .current_task_id()
            .expect("handle_task_failure called without current task");
        debug_assert!(!task_id.is_main(), "handle_task_failure called for main task");

        let awaiter = self.scheduler.get_task_mut(task_id).awaiter.take();
        let delivery = awaiter.and_then(|awaiter| self.deliver_awaiter_failure(awaiter, error));
        self.cleanup_current_task();
        self.scheduler.cancel_task(task_id, self.heap);
        self.resume_after_task_exit(delivery)?;
        Ok(())
    }

    /// Selects the task awakened by a child's exit, or the next queued task if none was awakened.
    /// The exiting task and its VM context must already be discarded. Returns false if all tasks block.
    fn resume_after_task_exit(&mut self, waiter: Option<TaskId>) -> RunResult<bool> {
        let next = if let Some(waiter) = waiter {
            self.scheduler.remove_from_ready_queue(waiter);
            Some(waiter)
        } else {
            self.scheduler.next_ready_task()
        };
        if let Some(task_id) = next {
            self.activate_task(task_id)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Saves the current VM context into the given task in the scheduler.
    ///
    /// Serializes frames, moves stack/exception_stack, stores instruction_ip,
    /// and adjusts the global recursion depth counter.
    fn save_task_context(&mut self, task_id: TaskId) {
        let mut frames: Vec<SerializedTaskFrame> = self
            .suspended_frames
            .drain(..)
            .map(|f| SerializedTaskFrame {
                function_id: f.function_id,
                ip: f.ip,
                stack_base: f.stack_base,
                locals_count: f.locals_count,
                exception_stack_base: f.exception_stack_base,
                call_offset: f.call_offset,
                is_initializer: f.is_initializer,
            })
            .collect();
        let current = &self.current_frame;
        frames.push(SerializedTaskFrame {
            function_id: current.function_id,
            ip: current.ip,
            stack_base: current.stack_base,
            locals_count: current.locals_count,
            exception_stack_base: current.exception_stack_base,
            call_offset: current.call_offset,
            is_initializer: current.is_initializer,
        });

        // Count this task's recursion depth contribution and subtract it from
        // the global counter so the next task gets a clean budget.
        let task_depth = frames.len().saturating_sub(1); // root frame doesn't contribute to recursion depth
        self.recursion_depth -= task_depth;

        // Save VM state into the task
        let task = self.scheduler.get_task_mut(task_id);
        task.frames = frames;
        task.stack = mem::take(&mut self.stack);
        task.exception_stack = mem::take(&mut self.exception_stack);
        task.instruction_ip = self.instruction_ip;
    }

    /// Activates a task whose queue entry was removed, taking its resume result exactly once.
    /// Any returned error must be raised in the loaded context before executing bytecode.
    fn activate_task(&mut self, task_id: TaskId) -> RunResult<()> {
        let result = self.scheduler.take_resume_result(task_id);
        if self.scheduler.current_task_id() != Some(task_id) {
            if let Some(current) = self.scheduler.current_task_id() {
                self.save_task_context(current);
            }
            self.scheduler.set_current_task(Some(task_id));
            self.load_or_init_task(task_id)?;
        }
        result
    }

    /// Restores a task's saved context and recursion depth, or starts its coroutine on first use.
    /// Resolved values are already on the saved stack. Pending exceptions are handled by `activate_task`.
    fn load_or_init_task(&mut self, task_id: TaskId) -> Result<(), RunError> {
        let task = self.scheduler.get_task_mut(task_id);
        let frames = mem::take(&mut task.frames);
        let stack = mem::take(&mut task.stack);
        let exception_stack = mem::take(&mut task.exception_stack);
        let instruction_ip = task.instruction_ip;
        let coroutine_id = task.coroutine_id;

        // Restore this task's recursion depth contribution to the global counter
        let task_depth = frames.len().saturating_sub(1); // root frame doesn't contribute to recursion depth
        self.recursion_depth += task_depth;

        if !frames.is_empty() {
            // Task has existing context - restore it
            self.stack = stack;
            self.exception_stack = exception_stack;
            self.instruction_ip = instruction_ip;

            // Reconstruct the suspended callers and current frame.
            let mut frames: Vec<_> = frames
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
                        bytecode: code.bytecode(),
                        ip: sf.ip,
                        stack_base: sf.stack_base,
                        locals_count: sf.locals_count,
                        exception_stack_base: sf.exception_stack_base,
                        function_id: sf.function_id,
                        call_offset: sf.call_offset,
                        should_return: false,
                        is_parked: false,
                        is_initializer: sf.is_initializer,
                    }
                })
                .collect();
            self.current_frame = frames.pop().expect("task context contains no active frame");
            self.suspended_frames = frames;
        } else if let Some(coro_id) = coroutine_id {
            // The previous task's frames are already saved. A reused coroutine
            // must fail this new task and select its successor before the
            // exception can propagate.
            let HeapReadOutput::Coroutine(coro) = self.heap.read(coro_id) else {
                panic!("task coroutine_id doesn't point to a Coroutine")
            };
            let is_new = coro.get(self.heap).state == CoroutineState::New;
            // Release the handle before either branch: both go on to drop
            // references to this coroutine, and freeing it under a live
            // reader panics.
            drop(coro);
            if is_new {
                self.init_task_from_coroutine(coro_id)?;
            } else {
                return self.handle_task_failure(ExcType::cannot_reuse_already_awaited_coroutine());
            }
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
        let HeapReadOutput::Coroutine(mut coro) = self.heap.read(coroutine_id) else {
            panic!("task coroutine_id doesn't point to a Coroutine")
        };

        // Check state
        if coro.get(self.heap).state != CoroutineState::New {
            return Err(ExcType::cannot_reuse_already_awaited_coroutine());
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
        // because that needs a current frame for call_offset, but spawned tasks
        // don't have a parent frame — the coroutine is the root)
        let func = self.interns.get_function(func_id);
        let locals_count = u16::try_from(namespace_values.len()).expect("coroutine namespace size exceeds u16");

        let stack_base = self.stack.len();
        self.stack.extend(namespace_values);

        let exc_stack_base = self.exception_stack.len();
        self.current_frame = CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            exc_stack_base,
            func_id,
            None, // No call position — this is the root frame for a spawned task
        );
        self.suspended_frames.clear();

        Ok(())
    }

    /// Resolves an external future with a value.
    ///
    /// Called by the host when an async external call completes. Looks up
    /// the `ExternalFuture` heap entry for `call_id`, transitions it to
    /// `Resolved(value)`, and delivers `value` to the awaiter (if any).
    pub fn resolve_future(&mut self, call_id: u32, value: Value) {
        let call_id = CallId::new(call_id);

        let Some(future_id) = self.scheduler.take_pending_external(call_id) else {
            value.drop_with(self);
            return;
        };

        // Ensure future cleaned up on all paths
        let fut_val = Value::Ref(future_id);
        let this = self;
        defer_drop!(fut_val, this);

        let mut value_guard = DropGuard::new(value, this);
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
            this.deliver_awaiter_success(awaiter, value);
        }
    }

    /// Pushes `value` onto a blocked task's stack and queues it, returning the awakened task.
    /// Drops the value if the task is gone or its awaitable has already settled.
    fn deliver_value_to_task(&mut self, task_id: TaskId, value: Value) -> Option<TaskId> {
        if !self.scheduler.is_blocked(task_id) {
            value.drop_with(self);
            return None;
        }

        let task_is_current = self.scheduler.current_task_id() == Some(task_id) && !self.current_frame.is_parked;
        if task_is_current {
            self.stack.push(value);
        } else {
            self.scheduler.get_task_mut(task_id).stack.push(value);
        }
        self.scheduler.make_ready(task_id, Ok(()), self.heap);
        Some(task_id)
    }

    /// Delivers a value through nested gathers, queueing the terminal task if the chain completes.
    /// Returns the awakened task, or `None` if a gather is pending, a gather already settled,
    /// or the terminal task no longer awaits a result.
    fn deliver_awaiter_success(&mut self, mut awaiter: Awaiter, mut value: Value) -> Option<TaskId> {
        let this = self;
        loop {
            match awaiter {
                Awaiter::Task(t) => return this.deliver_value_to_task(t, value),
                Awaiter::GatherSlot { gather, source } => {
                    let gather_val = Value::Ref(gather);
                    defer_drop!(gather_val, this);
                    let HeapReadOutput::GatherFuture(mut outer) = this.heap.read(gather) else {
                        panic!("Awaiter::GatherSlot gather id is not a GatherFuture")
                    };
                    let success = outer.resolve_child(this, source, value)?;
                    awaiter = success.awaiter;
                    value = Value::Ref(success.list_id);
                }
            }
        }
    }

    /// Fails each gather in the awaiter chain and queues its terminal task to raise `error`.
    /// Returns that task's ID, or `None` if the task is gone or a gather already settled.
    /// The task raises the exception on activation; its own gather is notified only if it escapes.
    fn deliver_awaiter_failure(&mut self, awaiter: Awaiter, error: RunError) -> Option<TaskId> {
        let this = self;
        defer_drop_mut!(awaiter, this);
        let target = loop {
            let next = match awaiter {
                Awaiter::Task(t) => break *t,
                Awaiter::GatherSlot { gather, .. } => {
                    let HeapReadOutput::GatherFuture(mut gather) = this.heap.read(*gather) else {
                        panic!("Awaiter::GatherSlot gather id is not a GatherFuture")
                    };
                    // A gather that already settled has handed its waiter on;
                    // this failure arrives after the fact and stops here.
                    gather.fail(this.heap, &error)?
                }
            };
            mem::replace(awaiter, next).drop_with(this);
        };
        if !this.scheduler.is_blocked(target) {
            return None;
        }
        this.scheduler.make_ready(target, Err(error), this.heap);
        Some(target)
    }

    /// Records a host rejection and queues the task that must raise it.
    /// Task selection waits until the whole batch has been ingested.
    pub fn fail_future(&mut self, call_id: u32, error: RunError) {
        let Some(future_id) = self.scheduler.take_pending_external(CallId::new(call_id)) else {
            return;
        };
        let fut_val = Value::Ref(future_id);
        let this = self;
        defer_drop!(fut_val, this);

        let HeapReadOutput::ExternalFuture(mut fut) = this.heap.read(future_id) else {
            panic!("pending_externals entry doesn't point to an ExternalFuture")
        };
        let awaiter = match mem::replace(
            &mut fut.get_mut(this.heap).state,
            ExternalFutureState::Failed(error.clone()),
        ) {
            ExternalFutureState::Pending { awaiter } => awaiter,
            ExternalFutureState::Resolved(_) | ExternalFutureState::Failed(_) => {
                panic!("fail_future: future was already resolved")
            }
        };
        drop(fut);
        if let Some(awaiter) = awaiter {
            this.deliver_awaiter_failure(awaiter, error);
        }
    }

    /// Allocates an `ExternalFuture` for `call_id` and pushes a `Value::Ref`
    /// to it on the VM stack.
    ///
    /// The scheduler indexes `call_id -> future_id` (with its own inc_ref) so
    /// host resolutions can find the heap entry; the `Value::Ref` pushed onto
    /// the stack is the user's reference, which travels with the value until
    /// it's awaited or dropped.
    pub fn add_pending_call(&mut self, call_id: CallId) {
        let future_id = self
            .heap
            .allocate(HeapData::ExternalFuture(Box::new(ExternalFuture::new_pending(call_id))));
        self.scheduler.add_pending_external(call_id, future_id, self.heap);
        self.push(Value::Ref(future_id));
    }

    /// Gets the pending call IDs from the scheduler.
    pub fn get_pending_call_ids(&self) -> Vec<CallId> {
        self.scheduler.pending_call_ids()
    }

    /// Raises `exc` uncatchably at the suspension point, for hosts enforcing
    /// a limit while execution is suspended.
    ///
    /// A `ResolveFutures` snapshot is parked when the last runnable task
    /// finished with the rest still blocked, so the VM holds a placeholder
    /// frame and every real frame lives in the scheduler. Raising there would
    /// point the traceback at line 1, so the main task is reloaded first: it
    /// is always alive while futures are pending, and its `await` is the
    /// suspension the user sees.
    pub fn abort(&mut self, exc: MontyException) -> RunResult<FrameExit> {
        let main = TaskId::default();
        if self.current_frame.is_parked
            && self.scheduler.has_task(main)
            && !self.scheduler.get_task_mut(main).frames.is_empty()
        {
            self.scheduler.set_current_task(Some(main));
            self.load_or_init_task(main)?;
        }
        self.resume_with_exception(RunError::uncatchable(exc))
    }

    /// Caches host results and wakes their awaiters without executing bytecode.
    /// Eager results have no awaiter yet; their following `await` reads the cached result.
    pub fn apply_future_results(&mut self, results: Vec<(u32, ExtFunctionResult)>) -> RunResult<()> {
        for (call_id, ext_result) in results {
            match ext_result {
                ExtFunctionResult::Return(obj) => {
                    // A resource error is a `MemoryError` here as in `resume`,
                    // so a large host value is not misreported as a bad type.
                    let value = obj.to_value(self).map_err(|e| match e {
                        InvalidInputError::Resource(err) => RunError::from(err),
                        other @ InvalidInputError::InvalidType(_) => RunError::from(MontyException::runtime_error(
                            format!("Invalid return value for call {call_id}: {other}"),
                        )),
                    })?;
                    self.resolve_future(call_id, value);
                }
                ExtFunctionResult::Error(exc) => self.fail_future(call_id, RunError::from(exc)),
                ExtFunctionResult::Future(_) => {}
                ExtFunctionResult::NotFound(function_name) => {
                    self.fail_future(call_id, ExtFunctionResult::not_found_exc(&function_name));
                }
            }
        }
        Ok(())
    }

    /// Ingests a host batch, then resumes tasks in resolution order for both values and errors.
    /// With no runnable task, yields the remaining pending call IDs to the host.
    /// Returns an internal error if neither runnable tasks nor pending calls remain.
    pub fn resume_with_resolved_futures(&mut self, results: Vec<(u32, ExtFunctionResult)>) -> RunResult<FrameExit> {
        self.apply_future_results(results)?;
        if let Some(next_task_id) = self.scheduler.next_ready_task() {
            if let Err(error) = self.activate_task(next_task_id) {
                return self.resume_with_exception(error);
            }
            return self.run_external();
        }

        let pending_call_ids = self.get_pending_call_ids();

        if pending_call_ids.is_empty() {
            // A stalled turn loses one `feed_run`, aborting loses the session.
            Err(RunError::internal(
                "asyncio scheduler stalled: no ready tasks and no pending external calls",
            ))
        } else {
            Ok(FrameExit::ResolveFutures(pending_call_ids))
        }
    }
}

/// One gather part-way through being committed — a frame of the walk in
/// [`VM::commit_gather_tree`], and the reason that walk needs no native stack.
///
/// Everything the recursive commit held in local variables lives here instead:
/// the items handled so far, and where in `items` to resume once the nested
/// gather this frame descended into has settled.
struct GatherCommit {
    /// The gather being committed. Borrowed — the reference is owned by the
    /// parent gather's `items`, or for the root by the awaited value itself.
    gather: HeapId,
    /// Owned awaiter, installed on the gather when it parks in `Awaited` and
    /// dropped when it settles synchronously. `Awaiter::GatherSlot` carries an
    /// inc_ref on the parent gather (see [`Awaiter`]).
    awaiter: Awaiter,
    /// Slot in the parent frame's `results` that this gather fills; `None` for
    /// the root, whose result goes to the `await` site instead.
    parent_slot: Option<usize>,
    /// Next index into the gather's `items` to commit.
    next: usize,
    /// Result slots, one per item, in `items` order. Owned values.
    results: Vec<Option<Value>>,
    /// Children committed but not yet settled → the slots they fill.
    pending_children: PendingChildren,
}

impl GatherCommit {
    /// Records the outcome of the nested gather this frame descended into and
    /// resumes at the following item.
    ///
    /// A nested gather that settled synchronously fills its slot like any other
    /// ready item; one that parked joins `pending_children`, so a later
    /// resolution reaches this gather through [`HeapRead::resolve_child`].
    fn resume_after_child(&mut self, child: HeapId, slot: usize, poll: Poll<Value>) {
        match poll {
            Poll::Ready(value) => self.results[slot] = Some(value),
            Poll::Pending => {
                self.pending_children.insert(child, smallvec![slot]);
            }
        }
        self.next = slot + 1;
    }
}

impl<C: ContainsHeap> DropWithContext<C> for GatherCommit {
    fn drop_with(self, heap: &mut C) {
        self.results.drop_with(heap);
        self.awaiter.drop_with(heap);
    }
}

/// Preflights the commit stack's next reallocation against `max_memory`.
///
/// A `Vec` grows by allocating the new buffer while the old one is still live,
/// so one growth part-way through a deep walk can add several MiB at once —
/// enough to clear the allocator's hard ceiling between two periodic checks,
/// which kills the worker rather than ending the run.
fn check_commit_stack_growth(len: usize, capacity: usize, tracker: &ResourceTracker) -> Result<(), ResourceError> {
    if len == capacity {
        tracker.check_allocation(2 * capacity * size_of::<GatherCommit>())
    } else {
        Ok(())
    }
}

/// Polls a gather that may already have settled, returning `None` when it is
/// still `Pending` and so needs committing.
///
/// Shared by the `await` site and the commit walk, which face the same four
/// states: a cached result to replay, a cached error to re-raise, an in-flight
/// gather someone else owns, or work to do.
fn poll_settled_gather<'h>(
    gather: &HeapRead<'h, GatherFuture>,
    heap: &HeapReader<'h>,
) -> Result<Option<Value>, RunError> {
    match &gather.get(heap).state {
        GatherState::Pending => Ok(None),
        GatherState::Completed(value) => Ok(Some(value.clone_with_heap(heap))),
        GatherState::Failed(err) => Err(err.clone()),
        // TODO: support concurrent re-await (CPython does).
        GatherState::Awaited(_) => Err(SimpleException::new_msg(
            ExcType::RuntimeError,
            "cannot reuse gather that is currently being awaited",
        )
        .into()),
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
    pub(crate) fn cache_result(&mut self, heap: &mut HeapReader<'h>, list_id: HeapId) {
        heap.inc_ref(list_id);
        self.get_mut(heap).state = GatherState::Completed(Value::Ref(list_id));
    }

    /// Records a child's value in its result slots, completing the gather when no children remain.
    /// Returns the completed result and awaiter, or `None` while other children are pending.
    /// A gather that already failed discards later values while its siblings continue running.
    fn resolve_child(&mut self, vm: &mut VM<'h>, child_id: HeapId, value: Value) -> Option<GatherSuccess> {
        // A sibling failed (or the commit pass rolled back) while this child
        // was still running: it has a result, and nowhere for it to go.
        // `Pending` is not reachable — a child only exists once awaited.
        match &self.get(vm.heap).state {
            GatherState::Awaited(_) => {}
            GatherState::Completed(_) | GatherState::Failed(_) => {
                value.drop_with(vm.heap);
                return None;
            }
            GatherState::Pending => panic!("resolve_child called on a gather that was never awaited"),
        }

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
            value.drop_with(vm.heap);
        }

        // Restore results and check completion.
        let awaited = self
            .get_mut(vm.heap)
            .as_awaited_mut()
            .expect("gather still Awaited after recording child resolution");
        awaited.results = results;

        if !awaited.pending_children.is_empty() {
            return None;
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
        let list_id = vm.heap.allocate(HeapData::List(List::new(results)));
        self.cache_result(vm.heap, list_id);
        Some(GatherSuccess { list_id, awaiter })
    }

    /// Caches the first failure and transfers the gather's waiter to the caller.
    /// Returns `None` if already settled. Children keep running and retain their references
    /// to this gather, so none can free it while the caller holds this heap reader.
    pub(crate) fn fail(&mut self, heap: &mut HeapReader<'h>, error: &RunError) -> Option<Awaiter> {
        // Take the Awaited bookkeeping. The state stays `Awaited` (with
        // placeholder fields) until the state replace below commits the
        // transition. The extracted `awaiter` is transferred to the caller
        // — it owns any `GatherSlot` inc_ref it carried. Already settled means
        // a chain that ends here: an earlier failure cached its error and
        // handed the waiter on.
        let (waiter, results) = {
            let awaited = self.get_mut(heap).as_awaited_mut()?;
            (
                mem::replace(&mut awaited.awaiter, Awaiter::Task(TaskId::default())),
                mem::take(&mut awaited.results),
            )
        };

        // Cache a clone so re-awaits replay the same exception.
        self.get_mut(heap).state = GatherState::Failed(error.clone());

        // Drop fanned-out result Values that won't reach the waiter. The
        // `pending_children` map goes with the `Awaited` payload; its keys are
        // borrowed ids, owned by `items`.
        results.drop_with(heap);

        Some(waiter)
    }
}
