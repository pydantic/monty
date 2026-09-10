//! Async execution support for the VM.
//!
//! This module contains all async-related methods for the VM including:
//! - Awaiting coroutines, external futures, and gather futures
//! - Task scheduling and context switching
//! - Task completion and failure handling
//! - External future resolution

use std::{mem, task::Poll};

use monty_types::{MontyException, ResourceError, ResourceTracker};
use smallvec::{SmallVec, smallvec};

use super::{AwaitResult, CallFrame, FrameExit, VM};
use crate::{
    asyncio::{
        AwaitedGather, Awaiter, CallId, Coroutine, CoroutineState, ExternalFuture, ExternalFutureState, GatherFuture,
        GatherState, PendingChildren, TaskId,
    },
    bytecode::vm::scheduler::{SerializedTaskFrame, TaskState},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{
        ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapObjectRead, HeapRead, HeapReadOutput,
        HeapReader,
    },
    intern::FunctionId,
    object_bridge::MontyObjectExt,
    run_progress::{ExtFunctionResult, ExtFunctionResultExt},
    types::List,
    value::Value,
};

impl<'h> VM<'h> {
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
            self.save_current_context();
            self.scheduler.set_current_task(Some(next_task_id));

            // Load or initialize the next task's context
            self.load_or_init_task(next_task_id)?;

            // Continue execution with the newly current frame
            Ok(AwaitResult::FramePushed)
        } else {
            // No ready tasks - yield control to host.
            // Don't save the main task's context - frames stay in VM for the snapshot.
            Ok(AwaitResult::Yield(self.scheduler.pending_call_ids()))
        }
    }

    /// Saves the current task's context before switching tasks.
    fn save_current_context(&mut self) {
        if let Some(current_task_id) = self.scheduler.current_task_id() {
            self.save_task_context(current_task_id);
        }
    }

    /// Handles completion of a spawned task.
    ///
    /// Called when a spawned task's coroutine returns. This:
    /// 1. Marks the task as completed in the scheduler
    /// 2. Hands the result to whatever awaits the task, if anything still does
    /// 3. If that completes a gather, unblocks its waiter with the result list
    /// 4. Otherwise, switches to the next ready task
    pub(super) fn handle_task_completion(&mut self, result: Value) -> Result<AwaitResult, RunError> {
        let task_id = self
            .scheduler
            .current_task_id()
            .expect("handle_task_completion called without current task");
        // Take the awaiter before cancelling the task: it owns the inc_ref on
        // the gather it points at, so holding it here keeps that gather alive
        // across the teardown below.
        let task = self.scheduler.get_task_mut(task_id);
        let awaiter = task.awaiter.take();
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

        // Cancel the task now to release its inc_ref on the coroutine;
        // otherwise it would linger in the scheduler. Its awaiter is already
        // out, so this releases nothing the delivery below needs.
        self.scheduler.cancel_task(task_id, self.heap);

        // Hand the result down the chain. `None` means the gather that spawned
        // this task settled first, so it ran on only for its side effects.
        let delivery = if let Some(awaiter) = awaiter {
            self.deliver_awaiter_success(awaiter, result)
        } else {
            result.drop_with(self);
            None
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
    /// Called when an exception escapes all frames in a spawned task. The
    /// task's awaiter chain is walked — settling each gather on the way — to
    /// the task that should raise it.
    ///
    /// A task nothing awaits has no such chain, nor has one whose chain ends
    /// at an already-settled gather. Nothing can receive the exception, so it
    /// is dropped — as CPython does, whose `gather` retrieves a late child's
    /// exception through a done-callback and prints nothing either.
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

        // Take the task's awaiter — it owns the inc_ref on whatever it points
        // at, so the chain walk below cannot free its first link underneath
        // itself.
        let awaiter = self.scheduler.get_task_mut(task_id).awaiter.take();

        // Walk the awaiter chain, settling each gather on the way, to reach
        // the task that should resume with the exception. Delivering nothing
        // means the chain ended nowhere: no awaiter, a gather on the way that
        // had already settled, or a waiter that is gone.
        if let Some(awaiter) = awaiter
            && let Some(waiter_id) = self.deliver_awaiter_failure(awaiter, error.clone())
        {
            // `deliver_awaiter_failure` set the waiter to `Failed`, but we
            // propagate the exception via `Err` (the run loop's
            // `handle_exception` raises in the waiter's frame), so the task
            // should be running. Override to `Ready` before switching in.
            self.scheduler.set_state(waiter_id, TaskState::Ready, self.heap);
            self.discard_failed_task(task_id);
            self.scheduler.set_current_task(Some(waiter_id));
            self.load_or_init_task(waiter_id)?;
            return Err(error);
        }

        // Nothing can receive this exception, so it is dropped: CPython's
        // `gather` retrieves each child's exception through a done-callback
        // even after it has settled, so it prints nothing here either. Then
        // drop the task and switch to the next ready one; if there is none,
        // frames are left empty and the run loop yields.
        drop(error);
        self.discard_failed_task(task_id);
        self.scheduler.set_current_task(None);
        if let Some(next_task_id) = self.scheduler.next_ready_task() {
            self.scheduler.set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
        }

        Ok(())
    }

    /// Drops the current task after an exception escaped its last frame,
    /// discarding the VM context it was running in.
    ///
    /// The task has no way back — its root frame is gone — so it must leave
    /// the scheduler rather than linger with a half-torn-down context. Its
    /// awaiter has already been taken by this point, so this only releases
    /// what the task itself owns.
    fn discard_failed_task(&mut self, task_id: TaskId) {
        self.cleanup_current_task();
        self.scheduler.cancel_task(task_id, self.heap);
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
            // New task: pre-check the coroutine state here rather than letting
            // `init_task_from_coroutine` raise. By this point the calling task's
            // frames have already been saved, so route already-awaited failures
            // through `handle_task_failure`, which restores the waiter before
            // the error propagates.
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

    /// Pushes `value` onto `task_id`'s stack and marks it ready. If the task
    /// has already been cancelled (no longer in the scheduler) or failed,
    /// drops `value` instead — the resolution still gets cached on the future,
    /// but the (now-gone) awaiter doesn't receive it.
    ///
    fn deliver_value_to_task(&mut self, task_id: TaskId, value: Value) {
        if !self.scheduler.has_task(task_id) || self.scheduler.is_task_failed(task_id) {
            value.drop_with(self);
            return;
        }

        let task_is_current = self.scheduler.current_task_id() == Some(task_id) && !self.current_frame.is_parked;
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
    fn deliver_awaiter_success(&mut self, mut awaiter: Awaiter, mut value: Value) -> Option<TaskId> {
        let this = self;
        loop {
            match awaiter {
                Awaiter::Task(t) => {
                    this.deliver_value_to_task(t, value);
                    return Some(t);
                }
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
    /// - `None` if the chain ends nowhere — the terminal task is gone, or a
    ///   gather on the way had already settled, in which case the error has no
    ///   reader and dies here.
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
        if !this.scheduler.has_task(target) {
            return None;
        }
        this.scheduler.fail_task(target, error, this.heap);
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
            // The task being switched away from is parked on a call of its
            // own, and a gather failing no longer cancels it, so its context
            // has to be saved rather than dropped — it still has to resume
            // when its own call comes back.
            self.park_current_context();
            self.scheduler.set_current_task(Some(waiter_id));
            self.load_or_init_task(waiter_id)?;
        }
        Ok(())
    }

    /// Puts the current task's VM context away before switching to another
    /// task: saved if the task will run again, discarded if it is gone.
    fn park_current_context(&mut self) {
        match self.scheduler.current_task_id() {
            Some(task_id) if self.scheduler.has_task(task_id) => self.save_task_context(task_id),
            _ => self.cleanup_current_task(),
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

    /// Resolves external futures and resumes execution.
    ///
    /// This is the standard sequence for resuming after a `FrameExit::ResolveFutures`:
    /// 1. Resolve or fail each future from the provided results
    /// 2. Attempt to resume the current task (or fail it if any future resolution caused it to fail)
    /// 3. Load a ready task if needed (current task still blocked)
    /// 4. If no task is ready, return `ResolveFutures` with remaining pending call IDs
    ///
    /// # Errors
    /// Returns [`RunError::Internal`] if nothing is ready to run and nothing is
    /// pending: unreachable by design, but ends the turn rather than the worker.
    pub fn resume_with_resolved_futures(&mut self, results: Vec<(u32, ExtFunctionResult)>) -> RunResult<FrameExit> {
        for (call_id, ext_result) in results {
            match ext_result {
                ExtFunctionResult::Return(obj) => {
                    let value = obj.to_value(self).map_err(|e| {
                        RunError::from(MontyException::runtime_error(format!(
                            "Invalid return value for call {call_id}: {e}"
                        )))
                    })?;
                    self.resolve_future(call_id, value);
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
                    return self.run_external();
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
            self.save_current_context();
            self.scheduler.set_current_task(Some(next_task_id));
            self.load_or_init_task(next_task_id)?;
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
    /// `VM::handle_task_failure` for in-frame exceptions). Both settle the
    /// gather before any other sibling has a chance to resolve, and the
    /// siblings then keep running: their results arrive here afterwards and
    /// are dropped.
    ///
    /// Returns `None` while children are still in flight, or if the gather has
    /// already settled; otherwise `Some(GatherSuccess)` with the cached result
    /// list.
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

    /// Settles the gather on `error` and returns its waiter, or `None` if it
    /// had already settled.
    ///
    /// Touches nothing but this gather. The children still in flight keep
    /// running and keep pointing here, as CPython leaves them on the loop;
    /// what each produces is discarded when it arrives (see
    /// [`Self::resolve_child`] and [`VM::deliver_awaiter_success`]). Releasing
    /// nothing is what makes this safe to call with a live `HeapRead` on the
    /// gather — severing the children here would run their `dec_ref`s under
    /// that reader, and the last one can free the entry.
    ///
    /// Takes `&mut HeapReader` rather than `&mut VM` so this works from both
    /// `VM::deliver_awaiter_failure` (has a VM, splits borrows on its fields)
    /// and `Scheduler::fail_for_call` (only has a heap reader).
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
