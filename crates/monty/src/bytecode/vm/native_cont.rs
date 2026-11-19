//! Serializable native continuations: Rust consumers of a user `__next__` that survive a suspension.
//!
//! Instead of running a user callback in a nested `vm.run()` (`CallResult::require_synchronous`), a
//! consumer parks a [`NativeCont`] on the callback's frame and returns to the main loop. When that frame
//! returns or raises, the VM hands the outcome to the continuation in the caller frame. A suspension
//! inside the callback therefore leaves only the main loop on the Rust stack, and the continuation is
//! dumped with its frame.
//!
//! Variant and field names are the dump format. A dump does not capture [`NativeCont::resume`], so
//! changing what a stored state means needs a `DUMP_VERSION` bump or an upgrade.

use serde::{Deserialize, Serialize};

use super::{CallResult, VM};
use crate::{
    exception_private::{RunError, RunResult},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapReadOutput},
    types::{List, TupleVec, allocate_tuple},
    value::{VALUE_SIZE, Value},
};

/// Native work waiting on the result of one user `__next__` call.
///
/// Owns its values: a continuation lives on a [`CallFrame`](super::CallFrame), in a dump, or on the
/// Rust stack between steps, and is released with `drop_with` wherever a frame is discarded.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum NativeCont {
    /// A `for` loop over a user iterator, which stays on the caller's operand stack.
    /// Lands on the `ForIter` that started it: pushes the item, or pops the iterator and jumps.
    ForIter {
        /// The `ForIter` jump offset, relative to the instruction after it.
        offset: i16,
    },
    /// `list()` or `tuple()` draining a user iterator.
    Collect {
        iterator: Value,
        items: Vec<Value>,
        kind: CollectKind,
    },
    /// `sum()` folding a user iterator into `total`.
    Sum { iterator: Value, total: Value },
}

/// The collection a [`NativeCont::Collect`] builds once its iterator is exhausted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) enum CollectKind {
    List,
    Tuple,
}

/// What a continuation wants after consuming one outcome.
pub(super) enum Step {
    /// Finished with the consumer's result, which lands on the caller's operand stack.
    Value(Value),
    /// Finished, having already updated the caller frame itself (`for` loops).
    Landed,
    /// Needs the next item: call the iterator's `__next__` again.
    Next(NativeCont),
}

/// How [`VM::run_native`] left a continuation.
pub(super) enum NativeExit {
    /// The consumer finished with this result.
    Value(Value),
    /// The consumer finished and updated the caller frame itself.
    Landed,
    /// A `__next__` frame is now current and carries the continuation.
    Parked,
}

impl NativeCont {
    /// Resumes with the outcome of the `__next__` call this continuation waited on.
    /// `StopIteration` means the iterator is exhausted; any other error releases the state and propagates.
    pub(super) fn resume(self, result: RunResult<Value>, vm: &mut VM<'_>) -> RunResult<Step> {
        match result {
            Ok(item) => self.next_item(item, vm),
            Err(error) if error.is_stop_iteration() => Ok(self.exhausted(vm)),
            Err(error) => {
                self.drop_with(vm);
                Err(error)
            }
        }
    }

    /// Whether an error raised by the awaited `__next__` call ends iteration rather than propagating.
    pub(super) fn catches(error: &RunError) -> bool {
        error.is_stop_iteration()
    }

    /// Calls the iterator's `__next__` without completing it: a user method returns `FramePushed`.
    fn call_next(&self, vm: &mut VM<'_>) -> RunResult<CallResult> {
        let iterator = match self {
            Self::ForIter { .. } => vm.peek(),
            Self::Collect { iterator, .. } | Self::Sum { iterator, .. } => iterator,
        };
        let &Value::Ref(id) = iterator else {
            return Err(RunError::internal("native continuation iterator is not a heap value"));
        };
        match vm.heap.read(id) {
            HeapReadOutput::Instance(instance) => instance.call_next(vm),
            _ => Err(RunError::internal(
                "native continuation iterator is not a user instance",
            )),
        }
    }

    /// Consumes one item produced by `__next__`.
    fn next_item(self, item: Value, vm: &mut VM<'_>) -> RunResult<Step> {
        match self {
            Self::ForIter { .. } => {
                vm.push(item);
                Ok(Step::Landed)
            }
            Self::Collect {
                iterator,
                mut items,
                kind,
            } => {
                // Each item may come from host code, past any dispatch checkpoint.
                if let Err(error) = vm.heap.tracker.check_growth(items.len(), items.capacity(), VALUE_SIZE) {
                    item.drop_with(vm);
                    Self::Collect { iterator, items, kind }.drop_with(vm);
                    return Err(error.into());
                }
                items.push(item);
                Ok(Step::Next(Self::Collect { iterator, items, kind }))
            }
            Self::Sum { iterator, total } => {
                let added = total.py_add(&item, vm);
                item.drop_with(vm);
                total.drop_with(vm);
                match added {
                    Ok(total) => Ok(Step::Next(Self::Sum { iterator, total })),
                    Err(error) => {
                        iterator.drop_with(vm);
                        Err(error)
                    }
                }
            }
        }
    }

    /// Finishes after the iterator raised `StopIteration`.
    fn exhausted(self, vm: &mut VM<'_>) -> Step {
        match self {
            Self::ForIter { offset } => {
                vm.pop().drop_with(vm);
                let frame = vm.current_frame_mut();
                frame.ip = frame
                    .ip
                    .checked_add_signed(offset.into())
                    .expect("jump resulted in negative or overflowing IP");
                Step::Landed
            }
            Self::Collect { iterator, items, kind } => {
                iterator.drop_with(vm);
                Step::Value(match kind {
                    CollectKind::List => Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))),
                    CollectKind::Tuple => allocate_tuple(TupleVec::from_vec(items), vm.heap),
                })
            }
            Self::Sum { iterator, total } => {
                iterator.drop_with(vm);
                Step::Value(total)
            }
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for NativeCont {
    fn drop_with(self, heap: &mut C) {
        match self {
            Self::ForIter { .. } => {}
            Self::Collect { iterator, items, .. } => {
                iterator.drop_with(heap);
                items.drop_with(heap);
            }
            Self::Sum { iterator, total } => {
                iterator.drop_with(heap);
                total.drop_with(heap);
            }
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Box<NativeCont> {
    fn drop_with(self, heap: &mut C) {
        (*self).drop_with(heap);
    }
}

impl VM<'_> {
    /// Starts a builtin consumer of a user iterator, asking it for the first item.
    ///
    /// Returns `FramePushed` when a `__next__` frame now carries the continuation: the consumer's
    /// result then lands on the caller's stack as if the builtin had returned it.
    pub(crate) fn start_native(&mut self, cont: NativeCont) -> RunResult<CallResult> {
        Ok(match self.run_native(Ok(Step::Next(cont)))? {
            NativeExit::Value(value) => CallResult::Value(value),
            NativeExit::Parked => CallResult::FramePushed,
            NativeExit::Landed => return Err(RunError::internal("builtin native continuation landed on a frame")),
        })
    }

    /// Advances a `for` loop whose iterator, on top of the stack, is a user instance.
    pub(super) fn for_iter_native(&mut self, offset: i16) -> RunResult<()> {
        let exit = self.run_native(Ok(Step::Next(NativeCont::ForIter { offset })))?;
        self.land_native(exit);
        Ok(())
    }

    /// Hands a finished `__next__` frame's outcome to the continuation it carried.
    /// The frame is already popped; `call_offset` restores the caller's position for tracebacks.
    #[cold]
    #[inline(never)]
    pub(super) fn resume_native(
        &mut self,
        cont: NativeCont,
        call_offset: Option<u32>,
        result: RunResult<Value>,
    ) -> RunResult<()> {
        if let Some(offset) = call_offset {
            self.instruction_ip = offset as usize;
        }
        let step = cont.resume(result, self);
        let exit = self.run_native(step)?;
        self.land_native(exit);
        Ok(())
    }

    /// Completes a continuation parked on the current, just-pushed frame at a Rust boundary that
    /// cannot yield. Runs under the caller's re-entry reservation, like `finish_synchronously`.
    pub(super) fn finish_native_synchronously(&mut self, cont: NativeCont) -> RunResult<Value> {
        let first = CallResult::FramePushed.finish_synchronously(self);
        let mut step = cont.resume(first, self)?;
        loop {
            step = match step {
                Step::Value(value) => return Ok(value),
                // `for` loops attach only in the main loop, never under a synchronous boundary.
                Step::Landed => return Err(RunError::internal("native continuation landed synchronously")),
                Step::Next(cont) => {
                    let result = cont.call_next(self).and_then(|call| call.finish_synchronously(self));
                    cont.resume(result, self)?
                }
            };
        }
    }

    /// Steps a continuation until it finishes or parks on a `__next__` frame.
    ///
    /// Items produced without a frame (a builtin or host `__next__`) loop here rather than recurse.
    /// A user frame is never run on the Rust stack: the continuation is attached and the main loop
    /// resumes it from `ReturnValue` or exception unwinding.
    fn run_native(&mut self, step: RunResult<Step>) -> RunResult<NativeExit> {
        let mut step = step?;
        let mut steps = 0usize;
        loop {
            let cont = match step {
                Step::Value(value) => return Ok(NativeExit::Value(value)),
                Step::Landed => return Ok(NativeExit::Landed),
                Step::Next(cont) => cont,
            };
            // Only frameless items repeat this loop, and they reach no dispatch checkpoint.
            steps += 1;
            if let Err(error) = self.heap.tracker.check_time_every(steps) {
                cont.drop_with(self);
                return Err(error.into());
            }
            step = match cont.call_next(self) {
                // An initializer frame (`__next__` naming a class) lands its own result, so it runs here.
                Ok(CallResult::FramePushed) if !self.current_frame.is_initializer => {
                    debug_assert!(self.current_frame.continuation.is_none());
                    self.current_frame.continuation = Some(Box::new(cont));
                    return Ok(NativeExit::Parked);
                }
                result => {
                    let result = result.and_then(|call| call.require_synchronous(self));
                    cont.resume(result, self)?
                }
            };
        }
    }

    /// Pushes a finished consumer's result where the instruction that started it expects it.
    fn land_native(&mut self, exit: NativeExit) {
        if let NativeExit::Value(value) = exit {
            self.push(value);
        }
    }
}
