//! Exception types for the Monty TypeScript/JavaScript bindings.
//!
//! This module provides thin napi wrappers around Monty's internal exceptions.
//! The JavaScript wrapper layer (`wrapper.js`) is responsible for converting
//! these into proper JS `Error` subclasses (`MontySyntaxError`, `MontyRuntimeError`).
//!
//! It is done this way because `napi` has no way to create JS `Error` subclasses from
//! Rust.
//!
//! ## Architecture
//!
//! - `JsMontyException`: Thin wrapper around `monty::MontyException`. The JS wrapper
//!   checks `exception.typeName` to distinguish syntax errors from runtime errors.
//! - `MontyTypingError`: Wraps `TypeCheckingDiagnostics` for static type checking errors.
//!   This is separate because type errors come from static analysis, not Python execution.

use std::{collections::HashMap, fmt, sync::Arc};

use monty_type_checking::TypeCheckingDiagnostics;
use napi::{bindgen_prelude::*, JsString};
use napi_derive::napi;
use serde::{Deserialize, Serialize};

// =============================================================================
// JsMontyException - Thin wrapper around core MontyException
// =============================================================================

/// Wrapper around core `MontyException` for napi bindings.
///
/// This is a thin newtype wrapper that exposes the necessary getters for the
/// JavaScript wrapper to construct appropriate error types (`MontySyntaxError`
/// or `MontyRuntimeError`) based on the exception type.
#[napi(js_name = "MontyException")]
pub struct JsMontyException(monty::MontyException);

impl fmt::Display for JsMontyException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[napi]
impl JsMontyException {
    /// Returns information about the inner Python exception.
    ///
    /// The `typeName` field can be used to distinguish syntax errors (`"SyntaxError"`)
    /// from runtime errors (e.g., `"ValueError"`, `"TypeError"`).
    #[napi(getter)]
    #[must_use]
    pub fn exception(&self) -> ExceptionInfo {
        ExceptionInfo {
            type_name: self.0.exc_type().to_string(),
            message: self.0.message().unwrap_or_default().to_string(),
        }
    }

    /// Returns the error message.
    #[napi(getter)]
    #[must_use]
    pub fn message(&self) -> String {
        self.0.message().unwrap_or_default().to_string()
    }

    /// Returns the Monty traceback as an array of Frame objects.
    ///
    /// For syntax errors, this will be an empty array.
    /// For runtime errors, this contains the stack frames where the error occurred.
    ///
    /// `Frame.source_line` is built as a `JsString` shared across frames that
    /// resolve to the same source line. For deep recursion where every frame
    /// points at the same line this creates a single V8 string referenced by
    /// every frame, instead of one copy per frame.
    #[napi]
    pub fn traceback<'env>(&self, env: &'env Env) -> Result<Vec<Frame<'env>>> {
        let stack_frames = self.0.traceback();
        let mut line_cache: HashMap<usize, JsString<'env>> = HashMap::new();
        stack_frames
            .iter()
            .map(|f| {
                let source_line = f
                    .preview_line
                    .as_ref()
                    .map(|arc| -> Result<JsString<'env>> {
                        let key = Arc::as_ptr(arc).cast::<()>() as usize;
                        if let Some(s) = line_cache.get(&key) {
                            Ok(*s)
                        } else {
                            let s = env.create_string(arc)?;
                            line_cache.insert(key, s);
                            Ok(s)
                        }
                    })
                    .transpose()?;
                Ok(Frame {
                    filename: f.filename.clone(),
                    line: f.start.line,
                    column: f.start.column,
                    end_line: f.end.line,
                    end_column: f.end.column,
                    function_name: f.frame_name.clone(),
                    source_line,
                })
            })
            .collect()
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
            "traceback" => Ok(self.0.to_string()),
            "type-msg" => {
                let type_name = self.0.exc_type().to_string();
                let message = self.0.message().unwrap_or_default();
                if message.is_empty() {
                    Ok(type_name)
                } else {
                    Ok(format!("{type_name}: {message}"))
                }
            }
            "msg" => Ok(self.0.message().unwrap_or_default().to_string()),
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

impl JsMontyException {
    /// Creates a new JsMontyException from a core MontyException.
    #[must_use]
    pub fn new(exc: monty::MontyException) -> Self {
        Self(exc)
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
    failure: TypeCheckingDiagnostics,
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
    #[must_use]
    pub fn exception(&self) -> ExceptionInfo {
        ExceptionInfo {
            type_name: "TypeError".to_string(),
            message: self.cached_string.clone(),
        }
    }

    /// Returns the error message.
    #[napi(getter)]
    #[must_use]
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
    /// Creates a MontyTypingError from a TypeCheckingDiagnostics.
    #[must_use]
    pub fn from_failure(failure: TypeCheckingDiagnostics) -> Self {
        let cached_string = failure.to_string();
        Self { failure, cached_string }
    }
}

// =============================================================================
// Helper types
// =============================================================================

/// Information about the inner Python exception.
///
/// This provides structured access to the exception type and message
/// for programmatic error handling.
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExceptionInfo {
    /// The exception type name (e.g., "ValueError", "TypeError", "SyntaxError").
    pub type_name: String,
    /// The exception message.
    pub message: String,
}

/// A single frame in a Monty traceback.
///
/// Contains all the information needed to display a traceback line:
/// the file location, function name, and optional source code preview.
///
/// `source_line` is a `JsString` borrowed from the env scope of the
/// `traceback()` call that produced this frame. Frames produced by the same
/// `traceback()` call that resolve to the same source location share one V8
/// string allocation. The lifetime parameter ties the frame to that env
/// scope, since `JsString<'env>` is a non-owning handle.
#[napi(object)]
pub struct Frame<'env> {
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
    pub source_line: Option<JsString<'env>>,
}
