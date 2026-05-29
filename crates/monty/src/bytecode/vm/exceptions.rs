//! Exception handling helpers for the VM.

use super::VM;
use crate::{
    builtins::Builtins,
    defer_drop,
    exception_private::{ExcType, ExceptionRaise, RawStackFrame, RunError, SimpleException},
    heap::{HeapData, HeapGuard},
    intern::{StaticStrings, StringId},
    resource::ResourceTracker,
    types::{PyTrait, Type},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, T> {
    /// Returns the current frame's name for traceback generation.
    ///
    /// Returns the function name for user-defined functions, or `<module>` for
    /// module-level code. The frame stack must be non-empty: callers in the
    /// async path that may run with no active frame (e.g. just before a spawned
    /// task's first frame is pushed) are expected to route errors through
    /// `handle_task_failure` rather than the regular exception machinery.
    fn current_frame_name(&self) -> StringId {
        let frame = self.current_frame();
        match frame.function_id {
            Some(func_id) => self.interns.get_function(func_id).name.name_id,
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

        // Use HeapGuard because exc_value is conditionally consumed (pushed onto
        // exception_stack when handler found) or dropped (when no handler found)
        let mut exc_guard = HeapGuard::new(exc_value, self);

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
                    value.drop_with_heap(this.heap);
                }

                // Drop any `exception_stack` entries left behind by handlers
                // the propagating exception is bypassing — without this, a
                // handler whose body terminated via `raise`/`return`/`break`/
                // `continue` (so its trailer's `ClearException` is dead code)
                // would leak its exception onto `exception_stack`, where a
                // later bare `raise` could resurrect it.
                while this.exception_stack.len() > target_exc_stack_depth {
                    let value = this.exception_stack.pop().unwrap();
                    value.drop_with_heap(this);
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

            // Get the call site position before popping frame
            // This is where the caller invoked the function that's failing
            let call_position = this.current_frame().call_position;

            // Pop this frame
            if this.pop_frame() {
                // The frame indicated evaluation should stop - e.g. inside `evaluate_function` - return the error
                // now to stop unwinding.
                return Some(error);
            }

            // Add caller frame info to traceback (if we have call position)
            if let Some(pos) = call_position {
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
            // Get the call site position before popping frame
            let call_position = self.current_frame().call_position;

            // Pop this frame (cleans up namespace, etc.)
            self.pop_frame();

            // Add caller frame info to traceback
            if let Some(pos) = call_position {
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
