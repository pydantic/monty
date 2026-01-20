//! Custom exception types for the Monty TypeScript/JavaScript bindings.
//!
//! Provides exception classes that wrap Monty's internal exceptions,
//! preserving traceback information and allowing JavaScript code to distinguish
//! between syntax errors, runtime errors, and type checking errors.
//!
//! ## Exception Hierarchy
//!
//! ```text
//! MontyError (Error)           # Base class for all Monty exceptions
//! ├── MontySyntaxError         # Raised when syntax is invalid or Monty can't parse the code
//! ├── MontyRuntimeError        # Raised when code fails during execution
//! └── MontyTypingError         # Raised when type checking finds errors in the code
//! ```

use monty::{ExcType, MontyException, StackFrame};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};

use crate::convert::exc_type_to_js_name;

/// Information about the inner exception.
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExceptionInfo {
    /// The exception type name (e.g., "ValueError", "TypeError").
    pub type_name: String,
    /// The exception message.
    pub message: String,
}

/// A single frame in a Monty traceback.
///
/// Contains all the information needed to display a traceback line:
/// the file location, function name, and optional source code preview.
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    /// The filename where the code is located.
    pub filename: String,
    /// Line number (1-based).
    pub line: u32,
    /// Column number (1-based).
    pub column: u32,
    /// End line number (1-based).
    pub end_line: u32,
    /// End column number (1-based).
    pub end_column: u32,
    /// The name of the function, or null for module-level code.
    pub function_name: Option<String>,
    /// The source code line for preview in the traceback.
    pub source_line: Option<String>,
}

impl Frame {
    /// Creates a `Frame` from Monty's `StackFrame`.
    #[must_use]
    pub fn from_stack_frame(frame: &StackFrame) -> Self {
        Self {
            filename: frame.filename.clone(),
            line: u32::from(frame.start.line),
            column: u32::from(frame.start.column),
            end_line: u32::from(frame.end.line),
            end_column: u32::from(frame.end.column),
            function_name: frame.frame_name.clone(),
            source_line: frame.preview_line.clone(),
        }
    }
}

/// Runtime error information including traceback.
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeErrorInfo {
    /// The exception type name and message.
    pub exception: ExceptionInfo,
    /// The full traceback string.
    pub traceback: String,
    /// The traceback frames.
    pub frames: Vec<Frame>,
}

/// Converts a `MontyException` to the appropriate napi `Error`.
///
/// Returns a formatted error message that includes the exception type and traceback.
pub fn monty_exception_to_error(exc: &MontyException) -> Error {
    if exc.exc_type() == ExcType::SyntaxError {
        // Syntax errors don't have tracebacks
        let message = exc.message().unwrap_or_default();
        Error::from_reason(format!("SyntaxError: {message}"))
    } else {
        // Include full traceback for runtime errors
        Error::from_reason(exc.to_string())
    }
}

/// Creates a RuntimeErrorInfo from a MontyException.
#[expect(dead_code, reason = "may be used in future for structured error reporting")]
pub fn create_runtime_error_info(exc: &MontyException) -> RuntimeErrorInfo {
    RuntimeErrorInfo {
        exception: ExceptionInfo {
            type_name: exc_type_to_js_name(exc.exc_type()).to_string(),
            message: exc.message().unwrap_or_default().to_string(),
        },
        traceback: exc.to_string(),
        frames: exc.traceback().iter().map(Frame::from_stack_frame).collect(),
    }
}
