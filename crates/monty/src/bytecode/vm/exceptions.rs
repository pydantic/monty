//! Exception handling helpers for the VM.

use std::fmt::{self, Write};

use super::VM;
use crate::{
    args::ArgValues,
    builtins::Builtins,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, ExceptionRaise, RawStackFrame, RunError, RunResult, SimpleException},
    expressions::CmpOperator,
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::{StaticStrings, StringId},
    types::{LazyHeapSet, PyTrait, class::ClassBase},
    value::Value,
};

/// An active exception, preserving its traceback across invisible implementation handlers.
/// Ordinary entries keep the existing serialized `Value` shape for upstream dump compatibility.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum CaughtException {
    /// An exception handled by user code, retaining its existing dump representation.
    Value(Value),
    /// An internal handler must not replace the original traceback when its match fails.
    Frozen { value: Value, error: Box<ExceptionRaise> },
}

impl CaughtException {
    /// Borrows the exception object without exposing its internal traceback metadata.
    fn value(&self) -> &Value {
        match self {
            Self::Value(value) | Self::Frozen { value, .. } => value,
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for CaughtException {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Value(value) | Self::Frozen { value, .. } => value.drop_with(ctx),
        }
    }
}

/// Whether exception handling finished or must continue in a newly activated task.
enum ExceptionHandlingResult {
    /// Execution can resume without propagating an error to the caller.
    Caught,
    /// The error should be returned to the VM caller.
    Unhandled(RunError),
    /// The newly activated task has an exception to handle before running bytecode.
    NextTask(RunError),
}

impl VM<'_> {
    /// Returns the current function name, or `<module>` outside a function.
    fn current_frame_name(&self) -> StringId {
        match self.current_frame().function_id {
            Some(func_id) => self.interns.get_function(func_id).name.name_id,
            None => self.interns.intern_static(StaticStrings::Module),
        }
    }

    /// Creates a `RawStackFrame` for the current execution point.
    ///
    /// Used when raising exceptions to capture traceback information.
    fn make_stack_frame(&self) -> RawStackFrame {
        RawStackFrame::new(self.current_position(), self.current_frame_name(), None)
    }

    /// Returns whether the active frame implements a builtin in frozen Python.
    pub(super) fn current_frame_is_frozen(&self) -> bool {
        self.current_frame()
            .function_id
            .is_some_and(|id| self.interns.is_internal_function(id))
    }

    /// Attaches initial frame information to an error if it doesn't have any.
    ///
    /// Only sets the innermost frame if the exception doesn't already have one.
    /// Caller frames are added separately during exception propagation.
    ///
    /// Uses the `hide_caret` flag from `ExceptionRaise` to determine whether to show
    /// the caret marker in the traceback. This flag is set by error creators that know
    /// whether CPython would show a caret for this specific error type.
    fn attach_frame_to_error(&self, error: RunError) -> RunError {
        match error {
            RunError::Exc(mut exc) => {
                if exc.frame.is_none() && !self.current_frame_is_frozen() {
                    let mut frame = self.make_stack_frame();
                    // Use the hide_caret flag from the error (set by error creators)
                    frame.hide_caret = exc.hide_caret;
                    exc.frame = Some(Box::new(frame));
                }
                RunError::Exc(exc)
            }
            RunError::UncatchableExc(mut exc) => {
                if exc.frame.is_none() && !self.current_frame_is_frozen() {
                    let mut frame = self.make_stack_frame();
                    frame.hide_caret = exc.hide_caret;
                    exc.frame = Some(Box::new(frame));
                }
                RunError::UncatchableExc(exc)
            }
            RunError::Internal(_) => error,
        }
    }

    /// Creates a RunError from a Value that should be an exception.
    ///
    /// Borrows the value so callers that already own one can keep it — the
    /// `raise`/`Reraise` paths reuse it as the raised object itself.
    /// The `is_raise` flag indicates if this is from a `raise` statement (hide caret).
    pub(super) fn make_exception(&mut self, exc_value: &Value, is_raise: bool) -> RunError {
        let simple_exc = match exc_value {
            // Exception instance on heap
            Value::Ref(heap_id) => {
                if let HeapData::Exception(exc) = self.heap.get(*heap_id) {
                    exc.clone()
                } else if let HeapData::Instance(inst) = self.heap.get(*heap_id)
                    && let HeapData::Class(cls) = self.heap.get(inst.class())
                    && let Some(base_exc) = cls.base_exception()
                {
                    let custom_name = cls.name().as_str(self.interns).to_owned();
                    SimpleException::new_none(base_exc).with_custom_type_name(Some(custom_name))
                } else {
                    // Not an exception type
                    SimpleException::new_msg(ExcType::TypeError, "exceptions must derive from BaseException")
                }
            }
            // Exception type (e.g., `raise ValueError` instead of `raise ValueError()`)
            // Instantiate with no message
            Value::Builtin(Builtins::ExcType(exc_type)) => SimpleException::new_none(*exc_type),
            // Invalid exception value
            _ => SimpleException::new_msg(ExcType::TypeError, "exceptions must derive from BaseException"),
        };

        let frame = (!self.current_frame_is_frozen()).then(|| {
            Box::new(if is_raise {
                RawStackFrame::from_raise(self.current_position(), self.current_frame_name())
            } else {
                self.make_stack_frame()
            })
        });

        RunError::Exc(ExceptionRaise {
            exc: simple_exc,
            frame,
            snippet_frame: None,
            hide_caret: false,
        })
    }

    /// Runs `raise exc from cause`.
    ///
    /// `exc` is checked first, as in CPython, then `cause` must be `None`, an
    /// exception or an exception class (instantiated without a message). The
    /// raised object records the cause and suppresses its context; a bare class
    /// is instantiated here so there is an object to record it on.
    pub(super) fn raise_with_cause(&mut self, exc: Value, cause: Value) -> Option<RunError> {
        let exc = match self.instantiate_if_user_class(exc) {
            Ok(v) => v,
            Err(error) => {
                cause.drop_with(self);
                return self.handle_exception_with_value(error, None);
            }
        };
        let error = self.make_exception(&exc, true);
        let raised = match self.exception_object(exc) {
            Ok(raised) => raised,
            Err(other) => {
                // `error` is the `exceptions must derive from BaseException` TypeError.
                other.drop_with(self);
                cause.drop_with(self);
                return self.handle_exception_with_value(error, None);
            }
        };
        let cause = match cause {
            Value::None => None,
            other => {
                let cause_obj = match self.instantiate_if_user_class(other) {
                    Ok(v) => v,
                    Err(error) => {
                        raised.drop_with(self);
                        return self.handle_exception_with_value(error, None);
                    }
                };
                match self.exception_object(cause_obj) {
                    Ok(cause) => cause.into_ref_id(),
                    Err(other) => {
                        other.drop_with(self);
                        raised.drop_with(self);
                        let error = ExcType::type_error("exception causes must derive from BaseException");
                        return self.handle_exception_with_value(error, None);
                    }
                }
            }
        };
        let Value::Ref(raised_id) = &raised else {
            unreachable!("exceptions are heap values")
        };
        if let Err(err) = self.set_exception_cause(*raised_id, cause) {
            raised.drop_with(self);
            return self.handle_exception_with_value(err, None);
        }
        self.handle_exception_with_value(error, Some(raised))
    }

    /// Returns the instantiated exception if `value` is a user-defined exception class,
    /// or `value` unchanged.
    pub(crate) fn instantiate_if_user_class(&mut self, value: Value) -> RunResult<Value> {
        if let Value::Ref(id) = &value
            && let HeapData::Class(cls) = self.heap.get(*id)
            && cls.base_exception().is_some()
        {
            let saved_ip = self.instruction_ip;
            let result = {
                let this = &mut *self;
                defer_drop!(value, this);
                this.evaluate_function(value, ArgValues::Empty)
            };
            self.instruction_ip = saved_ip;
            result
        } else {
            Ok(value)
        }
    }

    /// The exception object `value` denotes: itself for an exception, a fresh
    /// instance for an exception class. Anything else comes back as `Err` for the
    /// caller to release and report.
    fn exception_object(&mut self, value: Value) -> Result<Value, Value> {
        match value {
            Value::Ref(id) if matches!(self.heap.get(id), HeapData::Exception(_)) => Ok(value),
            Value::Ref(id)
                if matches!(self.heap.get(id), HeapData::Instance(inst)
                    if matches!(self.heap.get(inst.class()), HeapData::Class(cls) if cls.base_exception().is_some())) =>
            {
                Ok(value)
            }
            Value::Builtin(Builtins::ExcType(exc_type)) => Ok(self.allocate_exception(exc_type)),
            other => Err(other),
        }
    }

    /// Allocates `exc_type()` with no message, for `raise ExcType from ...` and its cause.
    fn allocate_exception(&mut self, exc_type: ExcType) -> Value {
        Value::Ref(
            self.heap
                .allocate(HeapData::Exception(SimpleException::new_none(exc_type))),
        )
    }

    /// Stores `cause` (an owned reference) as `__cause__` of the exception `id`,
    /// releasing any previous cause, and sets `__suppress_context__`.
    fn set_exception_cause(&mut self, id: HeapId, cause: Option<HeapId>) -> RunResult<()> {
        match self.heap.read(id) {
            HeapReadOutput::Exception(exc) => {
                let previous = exc.get_mut(self.heap).set_cause(cause);
                previous.map(Value::Ref).drop_with(self);
                Ok(())
            }
            HeapReadOutput::Instance(inst) => {
                let key = Value::InternString(self.interns.intern_static(StaticStrings::DunderCause));
                let value = match cause {
                    Some(cause_id) => Value::Ref(cause_id),
                    None => Value::None,
                };
                let previous = inst.set_attr_unchecked(key, value, self)?;
                if let Some(prev) = previous {
                    prev.drop_with(self);
                }
                let suppress_key =
                    Value::InternString(self.interns.intern_static(StaticStrings::DunderSuppressContext));
                let previous_suppress = inst.set_attr_unchecked(suppress_key, Value::Bool(true), self)?;
                if let Some(prev) = previous_suppress {
                    prev.drop_with(self);
                }
                Ok(())
            }
            _ => unreachable!("set_exception_cause is only called on exceptions"),
        }
    }

    /// Records the exception being handled as `__context__` of the newly raised
    /// `raised`, as CPython's `_PyErr_SetObject` does: a bare `raise` of the handled
    /// exception itself gets none, and a link that would make the chain circular is
    /// cut first. An internal (frozen) handler's exception is skipped, since the
    /// equivalent CPython code is C and leaves the user's handled exception in place.
    fn attach_context(&mut self, raised: &Value) -> RunResult<()> {
        let handled = self.exception_stack.iter().rev().find_map(|caught| match caught {
            CaughtException::Value(Value::Ref(id)) => Some(*id),
            _ => None,
        });
        let Some(handled_id) = handled else {
            return Ok(());
        };
        let Value::Ref(raised_id) = raised else {
            return Ok(());
        };
        if *raised_id == handled_id {
            return Ok(());
        }
        // Each re-raise inside a handler adds a link, so an unchecked scan of a
        // chain grown by a loop would be quadratic: poll the clock.
        let mut cursor = handled_id;
        for step in 0usize.. {
            let Some(next) = self.exception_context(cursor) else {
                break;
            };
            if next == *raised_id {
                self.set_exception_context(cursor, None)?;
                break;
            }
            self.heap.tracker.check_time_every(step)?;
            cursor = next;
        }
        self.heap.inc_ref(handled_id);
        if let Err(err) = self.set_exception_context(*raised_id, Some(handled_id)) {
            self.heap.dec_ref(handled_id);
            return Err(err);
        }
        Ok(())
    }

    /// The `__context__` of the exception `id`, borrowed.
    fn exception_context(&self, id: HeapId) -> Option<HeapId> {
        match self.heap.get(id) {
            HeapData::Exception(exc) => exc.context(),
            HeapData::Instance(inst) => match inst.attrs().get_by_str("__context__", self.heap, self.interns) {
                Some(Value::Ref(ctx_id)) => Some(*ctx_id),
                _ => None,
            },
            _ => None,
        }
    }

    /// Stores `context` (an owned reference) as `__context__` of the exception
    /// `id`, releasing any previous context.
    fn set_exception_context(&mut self, id: HeapId, context: Option<HeapId>) -> RunResult<()> {
        match self.heap.read(id) {
            HeapReadOutput::Exception(exc) => {
                let previous = exc.get_mut(self.heap).set_context(context);
                previous.map(Value::Ref).drop_with(self);
                Ok(())
            }
            HeapReadOutput::Instance(inst) => {
                let key = Value::InternString(self.interns.intern_static(StaticStrings::DunderContext));
                let value = match context {
                    Some(ctx_id) => Value::Ref(ctx_id),
                    None => Value::None,
                };
                let previous = inst.set_attr_unchecked(key, value, self)?;
                if let Some(prev) = previous {
                    prev.drop_with(self);
                }
                Ok(())
            }
            _ => unreachable!("set_exception_context is only called on exceptions"),
        }
    }

    /// Re-raises the active exception without replacing a user traceback with an internal handler.
    pub(super) fn reraise_exception(&mut self) -> (RunError, Option<Value>) {
        if let Some(caught) = self.exception_stack.last() {
            let raised = caught.value().clone_with_heap(self.heap);
            let error = if self.current_frame_is_frozen()
                && let CaughtException::Frozen { error, .. } = caught
            {
                RunError::Exc((**error).clone())
            } else {
                self.make_exception(&raised, true)
            };
            (error, Some(raised))
        } else {
            (
                SimpleException::new_msg(ExcType::RuntimeError, "No active exception to reraise").into(),
                None,
            )
        }
    }

    /// Runs fused bare `assert test`.
    ///
    /// Truthy values pass; falsy values raise with their repr, except literal
    /// `False` has no detail because `assert False` adds no information.
    pub(super) fn assert_test(&mut self) -> Result<(), RunError> {
        let this = self;
        let test = this.pop();
        defer_drop!(test, this);
        if test.py_bool(this)? {
            Ok(())
        } else if matches!(test, Value::Bool(false)) {
            Err(this.assertion_error(None))
        } else {
            let detail = assert_operand_repr(test, this).map(Some);
            Err(this.assert_failure(detail))
        }
    }

    /// Runs fused bare `assert lhs OP rhs`.
    ///
    /// Shares [`cmp_values`](VM::cmp_values) with the `Compare*` opcodes, so
    /// the comparison (and any `TypeError` it raises) behaves identically to
    /// the unfused form; only a `false` result diverges, raising with both
    /// operand reprs.
    pub(super) fn assert_cmp(&mut self, op: CmpOperator) -> Result<(), RunError> {
        let this = self;
        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);
        if this.cmp_values(op, lhs, rhs)? {
            Ok(())
        } else {
            let detail = assert_operand_repr(lhs, this).and_then(|lhs_repr| {
                let rhs_repr = assert_operand_repr(rhs, this)?;
                Ok(Some(format!("{lhs_repr} {op} {rhs_repr}")))
            });
            Err(this.assert_failure(detail))
        }
    }

    /// Converts best-effort detail into an `AssertionError` message.
    /// Catchable formatting errors fall back to no detail; terminal errors propagate.
    fn assert_failure(&self, detail: RunResult<Option<String>>) -> RunError {
        match detail {
            Ok(Some(detail)) => self.assertion_error(Some(format!("assert {detail}"))),
            Ok(None) | Err(RunError::Exc(_)) => self.assertion_error(None),
            Err(e) => e,
        }
    }

    /// Raises for failed `assert test, msg`.
    ///
    /// Uses `msg` first and appends introspected detail when available. If
    /// either formatting path raises a Python exception, the other still wins.
    pub(super) fn assert_failed_msg(&mut self, cmp_op: Option<CmpOperator>) -> RunError {
        let this = self;
        let msg_value = this.pop();
        defer_drop!(msg_value, this);
        // Format the operands first so they are popped and released even if
        // the message itself fails to stringify.
        let detail = match this.assert_detail(cmp_op) {
            Ok(detail) => detail,
            Err(RunError::Exc(_)) => None,
            Err(e) => return e,
        };
        let msg = match assert_msg_str(msg_value, this) {
            // An empty message adds nothing, so treat it like an absent one and
            // show only the detail — avoids a stray leading `\n` before `assert`.
            Ok(msg) if msg.is_empty() => None,
            Ok(msg) => Some(msg),
            Err(RunError::Exc(_)) => None,
            Err(e) => return e,
        };
        let full = match (msg, detail) {
            (Some(msg), Some(detail)) => Some(format!("{msg}\nassert {detail}")),
            (Some(msg), None) => Some(msg),
            (None, Some(detail)) => Some(format!("assert {detail}")),
            (None, None) => None,
        };
        this.assertion_error(full)
    }

    /// Pops failed assert operands and formats their detail.
    ///
    /// Comparisons produce `{lhs!r} {op} {rhs!r}`; other tests use the falsy
    /// value repr. Literal `False` returns no detail.
    fn assert_detail(&mut self, cmp_op: Option<CmpOperator>) -> RunResult<Option<String>> {
        let this = self;
        if let Some(op) = cmp_op {
            let rhs = this.pop();
            defer_drop!(rhs, this);
            let lhs = this.pop();
            defer_drop!(lhs, this);
            let lhs_repr = assert_operand_repr(lhs, this)?;
            let rhs_repr = assert_operand_repr(rhs, this)?;
            Ok(Some(format!("{lhs_repr} {op} {rhs_repr}")))
        } else {
            let test = this.pop();
            defer_drop!(test, this);
            if matches!(test, Value::Bool(false)) {
                Ok(None)
            } else {
                assert_operand_repr(test, this).map(Some)
            }
        }
    }

    /// Creates an `AssertionError` raised at the current source position.
    fn assertion_error(&self, msg: Option<String>) -> RunError {
        let frame = (!self.current_frame_is_frozen()).then(|| {
            Box::new(RawStackFrame::from_raise(
                self.current_position(),
                self.current_frame_name(),
            ))
        });
        RunError::Exc(ExceptionRaise {
            exc: SimpleException::new(ExcType::AssertionError, msg),
            frame,
            snippet_frame: None,
            hide_caret: false,
        })
    }

    /// Handles an exception by searching for a handler in the exception table.
    ///
    /// Returns:
    /// - `Some(VMResult)` if the exception was not caught (should return from run loop)
    /// - `None` if the exception was caught (continue execution)
    ///
    /// When an exception is caught:
    /// 1. Unwinds the stack to the handler's expected depth
    /// 2. Pushes the exception value onto the stack
    /// 3. Sets `current_exception` for bare `raise`
    /// 4. Jumps to the handler code
    pub(super) fn handle_exception(&mut self, error: RunError) -> Option<RunError> {
        self.handle_exception_with_value(error, None)
    }

    /// [`handle_exception`](Self::handle_exception) reusing an already-built
    /// exception instead of reallocating an identical one per level; this also
    /// preserves its identity, as CPython does. Owned: dropped if unused.
    pub(super) fn handle_exception_with_value(
        &mut self,
        mut error: RunError,
        mut raised: Option<Value>,
    ) -> Option<RunError> {
        loop {
            match self.handle_exception_step(error, raised.take()) {
                ExceptionHandlingResult::Caught => return None,
                ExceptionHandlingResult::Unhandled(error) => return Some(error),
                ExceptionHandlingResult::NextTask(next_error) => error = next_error,
            }
        }
    }

    /// Handles one task's exception, returning to the caller's loop if another task must raise.
    fn handle_exception_step(&mut self, mut error: RunError, raised: Option<Value>) -> ExceptionHandlingResult {
        // Ensure exception has initial frame info
        error = self.attach_frame_to_error(error);

        // For terminal resource errors such as memory limits,
        // we still need to unwind the stack to collect all frames for the traceback
        if matches!(error, RunError::UncatchableExc(_) | RunError::Internal(_)) {
            if let Some(raised) = raised {
                raised.drop_with(self);
            }
            if let Some(exc) = self.unwinding_exception.take() {
                exc.drop_with(self);
            }
            return ExceptionHandlingResult::Unhandled(self.unwind_for_traceback(error));
        }

        let exc_value = if let Some(raised) = raised {
            if let Some(old) = self.unwinding_exception.take() {
                old.drop_with(self);
            }
            raised
        } else if let Some(unwinding) = self.unwinding_exception.take() {
            if exception_value_matches_error(&unwinding, &error, self) {
                unwinding
            } else {
                unwinding.drop_with(self);
                let RunError::Exc(exc) = &error else {
                    unreachable!("terminal errors returned above")
                };
                self.create_exception_value(exc)
            }
        } else {
            // Nothing to reuse: build it from `error`, borrowed rather than
            // cloned since it is a local and so cannot alias `self`.
            let RunError::Exc(exc) = &error else {
                unreachable!("terminal errors returned above")
            };
            self.create_exception_value(exc)
        };
        if let Err(error) = self.attach_context(&exc_value) {
            exc_value.drop_with(self);
            return ExceptionHandlingResult::Unhandled(self.unwind_for_traceback(error));
        }

        // Use DropGuard because exc_value is conditionally consumed (pushed onto
        // exception_stack when handler found) or dropped (when no handler found)
        let mut exc_guard = DropGuard::new(exc_value, self);

        // Search for handler in current and outer frames
        loop {
            let (exc_value, this) = exc_guard.as_parts();
            let frame = this.current_frame();
            let code = frame.code;
            let ip = u32::try_from(this.instruction_ip).unwrap_or(u32::MAX);

            // Search exception table for a handler covering this IP
            if let Some(entry) = code.find_exception_handler(ip) {
                // Unwind operands to the compiler-recorded region depth,
                // including any in-flight comprehension values.
                let handler_offset = entry.handler() as usize;
                let target_stack_depth =
                    frame.stack_base() + frame.locals_count as usize + entry.stack_depth() as usize;
                let target_exc_stack_depth = frame.exception_stack_base() + entry.exception_stack_count() as usize;
                let pushes_exception = entry.pushes_exception();

                // Unwind stack to target depth (drop excess values)
                for value in this.stack.drain(target_stack_depth..).rev() {
                    value.drop_with(this.heap);
                }

                // Drop exceptions from bypassed handlers so a later bare
                // `raise` cannot revive them.
                while this.exception_stack.len() > target_exc_stack_depth {
                    let value = this.exception_stack.pop().unwrap();
                    value.drop_with(this);
                }

                // Push the exception only for handlers that read it; cleanup
                // handlers re-raise straight from `exception_stack`.
                if pushes_exception {
                    let exc_for_stack = exc_value.clone_with_heap(this);
                    this.push(exc_for_stack);
                }

                // Reclaim exc_value from guard - it's being pushed onto exception_stack
                let (exc_value, this) = exc_guard.into_parts();

                // Push exception onto the exception_stack for bare raise.
                // This allows nested except handlers to restore outer
                // exception context.
                let caught = if this.current_frame_is_frozen() {
                    let RunError::Exc(error) = error else {
                        unreachable!("only catchable exceptions enter handlers")
                    };
                    CaughtException::Frozen {
                        value: exc_value,
                        error: Box::new(error),
                    }
                } else {
                    CaughtException::Value(exc_value)
                };
                this.exception_stack.push(caught);

                // Jump to handler
                this.current_frame_mut().ip = handler_offset;

                return ExceptionHandlingResult::Caught;
            }

            // No handler in this frame - pop frame and try outer
            if this.suspended_frames.is_empty() {
                // No more frames - exception is unhandled
                let is_spawned = this.is_spawned_task();

                if let Err(terminal) = format_unhandled_custom_exception(&mut error, &mut exc_guard) {
                    let (exc_value, this) = exc_guard.into_parts();
                    exc_value.drop_with(this);
                    if let Some(exc) = this.unwinding_exception.take() {
                        exc.drop_with(this);
                    }
                    return ExceptionHandlingResult::Unhandled(this.unwind_for_traceback(terminal));
                }

                // Drop exc_value before potentially switching tasks
                drop(exc_guard);

                // For spawned tasks, fail the task instead of propagating
                if is_spawned {
                    return match self.handle_task_failure(error) {
                        Ok(()) => {
                            // Switched to next task - continue execution
                            ExceptionHandlingResult::Caught
                        }
                        Err(next_error) => ExceptionHandlingResult::NextTask(next_error),
                    };
                }

                return ExceptionHandlingResult::Unhandled(error);
            }

            // Get the caller's call-site offset before popping frame.
            // This is where the caller invoked the function that's failing.
            let call_offset = this.current_frame().call_offset;

            // A raising module body must not stay cached; checked before the pop.
            let is_initializer = this.current_frame().is_initializer;
            // Synchronous evaluation stops unwinding here, but its caller still
            // belongs in the traceback when the error crosses that boundary.
            let should_return = this.pop_frame();
            // CPython shows no carets under the `import` a module body's error unwinds through.
            let from_module_body = is_initializer && this.forget_failed_module();

            // Add caller frame info to traceback (if we have a call site).
            // Resolve the offset now — against the caller, which is the current
            // frame after the pop above.
            if let Some(off) = call_offset
                && !this.current_frame_is_frozen()
            {
                let pos = this.resolve_offset(off);
                let frame_name = this.current_frame_name();
                match &mut error {
                    RunError::Exc(exc) => exc.add_caller_frame(pos, frame_name, from_module_body),
                    RunError::UncatchableExc(exc) => exc.add_caller_frame(pos, frame_name, from_module_body),
                    RunError::Internal(_) => {}
                }
            }
            if should_return {
                if let Err(terminal) = format_unhandled_custom_exception(&mut error, &mut exc_guard) {
                    let (exc_value, this) = exc_guard.into_parts();
                    exc_value.drop_with(this);
                    if let Some(exc) = this.unwinding_exception.take() {
                        exc.drop_with(this);
                    }
                    return ExceptionHandlingResult::Unhandled(this.unwind_for_traceback(terminal));
                }
                let (exc_value, this) = exc_guard.into_parts();
                if let Some(old) = this.unwinding_exception.replace(exc_value) {
                    old.drop_with(this);
                }
                return ExceptionHandlingResult::Unhandled(error);
            }
        }
    }

    /// Collects terminal-error traceback frames up to the next synchronous boundary.
    /// Native callers must release their borrows before their Python frames are unwound.
    fn unwind_for_traceback(&mut self, mut error: RunError) -> RunError {
        // Pop frames and add caller frame info to the traceback
        while !self.suspended_frames.is_empty() {
            // Get the caller's call-site offset before popping frame
            let call_offset = self.current_frame().call_offset;

            let is_initializer = self.current_frame().is_initializer;
            let should_return = self.pop_frame();
            // CPython shows no carets under the `import` a module body's error unwinds through.
            let from_module_body = is_initializer && self.forget_failed_module();

            // Add caller frame info to traceback. Resolve the offset against the
            // caller, which is the current frame after the pop above.
            if let Some(off) = call_offset
                && !self.current_frame_is_frozen()
            {
                let pos = self.resolve_offset(off);
                let frame_name = self.current_frame_name();
                match &mut error {
                    RunError::Exc(exc) => exc.add_caller_frame(pos, frame_name, from_module_body),
                    RunError::UncatchableExc(exc) => exc.add_caller_frame(pos, frame_name, from_module_body),
                    RunError::Internal(_) => {}
                }
            }
            if should_return {
                break;
            }
        }
        error
    }

    /// Creates an exception Value from exception info.
    ///
    /// Allocates an Exception on the heap and returns a Value::Ref to it.
    fn create_exception_value(&mut self, exc: &ExceptionRaise) -> Value {
        let exception = exc.exc.clone();
        let heap_id = self.heap.allocate(HeapData::Exception(exception));
        Value::Ref(heap_id)
    }

    /// Checks if an exception matches an `except` clause's exception type.
    ///
    /// `exc_type` must be either a single exception class, or a *flat* tuple of
    /// exception classes. Returns `Ok(true)` if the exception matches, `Ok(false)`
    /// if it doesn't, or `Err` if `exc_type` is not a valid exception type.
    ///
    /// This deliberately does **not** recurse into nested tuples. The exception
    /// type handed to `except` is constructed at runtime, so a tuple could be
    /// nested arbitrarily deeply regardless of source nesting limits; a recursive
    /// matcher would overflow the host's native stack inside this single bytecode
    /// instruction. Mirroring CPython's `check_except_type_valid` (the
    /// `CHECK_EXC_MATCH` opcode), only one level of tuple is accepted: a nested
    /// tuple element — or any non-exception value — raises
    /// `TypeError: catching classes that do not inherit from BaseException is not
    /// allowed`.
    ///
    /// If `exc_type` is a tuple, each element is checked with amortized deadline
    /// checks. Mirroring CPython, the entire tuple is validated rather than
    /// short-circuiting on the first match (so non-exception types still raise
    /// `TypeError`), but once matched, expensive subclass ancestry scans are
    /// skipped in favor of O(1) handler validation using cached class exception
    /// ancestry. Repeated consecutive handlers are also cached to avoid redundant
    /// ancestry traversal.
    pub(super) fn check_exc_match(&self, exception: &Value, exc_type: &Value) -> Result<bool, RunError> {
        if let Value::Ref(id) = exc_type
            && let HeapData::Tuple(tuple) = self.heap.get(*id)
        {
            let mut matched = false;
            let mut last_handler: Option<(HandlerKey, bool)> = None;
            for (i, v) in tuple.as_slice().iter().enumerate() {
                self.heap.tracker.check_time_every(i)?;
                if matched {
                    self.validate_handler(v)?;
                } else if let Some(key) = parse_handler_key(v)
                    && let Some((prev_key, prev_match)) = last_handler
                    && prev_key == key
                {
                    if prev_match {
                        matched = true;
                    }
                } else {
                    let this_match = self.check_single_handler(exception, v)?;
                    if let Some(key) = parse_handler_key(v) {
                        last_handler = Some((key, this_match));
                    }
                    if this_match {
                        matched = true;
                    }
                }
            }
            return Ok(matched);
        }
        self.check_single_handler(exception, exc_type)
    }

    fn validate_handler(&self, handler: &Value) -> Result<(), RunError> {
        match handler {
            Value::Builtin(Builtins::ExcType(_)) => Ok(()),
            Value::Ref(class_id) => {
                if let HeapData::Class(handler_cls) = self.heap.get(*class_id)
                    && handler_cls.base_exception().is_some()
                {
                    Ok(())
                } else {
                    Err(ExcType::except_invalid_type_error())
                }
            }
            _ => Err(ExcType::except_invalid_type_error()),
        }
    }

    fn check_single_handler(&self, exception: &Value, handler: &Value) -> Result<bool, RunError> {
        self.validate_handler(handler)?;
        match handler {
            Value::Builtin(Builtins::ExcType(handler_type)) => {
                let matched = match exception {
                    Value::Ref(id) => match self.heap.get(*id) {
                        HeapData::Exception(e) => e.exc_type().is_subclass_of(*handler_type),
                        HeapData::Instance(inst) => {
                            if let HeapData::Class(cls) = self.heap.get(inst.class()) {
                                cls.mro().iter().any(
                                    |b| matches!(b, ClassBase::BuiltinExc(exc) if exc.is_subclass_of(*handler_type)),
                                )
                            } else {
                                false
                            }
                        }
                        _ => false,
                    },
                    _ => false,
                };
                Ok(matched)
            }
            Value::Ref(class_id) => {
                let matched = match exception {
                    Value::Ref(id) => match self.heap.get(*id) {
                        HeapData::Instance(inst) => {
                            let inst_cls_id = inst.class();
                            if inst_cls_id == *class_id {
                                true
                            } else if let HeapData::Class(inst_cls) = self.heap.get(inst_cls_id) {
                                inst_cls.is_subclass_of(*class_id)
                            } else {
                                false
                            }
                        }
                        _ => false,
                    },
                    _ => false,
                };
                Ok(matched)
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HandlerKey {
    Builtin(ExcType),
    Class(HeapId),
}

fn parse_handler_key(handler: &Value) -> Option<HandlerKey> {
    match handler {
        Value::Builtin(Builtins::ExcType(t)) => Some(HandlerKey::Builtin(*t)),
        Value::Ref(id) => Some(HandlerKey::Class(*id)),
        _ => None,
    }
}

/// Streams an assert operand's repr into the configured byte-capped writer.
/// Reaching the cap stops formatting the remainder and appends `…`.
fn assert_operand_repr(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let mut writer = TruncatingWriter::new(vm.env.assert_repr_max_bytes as usize);
    let mut heap_ids = LazyHeapSet::default();
    match value.py_repr_fmt(&mut writer, vm, &mut heap_ids) {
        Ok(()) => Ok(writer.into_string()),
        // The cap abort is ours: the partial repr is the result. Genuine
        // errors can only surface before the cap (post-cap writes do no VM work).
        Err(_) if writer.truncated => Ok(writer.into_string()),
        Err(e) => Err(e),
    }
}

/// `str()` of an explicit assert message, matching how the message renders in
/// `AssertionError: {msg}` — not truncated, since the user chose it explicitly.
fn assert_msg_str(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let str_value = value.py_str(vm)?;
    defer_drop!(str_value, vm);
    Ok(str_value.as_str(vm).expect("py_str returns str").to_owned())
}

/// Byte-capped sink that stops repr formatting on a character boundary.
/// Its buffer is untracked because `py_repr_fmt` also needs mutable VM access.
struct TruncatingWriter {
    buf: String,
    /// Bytes still accepted before the cap.
    remaining: usize,
    /// Set when input was cut at the cap; `into_string` then appends `…`.
    truncated: bool,
}

impl TruncatingWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            buf: String::new(),
            remaining: max_bytes,
            truncated: false,
        }
    }

    /// Consumes the writer, appending `…` when input was cut at the cap.
    fn into_string(mut self) -> String {
        if self.truncated {
            self.buf.push('…');
        }
        self.buf
    }
}

impl Write for TruncatingWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.truncated {
            Err(fmt::Error)
        } else if let Some(remaining) = self.remaining.checked_sub(s.len()) {
            self.remaining = remaining;
            self.buf.push_str(s);
            Ok(())
        } else {
            // Over budget: cut at the last char boundary in budget (≤3 steps back).
            let mut idx = self.remaining;
            while !s.is_char_boundary(idx) {
                idx -= 1;
            }
            self.buf.push_str(&s[..idx]);
            self.remaining = 0;
            self.truncated = true;
            Err(fmt::Error)
        }
    }
}

fn format_unhandled_custom_exception(
    error: &mut RunError,
    exc_guard: &mut DropGuard<'_, VM<'_>, Value>,
) -> RunResult<()> {
    if let RunError::Exc(raise) = error
        && raise.exc.arg().is_none()
        && raise.exc.custom_type_name().is_some()
    {
        let (exc_val, this) = exc_guard.as_parts();
        match exc_val.py_str(this) {
            Ok(text) => {
                defer_drop!(text, this);
                if let Some(s) = text.as_str(this)
                    && !s.is_empty()
                {
                    raise.exc.set_arg(Some(s.to_owned()));
                }
            }
            Err(RunError::Exc(_)) => {
                this.clear_unwinding_exception();
            }
            Err(terminal) => {
                this.clear_unwinding_exception();
                return Err(terminal);
            }
        }
    }
    Ok(())
}

fn exception_value_matches_error(exc_val: &Value, error: &RunError, vm: &VM<'_>) -> bool {
    let RunError::Exc(raise) = error else {
        return false;
    };
    match exc_val {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Exception(e) => {
                e.exc_type() == raise.exc.exc_type()
                    && e.custom_type_name() == raise.exc.custom_type_name()
                    && e.arg() == raise.exc.arg()
            }
            HeapData::Instance(inst) => {
                if let HeapData::Class(cls) = vm.heap.get(inst.class()) {
                    cls.mro()
                        .iter()
                        .any(|b| matches!(b, ClassBase::BuiltinExc(exc) if *exc == raise.exc.exc_type()))
                        && raise.exc.custom_type_name() == Some(cls.name().as_str(vm.interns))
                } else {
                    false
                }
            }
            _ => false,
        },
        _ => false,
    }
}
