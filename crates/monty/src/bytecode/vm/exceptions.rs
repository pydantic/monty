//! Exception handling helpers for the VM.

use super::VM;
use crate::{
    builtins::Builtins,
    exception_private::{ExcType, ExceptionRaise, RawStackFrame, RunError, SimpleException},
    heap::HeapData,
    intern::{StaticStrings, StringId},
    io::PrintWriter,
    resource::ResourceTracker,
    types::{PyTrait, Type},
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Returns the current frame's name for traceback generation.
    ///
    /// Returns the function name for user-defined functions, or `<module>` for
    /// module-level code.
    fn current_frame_name(&self) -> StringId {
        let frame = self.current_frame();
        match frame.function_id {
            Some(func_id) => self.interns.get_function(func_id).name.name_id,
            None => StaticStrings::Module.as_string_id(),
        }
    }

    /// Creates a `RawStackFrame` for the current execution point.
    ///
    /// Used when raising exceptions to capture traceback information.
    fn make_stack_frame(&self) -> RawStackFrame {
        RawStackFrame::new(self.current_position(), self.current_frame_name(), None)
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
        let simple_exc = match &exc_value {
            // Exception instance on heap
            Value::Ref(heap_id) => {
                if let HeapData::Exception(exc) = self.heap.get(*heap_id) {
                    // Clone the exception
                    let exc_clone = exc.clone();
                    // Drop the value with proper heap cleanup
                    exc_value.drop_with_heap(self.heap);
                    exc_clone
                } else {
                    // Not an exception type
                    exc_value.drop_with_heap(self.heap);
                    SimpleException::new_msg(ExcType::TypeError, "exceptions must derive from BaseException")
                }
            }
            // Exception type (e.g., `raise ValueError` instead of `raise ValueError()`)
            // Instantiate with no message
            Value::Builtin(Builtins::ExcType(exc_type)) => SimpleException::new_none(*exc_type),
            // Invalid exception value
            _ => {
                exc_value.drop_with_heap(self.heap);
                SimpleException::new_msg(ExcType::TypeError, "exceptions must derive from BaseException")
            }
        };

        // Create frame with appropriate hide_caret setting
        let frame = if is_raise {
            RawStackFrame::from_raise(self.current_position(), self.current_frame_name())
        } else {
            self.make_stack_frame()
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

        // Search for handler in current and outer frames
        loop {
            let frame = self.current_frame();
            let ip = u32::try_from(self.instruction_ip).expect("instruction IP exceeds u32");

            // Search exception table for a handler covering this IP
            if let Some(entry) = frame.code.find_exception_handler(ip) {
                // Found a handler! Unwind stack and jump to it.
                let handler_offset = usize::try_from(entry.handler()).expect("handler offset exceeds usize");
                let target_stack_depth = frame.stack_base + entry.stack_depth() as usize;

                // Unwind stack to target depth (drop excess values)
                while self.stack.len() > target_stack_depth {
                    let value = self.stack.pop().unwrap();
                    value.drop_with_heap(self.heap);
                }

                // Push exception value onto stack (handler expects it)
                let exc_for_stack = exc_value.clone_with_heap(self.heap);
                self.push(exc_for_stack);

                // Push exception onto the exception_stack for bare raise
                // This allows nested except handlers to restore outer exception context
                self.exception_stack.push(exc_value);

                // Jump to handler
                self.current_frame_mut().ip = handler_offset;

                return None; // Continue execution at handler
            }

            // No handler in this frame - pop frame and try outer
            if self.frames.len() <= 1 {
                // No more frames - exception is unhandled
                exc_value.drop_with_heap(self.heap);
                return Some(error);
            }

            // Get the call site position before popping frame
            // This is where the caller invoked the function that's failing
            let call_position = self.current_frame().call_position;

            // Pop this frame
            self.pop_frame();

            // Add caller frame info to traceback (if we have call position)
            if let Some(pos) = call_position {
                let frame_name = self.current_frame_name();
                match &mut error {
                    RunError::Exc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::UncatchableExc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::Internal(_) => {}
                }
            }

            // Update instruction_ip for the new frame
            self.instruction_ip = self
                .current_frame()
                .call_position
                .map_or(0, |p| p.start().line as usize);
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

    /// Checks if an exception matches an exception type for except clause matching.
    ///
    /// Validates that `exc_type` is a valid exception type (ExcType or tuple of ExcTypes).
    /// Returns `Ok(true)` if exception matches, `Ok(false)` if not, or `Err` if exc_type is invalid.
    pub(super) fn check_exc_match(&self, exception: &Value, exc_type: &Value) -> Result<bool, RunError> {
        let exc_type_enum = exception.py_type(self.heap);
        self.check_exc_match_inner(exc_type_enum, exc_type)
    }

    /// Inner recursive helper for check_exc_match that handles tuples.
    fn check_exc_match_inner(&self, exc_type_enum: Type, exc_type: &Value) -> Result<bool, RunError> {
        match exc_type {
            // Valid exception type
            Value::Builtin(Builtins::ExcType(handler_type)) => {
                // Check if exception is an instance of handler_type
                Ok(matches!(exc_type_enum, Type::Exception(et) if et.is_subclass_of(*handler_type)))
            }
            // Tuple of exception types
            Value::Ref(id) => {
                if let HeapData::Tuple(tuple) = self.heap.get(*id) {
                    for v in tuple.as_vec() {
                        if self.check_exc_match_inner(exc_type_enum, v)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                } else {
                    // Not a tuple - invalid exception type
                    Err(ExcType::except_invalid_type_error())
                }
            }
            // Any other type is invalid for except clause
            _ => Err(ExcType::except_invalid_type_error()),
        }
    }
}
