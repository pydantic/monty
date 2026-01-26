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
//!
//! ## JavaScript Usage
//!
//! ```typescript
//! import { Monty, MontySyntaxError, MontyRuntimeError, MontyTypingError } from 'monty';
//!
//! try {
//!     const m = new Monty('def');  // Invalid syntax
//! } catch (e) {
//!     if (e instanceof MontySyntaxError) {
//!         console.log('Syntax error:', e.display('msg'));
//!     }
//! }
//!
//! try {
//!     const m = new Monty('1 / 0');
//!     m.run();
//! } catch (e) {
//!     if (e instanceof MontyRuntimeError) {
//!         console.log('Runtime error:', e.display('traceback'));
//!         console.log('Frames:', e.traceback());
//!     }
//! }
//! ```

use std::fmt;

use monty::{ExcType, MontyException, StackFrame};
use monty_type_checking::TypeCheckingFailure;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};

// =============================================================================
// MontyError - Base class for all Monty exceptions
// =============================================================================

/// Base class for all Monty interpreter errors.
///
/// This is the parent class for `MontySyntaxError`, `MontyRuntimeError`, and `MontyTypingError`.
/// Catching `MontyError` will catch any exception raised by Monty.
///
/// In JavaScript, these are thrown as proper Error subclasses with the appropriate
/// error name set, allowing `instanceof` checks to work correctly.
#[napi]
pub struct MontyError {
    /// The exception type name (e.g., "ValueError", "TypeError").
    type_name: String,
    /// The exception message.
    message: String,
}

impl fmt::Display for MontyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            write!(f, "{}", self.type_name)
        } else {
            write!(f, "{}: {}", self.type_name, self.message)
        }
    }
}

#[napi]
impl MontyError {
    /// Creates a new MontyError with the given type name and message.
    #[napi(constructor)]
    #[must_use]
    pub fn new(type_name: String, message: String) -> Self {
        Self { type_name, message }
    }

    /// Returns information about the inner Python exception.
    ///
    /// Provides structured access to the exception type and message.
    #[napi(getter)]
    #[must_use]
    pub fn exception(&self) -> ExceptionInfo {
        ExceptionInfo {
            type_name: self.type_name.clone(),
            message: self.message.clone(),
        }
    }

    /// Returns the error message.
    #[napi(getter)]
    #[must_use]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// Returns a string representation of the error.
    #[napi(js_name = "toString")]
    #[must_use]
    pub fn to_js_string(&self) -> String {
        self.to_string()
    }
}

impl MontyError {
    /// Creates a MontyError from a MontyException.
    #[must_use]
    pub fn from_exception(exc: &MontyException) -> Self {
        Self {
            type_name: exc.exc_type().to_string(),
            message: exc.message().unwrap_or_default().to_string(),
        }
    }
}

// =============================================================================
// MontySyntaxError - Raised when Python code has syntax errors
// =============================================================================

/// Raised when Python code has syntax errors or cannot be parsed by Monty.
///
/// The inner exception is always a `SyntaxError`. Use `display()` to get
/// formatted error output.
#[napi]
pub struct MontySyntaxError {
    /// The exception message.
    message: String,
}

impl fmt::Display for MontySyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SyntaxError: {}", self.message)
    }
}

#[napi]
impl MontySyntaxError {
    /// Creates a new MontySyntaxError with the given message.
    #[napi(constructor)]
    #[must_use]
    pub fn new(message: String) -> Self {
        Self { message }
    }

    /// Returns information about the inner Python exception.
    #[napi(getter)]
    #[must_use]
    pub fn exception(&self) -> ExceptionInfo {
        ExceptionInfo {
            type_name: "SyntaxError".to_string(),
            message: self.message.clone(),
        }
    }

    /// Returns the error message.
    #[napi(getter)]
    #[must_use]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// Returns formatted exception string.
    ///
    /// @param format - Output format:
    ///   - 'type-msg' - 'ExceptionType: message' format
    ///   - 'msg' - just the message (default)
    #[napi]
    pub fn display(&self, format: Option<String>) -> Result<String> {
        let format = format.as_deref().unwrap_or("msg");
        match format {
            "msg" => Ok(self.message.clone()),
            "type-msg" => Ok(format!("SyntaxError: {}", self.message)),
            _ => Err(Error::from_reason(format!(
                "Invalid display format: '{format}'. Expected 'type-msg' or 'msg'"
            ))),
        }
    }

    /// Returns a string representation of the error.
    #[napi(js_name = "toString")]
    #[must_use]
    pub fn to_js_string(&self) -> String {
        self.to_string()
    }
}

impl MontySyntaxError {
    /// Creates a MontySyntaxError from a MontyException.
    #[must_use]
    pub fn from_exception(exc: &MontyException) -> Self {
        Self {
            message: exc.message().unwrap_or_default().to_string(),
        }
    }

    /// Converts to an napi Error that can be thrown.
    #[must_use]
    pub fn into_error(self) -> Error {
        Error::new(Status::GenericFailure, self.to_string())
    }
}

// =============================================================================
// MontyRuntimeError - Raised when code fails during execution
// =============================================================================

/// Raised when Monty code fails during execution.
///
/// Provides access to the traceback frames where the error occurred via `traceback()`,
/// and formatted output via `display()`.
#[napi]
pub struct MontyRuntimeError {
    /// The exception type name.
    type_name: String,
    /// The exception message.
    message: String,
    /// The full traceback string (pre-formatted).
    traceback_string: String,
    /// The traceback frames.
    frames: Vec<Frame>,
}

impl fmt::Display for MontyRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.traceback_string)
    }
}

#[napi]
impl MontyRuntimeError {
    /// Creates a new MontyRuntimeError.
    #[napi(constructor)]
    #[must_use]
    pub fn new(type_name: String, message: String, traceback_string: String, frames: Vec<Frame>) -> Self {
        Self {
            type_name,
            message,
            traceback_string,
            frames,
        }
    }

    /// Returns information about the inner Python exception.
    #[napi(getter)]
    #[must_use]
    pub fn exception(&self) -> ExceptionInfo {
        ExceptionInfo {
            type_name: self.type_name.clone(),
            message: self.message.clone(),
        }
    }

    /// Returns the error message.
    #[napi(getter)]
    #[must_use]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// Returns the Monty traceback as an array of Frame objects.
    ///
    /// Each frame contains the filename, line/column numbers, function name,
    /// and source code preview line.
    #[napi]
    #[must_use]
    pub fn traceback(&self) -> Vec<Frame> {
        self.frames.clone()
    }

    /// Returns formatted exception string.
    ///
    /// @param format - Output format:
    ///   - 'traceback' - Full traceback (default)
    ///   - 'type-msg' - 'ExceptionType: message' format
    ///   - 'msg' - just the message
    #[napi]
    pub fn display(&self, format: Option<String>) -> Result<String> {
        let format = format.as_deref().unwrap_or("traceback");
        match format {
            "traceback" => Ok(self.traceback_string.clone()),
            "type-msg" => {
                if self.message.is_empty() {
                    Ok(self.type_name.clone())
                } else {
                    Ok(format!("{}: {}", self.type_name, self.message))
                }
            }
            "msg" => Ok(self.message.clone()),
            _ => Err(Error::from_reason(format!(
                "Invalid display format: '{format}'. Expected 'traceback', 'type-msg', or 'msg'"
            ))),
        }
    }

    /// Returns a string representation of the error.
    #[napi(js_name = "toString")]
    #[must_use]
    pub fn to_js_string(&self) -> String {
        self.to_string()
    }
}

impl MontyRuntimeError {
    /// Creates a MontyRuntimeError from a MontyException.
    #[must_use]
    pub fn from_exception(exc: &MontyException) -> Self {
        Self {
            type_name: exc.exc_type().to_string(),
            message: exc.message().unwrap_or_default().to_string(),
            traceback_string: exc.to_string(),
            frames: exc.traceback().iter().map(Frame::from_stack_frame).collect(),
        }
    }

    /// Converts to an napi Error that can be thrown.
    #[must_use]
    pub fn into_error(self) -> Error {
        Error::new(Status::GenericFailure, self.traceback_string)
    }
}

// =============================================================================
// MontyTypingError - Raised when type checking finds errors
// =============================================================================

/// Raised when type checking finds errors in the code.
///
/// This exception is raised when static type analysis detects type errors.
/// Use `display()` to render diagnostics in various formats.
#[napi]
pub struct MontyTypingError {
    /// The type checking failure containing diagnostic information.
    failure: TypeCheckingFailure,
    /// Cached string representation.
    cached_string: String,
}

impl fmt::Display for MontyTypingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.cached_string)
    }
}

#[napi]
impl MontyTypingError {
    /// Returns information about the inner exception.
    #[napi(getter)]
    pub fn exception(&self) -> ExceptionInfo {
        ExceptionInfo {
            type_name: "TypeError".to_string(),
            message: self.cached_string.clone(),
        }
    }

    /// Returns the error message.
    #[napi(getter)]
    pub fn message(&self) -> String {
        self.cached_string.clone()
    }

    /// Renders the type error diagnostics with the specified format and color.
    ///
    /// @param format - Output format. One of:
    ///   - 'full' - Full diagnostic output (default)
    ///   - 'concise' - Concise output
    ///   - 'azure' - Azure DevOps format
    ///   - 'json' - JSON format
    ///   - 'jsonlines' - JSON Lines format
    ///   - 'rdjson' - RDJson format
    ///   - 'pylint' - Pylint format
    ///   - 'gitlab' - GitLab CI format
    ///   - 'github' - GitHub Actions format
    /// @param color - Whether to include ANSI color codes. Default: false
    #[napi]
    pub fn display(&self, format: Option<String>, color: Option<bool>) -> Result<String> {
        let format = format.as_deref().unwrap_or("full");
        let color = color.unwrap_or(false);

        self.failure
            .clone()
            .color(color)
            .format_from_str(format)
            .map_err(Error::from_reason)
            .map(|f| f.to_string())
    }

    /// Returns a string representation of the error.
    #[napi(js_name = "toString")]
    #[must_use]
    pub fn to_js_string(&self) -> String {
        self.to_string()
    }
}

impl MontyTypingError {
    /// Creates a MontyTypingError from a TypeCheckingFailure.
    #[must_use]
    pub fn from_failure(failure: TypeCheckingFailure) -> Self {
        let cached_string = failure.to_string();
        Self { failure, cached_string }
    }

    /// Converts to an napi Error that can be thrown.
    #[must_use]
    pub fn into_error(self) -> Error {
        Error::new(Status::GenericFailure, format!("TypeError: {}", self.cached_string))
    }
}

// =============================================================================
// Helper functions for creating and throwing errors
// =============================================================================

/// Converts a `MontyException` to the appropriate napi `Error`.
///
/// Returns a formatted error message that includes the exception type and traceback.
/// This is used for throwing errors in contexts where we can't use the class-based exceptions.
pub fn monty_exception_to_error(exc: &MontyException) -> Error {
    if exc.exc_type() == ExcType::SyntaxError {
        MontySyntaxError::from_exception(exc).into_error()
    } else {
        MontyRuntimeError::from_exception(exc).into_error()
    }
}

/// Converts a `TypeCheckingFailure` to an napi `Error`.
pub fn typing_failure_to_error(failure: TypeCheckingFailure) -> Error {
    MontyTypingError::from_failure(failure).into_error()
}

/// Information about the inner Python exception.
///
/// This provides structured access to the exception type and message
/// for programmatic error handling.
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

    /// Returns the Frame as a plain object (for compatibility with the interface).
    #[must_use]
    pub fn to_object(&self) -> Self {
        self.clone()
    }
}

// =============================================================================
// Runtime error info (for backward compatibility)
// =============================================================================

/// Runtime error information including traceback.
///
/// This is provided for backward compatibility and structured error handling
/// in cases where class instances cannot be used directly.
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

/// Creates a RuntimeErrorInfo from a MontyException.
#[expect(dead_code, reason = "may be used in future for structured error reporting")]
pub fn create_runtime_error_info(exc: &MontyException) -> RuntimeErrorInfo {
    RuntimeErrorInfo {
        exception: ExceptionInfo {
            type_name: exc.exc_type().to_string(),
            message: exc.message().unwrap_or_default().to_string(),
        },
        traceback: exc.to_string(),
        frames: exc.traceback().iter().map(Frame::from_stack_frame).collect(),
    }
}
