//! Exception handling helpers for the VM.

use std::fmt::{self, Write};

use super::VM;
use crate::{
    builtins::Builtins,
    defer_drop,
    exception_private::{ExcType, ExceptionRaise, RawStackFrame, RunError, RunResult, SimpleException},
    expressions::CmpOperator,
    heap::{DropGuard, HeapData},
    intern::{StaticStrings, StringId},
    resource::ResourceTracker,
    types::{LazyHeapSet, PyTrait, Type},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, T> {
    /// Returns the current frame's name for traceback generation: the
    /// function name for user-defined functions, or `<module>` for
    /// module-level code. The empty-frames branch is defensive — async
    /// error paths now charge their tracker growth *before* draining
    /// `self.frames`, so any caller reaching this with an empty stack
    /// indicates a bug elsewhere; the `<module>` fallback keeps
    /// traceback generation total rather than panicking.
    fn current_frame_name(&self) -> StringId {
        match self.frames.last() {
            Some(frame) => match frame.function_id {
                Some(func_id) => self.interns.get_function(func_id).name.name_id,
                None => StaticStrings::Module.into(),
            },
            None => StaticStrings::Module.into(),
        }
    }

    /// Creates a `RawStackFrame` for the current execution point.
    ///
    /// Used when raising exceptions to capture traceback information.
    fn make_stack_frame(&self) -> RawStackFrame {
        RawStackFrame::new(
            self.current_position().unwrap_or_default(),
            self.current_frame_name(),
            None,
        )
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
                if exc.frame.is_none() {
                    let mut frame = self.make_stack_frame();
                    // Use the hide_caret flag from the error (set by error creators)
                    frame.hide_caret = exc.hide_caret;
                    exc.frame = Some(frame);
                }
                RunError::Exc(exc)
            }
            RunError::UncatchableExc(mut exc) => {
                if exc.frame.is_none() {
                    let mut frame = self.make_stack_frame();
                    frame.hide_caret = exc.hide_caret;
                    exc.frame = Some(frame);
                }
                RunError::UncatchableExc(exc)
            }
            RunError::Internal(_) => error,
        }
    }

    /// Creates a RunError from a Value that should be an exception.
    ///
    /// Takes ownership of the exception value and drops it properly.
    /// The `is_raise` flag indicates if this is from a `raise` statement (hide caret).
    pub(super) fn make_exception(&mut self, exc_value: Value, is_raise: bool) -> RunError {
        let this = self;
        defer_drop!(exc_value, this);

        let simple_exc = match exc_value {
            // Exception instance on heap
            Value::Ref(heap_id) => {
                if let HeapData::Exception(exc) = this.heap.get(*heap_id) {
                    // Clone the exception (guard handles cleanup at scope exit)
                    exc.clone()
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

        // Create frame with appropriate hide_caret setting
        let frame = if is_raise {
            RawStackFrame::from_raise(this.current_position().unwrap_or_default(), this.current_frame_name())
        } else {
            this.make_stack_frame()
        };

        RunError::Exc(ExceptionRaise {
            exc: simple_exc,
            frame: Some(frame),
            hide_caret: false,
        })
    }

    /// Runs fused bare `assert test`.
    ///
    /// Truthy values pass; falsy values raise with their repr, except literal
    /// `False` has no detail because `assert False` adds no information.
    pub(super) fn assert_test(&mut self) -> Result<(), RunError> {
        let this = self;
        let test = this.pop();
        defer_drop!(test, this);
        if test.py_bool(this) {
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
        let frame = RawStackFrame::from_raise(self.current_position().unwrap_or_default(), self.current_frame_name());
        RunError::Exc(ExceptionRaise {
            exc: SimpleException::new(ExcType::AssertionError, msg),
            frame: Some(frame),
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
    pub(super) fn handle_exception(&mut self, mut error: RunError) -> Option<RunError> {
        // Ensure exception has initial frame info
        error = self.attach_frame_to_error(error);

        // For uncatchable exceptions (ResourceError like RecursionError),
        // we still need to unwind the stack to collect all frames for the traceback
        if matches!(error, RunError::UncatchableExc(_) | RunError::Internal(_)) {
            return Some(self.unwind_for_traceback(error));
        }

        // Only catchable exceptions can be handled
        let exc_info = match &error {
            RunError::Exc(exc) => exc.clone(),
            RunError::UncatchableExc(_) | RunError::Internal(_) => unreachable!(),
        };

        // Create exception value to push on stack
        let exc_value = self.create_exception_value(&exc_info);
        let exc_value = match exc_value {
            Ok(v) => v,
            Err(e) => return Some(e),
        };

        // Use DropGuard because exc_value is conditionally consumed (pushed onto
        // exception_stack when handler found) or dropped (when no handler found)
        let mut exc_guard = DropGuard::new(exc_value, self);

        // Search for handler in current and outer frames
        loop {
            let (exc_value, this) = exc_guard.as_parts();
            let frame = this.current_frame();
            let ip = u32::try_from(this.instruction_ip).expect("instruction IP exceeds u32");

            // Search exception table for a handler covering this IP
            if let Some(entry) = frame.code.find_exception_handler(ip) {
                // Found a handler! Unwind stack and jump to it.
                // The operand stack lives directly above the locals region.
                // `entry.stack_depth()` is the compiler's operand-stack depth
                // at the try region, so the absolute stack index to unwind to
                // is `stack_base + locals_count + stack_depth`. Any in-flight
                // comprehension variables sit on the operand stack inside this
                // depth window and get cleaned up by the same drain.
                let handler_offset = usize::try_from(entry.handler()).expect("handler offset exceeds usize");
                let target_stack_depth = frame.stack_base + frame.locals_count as usize + entry.stack_depth() as usize;
                let target_exc_stack_depth = frame.exception_stack_base + entry.exception_stack_count() as usize;

                // Unwind stack to target depth (drop excess values)
                for value in this.stack.drain(target_stack_depth..).rev() {
                    value.drop_with(this.heap);
                }

                // Drop any `exception_stack` entries left behind by handlers
                // the propagating exception is bypassing — without this, a
                // handler whose body terminated via `raise`/`return`/`break`/
                // `continue` (so its trailer's `ClearException` is dead code)
                // would leak its exception onto `exception_stack`, where a
                // later bare `raise` could resurrect it.
                while this.exception_stack.len() > target_exc_stack_depth {
                    let value = this.exception_stack.pop().unwrap();
                    value.drop_with(this);
                }

                // Push exception value onto stack (handler expects it)
                let exc_for_stack = exc_value.clone_with_heap(this);
                this.push(exc_for_stack);

                // Reclaim exc_value from guard - it's being pushed onto exception_stack
                let (exc_value, this) = exc_guard.into_parts();

                // Push exception onto the exception_stack for bare raise.
                // This allows nested except handlers to restore outer
                // exception context.
                this.exception_stack.push(exc_value);

                // Jump to handler
                this.current_frame_mut().ip = handler_offset;

                return None; // Continue execution at handler
            }

            // No handler in this frame - pop frame and try outer
            if this.frames.len() <= 1 {
                // No more frames - exception is unhandled
                let is_spawned = this.is_spawned_task();

                // Drop exc_value before potentially switching tasks
                drop(exc_guard);

                // For spawned tasks, fail the task instead of propagating
                if is_spawned {
                    match self.handle_task_failure(error) {
                        Ok(()) => {
                            // Switched to next task - continue execution
                            return None;
                        }
                        Err(waiter_error) => {
                            // Switched to waiter - handle error in waiter's context
                            return self.handle_exception(waiter_error);
                        }
                    }
                }

                return Some(error);
            }

            // Get the caller's call-site offset before popping frame.
            // This is where the caller invoked the function that's failing.
            let call_offset = this.current_frame().call_offset;

            // Pop this frame
            if this.pop_frame() {
                // The frame indicated evaluation should stop - e.g. inside `evaluate_function` - return the error
                // now to stop unwinding.
                return Some(error);
            }

            // Add caller frame info to traceback (if we have a call site).
            // Resolve the offset now — against the caller, which is the current
            // frame after the pop above.
            if let Some(off) = call_offset {
                let pos = this.resolve_offset(off);
                let frame_name = this.current_frame_name();
                match &mut error {
                    RunError::Exc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::UncatchableExc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::Internal(_) => {}
                }
            }
        }
    }

    /// Unwinds the call stack to collect all frames for a traceback.
    ///
    /// Used for uncatchable exceptions (like RecursionError) that can't be handled
    /// but still need a complete traceback showing all active call frames.
    fn unwind_for_traceback(&mut self, mut error: RunError) -> RunError {
        // Pop frames and add caller frame info to the traceback
        while self.frames.len() > 1 {
            // Get the caller's call-site offset before popping frame
            let call_offset = self.current_frame().call_offset;

            // Pop this frame (cleans up namespace, etc.)
            self.pop_frame();

            // Add caller frame info to traceback. Resolve the offset against the
            // caller, which is the current frame after the pop above.
            if let Some(off) = call_offset {
                let pos = self.resolve_offset(off);
                let frame_name = self.current_frame_name();
                match &mut error {
                    RunError::Exc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::UncatchableExc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::Internal(_) => {}
                }
            }
        }
        error
    }

    /// Creates an exception Value from exception info.
    ///
    /// Allocates an Exception on the heap and returns a Value::Ref to it.
    fn create_exception_value(&mut self, exc: &ExceptionRaise) -> Result<Value, RunError> {
        let exception = exc.exc.clone();
        let heap_id = self.heap.allocate(HeapData::Exception(exception))?;
        Ok(Value::Ref(heap_id))
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
    /// allowed`. Removing the recursion both keeps parity with CPython and
    /// eliminates the unbounded-recursion footgun entirely, so no recursion-depth
    /// or time bound is needed here.
    ///
    /// Like CPython, the *whole* tuple is validated rather than short-circuiting
    /// on the first match: an invalid element raises the `TypeError` even when an
    /// earlier element already matched (e.g. `except (TypeError, (ValueError,))`
    /// raising `TypeError` still raises the `TypeError` about catching classes).
    pub(super) fn check_exc_match(&self, exception: &Value, exc_type: &Value) -> Result<bool, RunError> {
        let exc_type_enum = exception.py_type(self);
        match exc_type {
            // Single exception class.
            Value::Builtin(Builtins::ExcType(handler_type)) => {
                Ok(Self::exc_matches_handler(exc_type_enum, *handler_type))
            }
            // Flat tuple of exception classes. CPython does not descend into
            // nested tuples in this position, so neither do we.
            Value::Ref(id) => {
                if let HeapData::Tuple(tuple) = self.heap.get(*id) {
                    let mut matched = false;
                    for v in tuple.as_slice() {
                        match v {
                            Value::Builtin(Builtins::ExcType(handler_type)) => {
                                if !matched && Self::exc_matches_handler(exc_type_enum, *handler_type) {
                                    matched = true;
                                }
                            }
                            // A nested tuple or any non-exception value is
                            // rejected exactly as CPython rejects it, even if a
                            // previous element already matched.
                            _ => return Err(ExcType::except_invalid_type_error()),
                        }
                    }
                    Ok(matched)
                } else {
                    // A non-tuple heap value (e.g. an exception instance) is not
                    // a valid exception type for an `except` clause.
                    Err(ExcType::except_invalid_type_error())
                }
            }
            // Any other value is invalid for an `except` clause.
            _ => Err(ExcType::except_invalid_type_error()),
        }
    }

    /// Returns whether a raised exception's type is caught by `handler_type`.
    ///
    /// Helper shared by the single-class and flat-tuple arms of
    /// [`check_exc_match`]; the raised value only matches when its type is an
    /// exception that is a subclass of the handler's class.
    fn exc_matches_handler(exc_type_enum: Type, handler_type: ExcType) -> bool {
        matches!(exc_type_enum, Type::Exception(et) if et.is_subclass_of(handler_type))
    }
}

/// Streams an assert operand's repr into the configured byte-capped writer.
/// Reaching the cap stops formatting the remainder and appends `…`.
fn assert_operand_repr(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<String> {
    let mut writer = TruncatingWriter::new(vm.assert_repr_max_bytes as usize);
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
fn assert_msg_str(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<String> {
    let str_value = value.py_str(vm)?;
    defer_drop!(str_value, vm);
    Ok(str_value.to_str(vm)?.to_owned())
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
