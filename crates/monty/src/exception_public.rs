use std::fmt::{self, Write};

use crate::{
    exception_private::{ExcType, RawStackFrame},
    intern::Interns,
    parse::CodeRange,
    types::str::StringRepr,
};

/// Public representation of a Monty exception.
#[derive(Debug, Clone, PartialEq)]
pub struct MontyException {
    /// The exception type raised
    exc_type: ExcType,
    /// Optional exception message explaining what went wrong
    message: Option<String>,
    /// Stack trace of the exception, first is the outermost frame shown first in the traceback
    traceback: Vec<StackFrame>,
}

/// Number of identical consecutive frames to show before collapsing.
///
/// CPython shows 3 identical frames, then "[Previous line repeated N more times]".
const REPEAT_FRAMES_SHOWN: usize = 3;

/// Display implementation for MontyException should exactly match python traceback format.
impl fmt::Display for MontyException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print the traceback header if we have frames
        if !self.traceback.is_empty() {
            writeln!(f, "Traceback (most recent call last):")?;
        }

        // Print frames, collapsing consecutive identical frames like CPython does
        let mut i = 0;
        while i < self.traceback.len() {
            let frame = &self.traceback[i];

            // Count consecutive identical frames
            let mut repeat_count = 1;
            while i + repeat_count < self.traceback.len()
                && frames_are_identical(frame, &self.traceback[i + repeat_count])
            {
                repeat_count += 1;
            }

            if repeat_count > REPEAT_FRAMES_SHOWN {
                // Show first REPEAT_FRAMES_SHOWN frames, then collapse the rest
                for j in 0..REPEAT_FRAMES_SHOWN {
                    write!(f, "{}", &self.traceback[i + j])?;
                }
                let collapsed = repeat_count - REPEAT_FRAMES_SHOWN;
                writeln!(f, "  [Previous line repeated {collapsed} more times]")?;
                i += repeat_count;
            } else {
                // Show all frames in this group
                for j in 0..repeat_count {
                    write!(f, "{}", &self.traceback[i + j])?;
                }
                i += repeat_count;
            }
        }

        if let Some(msg) = &self.message {
            write!(f, "{}: {}", self.exc_type, msg)
        } else {
            write!(f, "{}", self.exc_type)
        }
    }
}

impl std::error::Error for MontyException {}

impl MontyException {
    /// Create a new MontyException with the given exception type and message.
    ///
    /// You can't provide a traceback here, it's send when raising the exception.
    #[must_use]
    pub fn new(exc_type: ExcType, message: Option<String>) -> Self {
        Self {
            exc_type,
            message,
            traceback: vec![],
        }
    }

    /// The exception type raised.
    #[must_use]
    pub fn exc_type(&self) -> ExcType {
        self.exc_type
    }

    /// Optional exception message explaining what went wrong.
    ///
    /// Equivalent of python's `exc.args[0]`
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Optional exception message explaining what went wrong.
    ///
    /// This takes ownership of the MontyException and returns an owned String.
    ///
    /// Equivalent of python's `exc.args[0]`
    #[must_use]
    pub fn into_message(self) -> Option<String> {
        self.message
    }

    /// Stack trace of the exception, first is the outermost frame shown first in the traceback
    #[must_use]
    pub fn traceback(&self) -> &[StackFrame] {
        &self.traceback
    }

    /// Returns a compact summary of the exception.
    ///
    /// Format: `ExceptionType: message` (e.g., `NotImplementedError: feature not supported`)
    /// If there's no message, just returns the exception type name.
    #[must_use]
    pub fn summary(&self) -> String {
        if let Some(msg) = &self.message {
            format!("{}: {}", self.exc_type, msg)
        } else {
            self.exc_type.to_string()
        }
    }

    /// Returns the exception formatted as Python's repr() would display it.
    ///
    /// Format: `ExceptionType('message')` (e.g., `ValueError('invalid value')`)
    /// Uses appropriate quoting for messages containing quotes.
    #[must_use]
    pub fn py_repr(&self) -> String {
        let type_str: &'static str = self.exc_type.into();
        if let Some(msg) = &self.message {
            format!("{}({})", type_str, StringRepr(msg))
        } else {
            format!("{type_str}()")
        }
    }

    pub(crate) fn new_full(exc_type: ExcType, message: Option<String>, traceback: Vec<StackFrame>) -> Self {
        Self {
            exc_type,
            message,
            traceback,
        }
    }

    pub(crate) fn runtime_error(err: impl fmt::Display) -> Self {
        Self {
            exc_type: ExcType::RuntimeError,
            message: Some(err.to_string()),
            traceback: vec![],
        }
    }
}

/// Check if two stack frames are identical for the purpose of collapsing repeated frames.
///
/// Two frames are identical if they have the same filename, line number, and function name.
fn frames_are_identical(a: &StackFrame, b: &StackFrame) -> bool {
    a.filename == b.filename && a.start.line == b.start.line && a.frame_name == b.frame_name
}

/// A single frame in a Python traceback.
///
/// Contains all the information needed to display a traceback line:
/// the file location, function name, and optional source code preview.
///
/// # Caret Markers
///
/// Monty uses only `~` characters for caret markers in tracebacks, unlike CPython 3.11+
/// which uses `~` for the function name and `^` for arguments (e.g., `~~~~~~~~~~~^^^^^^^^^^^`).
/// This simplification is intentional - Monty marks the entire expression span uniformly.
#[derive(Debug, Clone, PartialEq)]
pub struct StackFrame {
    /// The filename where the code is located.
    pub filename: String,
    /// Start position in the source code.
    pub start: CodeLoc,
    /// End position in the source code.
    pub end: CodeLoc,
    /// The name of the frame (function name, or None for module-level code).
    pub frame_name: Option<String>,
    /// The source code line for preview in the traceback.
    pub preview_line: Option<String>,
    /// Whether to hide the caret marker in the traceback for this frame.
    ///
    /// Set to `true` for:
    /// - `raise` statements (CPython doesn't show carets for raise)
    /// - `AttributeError` on attribute access (CPython doesn't show carets for these)
    pub hide_caret: bool,
}

impl fmt::Display for StackFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, r#"  File "{}", line {}, in "#, self.filename, self.start.line)?;
        if let Some(frame_name) = &self.frame_name {
            f.write_str(frame_name)?;
        } else {
            f.write_str("<module>")?;
        }

        if let Some(line) = &self.preview_line {
            // Strip leading whitespace like CPython does
            let trimmed = line.trim_start();
            writeln!(f, "\n    {trimmed}")?;

            // Hide caret for raise statements, AttributeError, etc.
            if !self.hide_caret {
                let leading_spaces = line.len() - trimmed.len();
                // Calculate caret position relative to the trimmed line
                // Column is 1-indexed, so subtract 1, then subtract leading spaces we stripped
                let caret_start = if self.start.column as usize > leading_spaces {
                    4 + self.start.column as usize - leading_spaces - 1
                } else {
                    4
                };
                f.write_str(&" ".repeat(caret_start))?;
                writeln!(f, "{}", "~".repeat((self.end.column - self.start.column) as usize))?;
            }
        } else {
            f.write_char('\n')?;
        }
        Ok(())
    }
}

impl StackFrame {
    pub(crate) fn from_raw(f: &RawStackFrame, interns: &Interns, source: &str) -> Self {
        let filename = interns.get_str(f.position.filename).to_string();
        Self {
            filename,
            start: f.position.start(),
            end: f.position.end(),
            frame_name: f.frame_name.map(|id| interns.get_str(id).to_string()),
            preview_line: f
                .position
                .preview_line_number()
                .and_then(|ln| source.lines().nth(ln as usize))
                .map(str::to_string),
            hide_caret: f.hide_caret,
        }
    }

    pub(crate) fn from_position(position: CodeRange, filename: &str, source: &str) -> Self {
        Self {
            filename: filename.to_string(),
            start: position.start(),
            end: position.end(),
            frame_name: None,
            preview_line: position
                .preview_line_number()
                .and_then(|ln| source.lines().nth(ln as usize))
                .map(str::to_string),
            hide_caret: false,
        }
    }

    /// Creates a `StackFrame` from a `CodeRange` without caret markers.
    ///
    /// Used for errors like `ImportError` where CPython doesn't show caret markers.
    pub(crate) fn from_position_no_caret(position: CodeRange, filename: &str, source: &str) -> Self {
        Self {
            filename: filename.to_string(),
            start: position.start(),
            end: position.end(),
            frame_name: None,
            preview_line: position
                .preview_line_number()
                .and_then(|ln| source.lines().nth(ln as usize))
                .map(str::to_string),
            hide_caret: true,
        }
    }
}

/// A line and column position in source code.
///
/// Uses 1-based indexing for both line and column to match Python's conventions.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CodeLoc {
    /// Line number (1-based).
    pub line: u16,
    /// Column number (1-based).
    pub column: u16,
}

impl Default for CodeLoc {
    fn default() -> Self {
        Self { line: 1, column: 1 }
    }
}

impl CodeLoc {
    /// Creates a new CodeLoc from usize values.
    ///
    /// Lines and columns numbers are 1-indexed for display, hence `+1`
    ///
    /// # Panics
    /// Panics if the line or column number overflows `u16`.
    #[must_use]
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            line: u16::try_from(line).expect("Line number overflow") + 1,
            column: u16::try_from(column).expect("Column number overflow") + 1,
        }
    }
}
