//! Serializable native continuations: Rust consumers of a user `__next__` that survive a suspension.
//!
//! Instead of running a user callback in a nested `vm.run()` (`CallResult::require_synchronous`), a
//! consumer parks a [`NativeCont`] on the callback's frame and returns to the main loop. When that frame
//! returns or raises, the VM hands the outcome to the continuation in the caller frame. A suspension
//! inside the callback therefore leaves only the main loop on the Rust stack, and the continuation is
//! dumped with its frame.
//!
//! Variant and field names are the dump format, added in dump version 18: a 1.7.0 reader would
//! ignore a frame's continuation. A dump does not capture [`NativeCont::resume`], so changing what
//! a stored state means needs compatibility handling or a `DUMP_VERSION` bump.

use serde::{Deserialize, Serialize};

use super::{CallResult, VM};
use crate::{
    builtins::SumPhase,
    exception_private::{RunError, RunResult},
    heap::{ContainsHeap, DropWithContext},
    types::{Instance, List, TupleVec, allocate_tuple},
    value::{VALUE_SIZE, Value},
};

/// Native work waiting on the result of one user `__next__` call.
///
/// Owns its values: a continuation lives on a [`CallFrame`](super::CallFrame), in a dump, or on the
/// Rust stack between steps, and is released with `drop_with` wherever a frame is discarded.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum NativeCont {
    /// `list()` or `tuple()` draining a user iterator.
    Collect {
        iterator: Value,
        items: Vec<Value>,
        kind: CollectKind,
    },
    /// `sum()` folding a user iterator into `total`.
    Sum {
        iterator: Value,
        /// Stale while `phase` holds a compensated float or complex total.
        total: Value,
        /// CPython's fast path, so floats from a suspending iterator sum exactly as from a list.
        phase: SumPhase,
    },
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
    /// Needs the next item: call the iterator's `__next__` again.
    Next(NativeCont),
}

/// How [`VM::run_native`] left a continuation.
pub(super) enum NativeExit {
    /// The consumer finished with this result.
    Value(Value),
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
        let (Self::Collect { iterator, .. } | Self::Sum { iterator, .. }) = self;
        match iterator.read_as::<Instance>(vm.heap) {
            Some(instance) => instance.call_next(vm),
            None => Err(RunError::internal(
                "native continuation iterator is not a user instance",
            )),
        }
    }

    /// Consumes one item produced by `__next__`.
    fn next_item(self, item: Value, vm: &mut VM<'_>) -> RunResult<Step> {
        match self {
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
            Self::Sum {
                iterator,
                mut total,
                mut phase,
            } => {
                let folded = phase.fold(&mut total, &item, vm);
                item.drop_with(vm);
                match folded {
                    Ok(()) => Ok(Step::Next(Self::Sum { iterator, total, phase })),
                    Err(error) => {
                        iterator.drop_with(vm);
                        total.drop_with(vm);
                        Err(error)
                    }
                }
            }
        }
    }

    /// Finishes after the iterator raised `StopIteration`.
    fn exhausted(self, vm: &mut VM<'_>) -> Step {
        match self {
            Self::Collect { iterator, items, kind } => {
                iterator.drop_with(vm);
                Step::Value(match kind {
                    CollectKind::List => Value::allocate(List::new(items), vm.heap),
                    CollectKind::Tuple => allocate_tuple(TupleVec::from_vec(items), vm.heap),
                })
            }
            Self::Sum {
                iterator,
                mut total,
                phase,
            } => {
                iterator.drop_with(vm);
                phase.store(&mut total, vm);
                Step::Value(total)
            }
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for NativeCont {
    fn drop_with(self, heap: &mut C) {
        match self {
            Self::Collect { iterator, items, .. } => {
                iterator.drop_with(heap);
                items.drop_with(heap);
            }
            Self::Sum { iterator, total, .. } => {
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
        })
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
    ///
    /// Polls the limits itself: each `__next__` is a nested run that may end before any dispatch
    /// checkpoint, so an endless iterator would otherwise never see the clock.
    pub(super) fn finish_native_synchronously(&mut self, cont: NativeCont) -> RunResult<Value> {
        let first = CallResult::FramePushed.finish_synchronously(self);
        let mut step = cont.resume(first, self)?;
        let mut steps = 0usize;
        loop {
            step = match step {
                Step::Value(value) => return Ok(value),
                Step::Next(cont) => {
                    steps += 1;
                    if let Err(error) = self.heap.tracker.check_memory_time_every(steps) {
                        cont.drop_with(self);
                        return Err(error.into());
                    }
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
                Step::Next(cont) => cont,
            };
            // Only frameless items repeat this loop, and they reach no dispatch checkpoint, though each
            // may allocate (collected items, a user `__add__`), so poll memory as well as time.
            steps += 1;
            if let Err(error) = self.heap.tracker.check_memory_time_every(steps) {
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
