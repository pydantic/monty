//! Public exception types: [`ExcType`], [`MontyException`] and its
//! traceback/payload components ([`StackFrame`], [`CodeLoc`], [`ExcData`]).

use std::{
    error,
    fmt::{self, Write},
    mem, str,
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::format::StringRepr;

/// Public representation of a Monty exception.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MontyException {
    /// The exception type raised
    exc_type: ExcType,
    /// Optional exception message explaining what went wrong
    message: Option<String>,
    /// Stack trace of the exception, first is the outermost frame shown first in the traceback
    traceback: Vec<StackFrame>,
    /// Structured payload for exception types that carry more than a message.
    /// No `skip_serializing_if`: exceptions round-trip through
    /// non-self-describing snapshot formats where skipped fields break
    /// deserialization.
    #[serde(default)]
    data: ExcData,
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
                    write!(f, "{}", self.traceback[i + j])?;
                }
                let collapsed = repeat_count - REPEAT_FRAMES_SHOWN;
                writeln!(f, "  [Previous line repeated {collapsed} more times]")?;
                i += repeat_count;
            } else {
                // Show all frames in this group
                for j in 0..repeat_count {
                    write!(f, "{}", self.traceback[i + j])?;
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

impl error::Error for MontyException {}

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
            data: ExcData::None,
        }
    }

    /// Creates an exception with an explicit traceback.
    ///
    /// Most callers should use [`MontyException::new`] — the traceback is
    /// normally attached when the exception is raised. This constructor
    /// exists for boundaries that *reconstruct* an exception that was raised
    /// elsewhere (e.g. deserializing one received from a `monty subprocess`
    /// worker) and must preserve its original frames.
    #[must_use]
    pub fn with_traceback(exc_type: ExcType, message: Option<String>, traceback: Vec<StackFrame>) -> Self {
        Self {
            exc_type,
            message,
            traceback,
            data: ExcData::None,
        }
    }

    /// Attaches a structured payload — see [`ExcData`]. Public for
    /// boundaries that reconstruct an exception raised elsewhere (like
    /// [`MontyException::with_traceback`]); in-process raises attach the
    /// payload at the raise site instead.
    #[must_use]
    pub fn with_data(mut self, data: ExcData) -> Self {
        self.data = data;
        self
    }

    /// The structured payload, [`ExcData::None`] for most exceptions.
    #[must_use]
    pub fn data(&self) -> &ExcData {
        &self.data
    }

    /// Structured `UnicodeDecodeError`/`UnicodeEncodeError` fields, present
    /// only for unicode errors raised by codec operations on objects no
    /// larger than [`UnicodeErrorData::MAX_OBJECT_LEN`].
    #[must_use]
    pub fn unicode_data(&self) -> Option<&UnicodeErrorData> {
        self.data.unicode()
    }

    /// Structured `json.JSONDecodeError` fields, present only for decode
    /// errors raised by `json.loads` (not for manually raised exceptions).
    #[must_use]
    pub fn json_data(&self) -> Option<&JsonErrorData> {
        self.data.json()
    }

    /// Removes and returns the structured payload, for consumers (like the
    /// Python bindings) that rebuild the native exception and want the
    /// payload by value without cloning it.
    #[must_use]
    pub fn take_data(&mut self) -> ExcData {
        mem::take(&mut self.data)
    }

    /// Appends frames to this exception's traceback.
    pub fn add_traceback(&mut self, traceback: impl IntoIterator<Item = StackFrame>) {
        self.traceback.extend(traceback);
    }

    /// Shorthand for a traceback-free `RuntimeError` wrapping `err`'s display
    /// output — used at host boundaries (input conversion, REPL feeds) where
    /// no sandbox stack frames exist.
    #[must_use]
    pub fn runtime_error(err: impl fmt::Display) -> Self {
        Self {
            exc_type: ExcType::RuntimeError,
            message: Some(err.to_string()),
            traceback: vec![],
            data: ExcData::None,
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
}

/// Check if two stack frames are identical for the purpose of collapsing repeated frames.
///
/// Two frames are identical if they have the same filename, line number, and function name.
fn frames_are_identical(a: &StackFrame, b: &StackFrame) -> bool {
    a.filename == b.filename && a.start.line == b.start.line && a.frame_name == b.frame_name
}

/// Python exception types supported by the interpreter.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `Into<&'static str>` implementations.
/// The string representation matches the variant name exactly (e.g., `ValueError` -> "ValueError").
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Display, EnumString, IntoStaticStr, Serialize, Deserialize,
)]
pub enum ExcType {
    /// primary exception class - matches any exception in isinstance checks.
    ///
    /// Also the `Default` — required so `Type` (which embeds an `ExcType` in
    /// its `Exception` variant) can derive `strum::EnumIter`.
    #[default]
    Exception,

    /// System exit exceptions
    BaseException,
    SystemExit,
    KeyboardInterrupt,

    // --- ArithmeticError hierarchy ---
    /// Intermediate class for arithmetic errors.
    ArithmeticError,
    /// Subclass of ArithmeticError.
    OverflowError,
    /// Subclass of ArithmeticError.
    ZeroDivisionError,

    // --- LookupError hierarchy ---
    /// Intermediate class for lookup errors.
    LookupError,
    /// Subclass of LookupError.
    IndexError,
    /// Subclass of LookupError.
    KeyError,

    // --- RuntimeError hierarchy ---
    /// Intermediate class for runtime errors.
    RuntimeError,
    /// Subclass of RuntimeError.
    NotImplementedError,
    /// Subclass of RuntimeError.
    RecursionError,

    // --- AttributeError hierarchy ---
    AttributeError,
    /// Subclass of AttributeError (from dataclasses module).
    FrozenInstanceError,

    // --- NameError hierarchy ---
    NameError,
    /// Subclass of NameError - for accessing local variable before assignment.
    UnboundLocalError,

    // --- ValueError hierarchy ---
    ValueError,
    /// Subclass of ValueError - for encoding/decoding errors.
    UnicodeDecodeError,
    /// Subclass of ValueError - for encoding errors (e.g. `str.encode('ascii')`
    /// on a string containing non-ASCII characters).
    UnicodeEncodeError,
    /// Subclass of ValueError for invalid JSON syntax in `json.loads()`.
    #[strum(serialize = "json.JSONDecodeError")]
    JsonDecodeError,

    // --- ImportError hierarchy ---
    /// Import-related errors (module not found, name not in module).
    ImportError,
    /// Subclass of ImportError - for when a module cannot be found.
    ModuleNotFoundError,

    // --- OSError hierarchy ---
    /// OS-related errors (file not found, permission denied, etc.)
    OSError,
    /// Subclass of OSError - for when a file or directory cannot be found.
    FileNotFoundError,
    /// Subclass of OSError - for when a file already exists.
    FileExistsError,
    /// Subclass of OSError - for when a path is a directory but a file was expected.
    IsADirectoryError,
    /// Subclass of OSError - for when a path is not a directory but one was expected.
    NotADirectoryError,
    /// Subclass of OSError - for when an operation is not permitted (e.g., writing
    /// to a read-only mount, or attempting to access a path outside a mounted directory).
    PermissionError,
    /// `io.UnsupportedOperation` - raised by file objects when a requested
    /// operation isn't allowed by the open mode (e.g. `read()` on `'w'`).
    ///
    /// In CPython this inherits from both `OSError` and `ValueError`. Monty's
    /// `ExcType` enum models single parents, but [`Self::is_subclass_of`]
    /// matches `UnsupportedOperation` against both `OSError` and `ValueError`
    /// so `except ValueError:` and `except OSError:` both catch it as in
    /// CPython.
    #[strum(serialize = "io.UnsupportedOperation")]
    UnsupportedOperation,
    /// Subclass of OSError since Python 3.3 (PEP 3151).
    TimeoutError,

    // --- Standalone exception types ---
    AssertionError,
    MemoryError,
    StopIteration,
    SyntaxError,
    TypeError,

    // --- Module-specific exception types ---

    // --- re module ---
    /// `re.PatternError` - raised for invalid regex patterns or unsupported regex features.
    ///
    /// # Behavior Note
    ///
    /// Limited to monty's exception type, `PatternError` does not provide `pattern`, `pos`,
    /// `lineno` and `colno` attributes.
    ///
    /// As per CPython's implementation, it would be hard to convert `fancy-regex`'s error
    /// representations into the required attributes.
    #[strum(serialize = "re.PatternError")]
    RePatternError,
}
impl ExcType {
    /// Checks if this exception type is a subclass of another exception type.
    ///
    /// Implements Python's exception hierarchy for try/except matching:
    /// - `Exception` is the base class for all standard exceptions
    /// - `LookupError` is the base for `KeyError` and `IndexError`
    /// - `ArithmeticError` is the base for `ZeroDivisionError` and `OverflowError`
    /// - `RuntimeError` is the base for `RecursionError` and `NotImplementedError`
    ///
    /// Returns true if `self` would be caught by `except handler_type:`.
    #[must_use]
    pub fn is_subclass_of(self, handler_type: Self) -> bool {
        if self == handler_type {
            return true;
        }
        match handler_type {
            // BaseException catches all exceptions
            Self::BaseException => true,
            // Exception catches everything except BaseException, and direct subclasses: KeyboardInterrupt, SystemExit
            Self::Exception => !matches!(self, Self::BaseException | Self::KeyboardInterrupt | Self::SystemExit),
            // LookupError catches KeyError and IndexError
            Self::LookupError => matches!(self, Self::KeyError | Self::IndexError),
            // ArithmeticError catches ZeroDivisionError and OverflowError
            Self::ArithmeticError => matches!(self, Self::ZeroDivisionError | Self::OverflowError),
            // RuntimeError catches RecursionError and NotImplementedError
            Self::RuntimeError => matches!(self, Self::RecursionError | Self::NotImplementedError),
            // AttributeError catches FrozenInstanceError
            Self::AttributeError => matches!(self, Self::FrozenInstanceError),
            // NameError catches UnboundLocalError
            Self::NameError => matches!(self, Self::UnboundLocalError),
            // ValueError catches UnicodeDecodeError, UnicodeEncodeError, json.JSONDecodeError,
            // and io.UnsupportedOperation (which in CPython has dual OSError + ValueError parentage)
            Self::ValueError => matches!(
                self,
                Self::UnicodeDecodeError
                    | Self::UnicodeEncodeError
                    | Self::JsonDecodeError
                    | Self::UnsupportedOperation
            ),
            // ImportError catches ModuleNotFoundError
            Self::ImportError => matches!(self, Self::ModuleNotFoundError),
            // OSError catches FileNotFoundError, FileExistsError, IsADirectoryError,
            // NotADirectoryError, PermissionError, io.UnsupportedOperation, and
            // TimeoutError (an OSError subclass since Python 3.3)
            Self::OSError => matches!(
                self,
                Self::FileNotFoundError
                    | Self::FileExistsError
                    | Self::IsADirectoryError
                    | Self::NotADirectoryError
                    | Self::PermissionError
                    | Self::UnsupportedOperation
                    | Self::TimeoutError
            ),
            // All other types only match exactly (handled by self == handler_type above)
            _ => false,
        }
    }
}

/// Structured payload attached to exception types whose CPython counterparts
/// carry more than a message. Currently unicode and json decode errors have
/// one; the enum leaves room for future variants (e.g. `OSError`'s
/// `errno`/`filename`) without another field on every exception.
#[derive(Debug, Clone, Default, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ExcData {
    /// No structured payload — every exception type without a variant below.
    #[default]
    None,
    /// `UnicodeDecodeError` / `UnicodeEncodeError` constructor fields.
    /// Boxed to keep the common `None` case (and every exception embedding
    /// this enum) small.
    Unicode(Box<UnicodeErrorData>),
    /// `json.JSONDecodeError` attribute fields. Boxed like
    /// [`ExcData::Unicode`] to keep the enum small.
    Json(Box<JsonErrorData>),
}

impl ExcData {
    /// The unicode-error fields, if this is [`ExcData::Unicode`].
    #[must_use]
    pub fn unicode(&self) -> Option<&UnicodeErrorData> {
        match self {
            Self::Unicode(data) => Some(data),
            _ => None,
        }
    }

    /// The json-error fields, if this is [`ExcData::Json`].
    #[must_use]
    pub fn json(&self) -> Option<&JsonErrorData> {
        match self {
            Self::Json(data) => Some(data),
            _ => None,
        }
    }

    /// Approximate byte footprint, used by the heap's memory accounting when
    /// an exception carrying this payload is stored on the sandbox heap.
    #[must_use]
    pub fn estimate_size(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Unicode(data) => data.estimate_size(),
            Self::Json(data) => data.estimate_size(),
        }
    }
}

/// Structured fields of a `UnicodeDecodeError` / `UnicodeEncodeError`,
/// mirroring CPython's `encoding` / `object` / `start` / `end` / `reason`
/// exception attributes.
///
/// Monty exceptions are otherwise message-only; unicode errors additionally
/// carry these fields so host bindings (e.g. `pydantic_monty`) can construct
/// real `UnicodeDecodeError` / `UnicodeEncodeError` instances instead of
/// falling back to a plain `ValueError`. The payload is omitted when the
/// offending object is larger than [`UnicodeErrorData::MAX_OBJECT_LEN`] —
/// exceptions can be stored and copied outside the sandbox's resource
/// tracker, so an unbounded payload would let huge inputs evade memory
/// limits. Sandboxed code never sees these fields (in-sandbox exceptions
/// expose only `args`).
#[derive(Debug, Clone, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct UnicodeErrorData {
    /// The codec name as CPython reports it, e.g. `"utf-8"`, `"ascii"`.
    pub encoding: String,
    /// The full input that failed to encode/decode (`str` for encode errors,
    /// `bytes` for decode errors), matching CPython's `exc.object`.
    pub object: UnicodeErrorObject,
    /// Start of the failing range: a character index for encode errors, a
    /// byte offset for decode errors.
    pub start: usize,
    /// Exclusive end of the failing range, in the same units as `start`.
    pub end: usize,
    /// CPython's reason wording, e.g. `"ordinal not in range(128)"`.
    pub reason: String,
}

/// The `object` attribute of a unicode error: the input being converted.
#[derive(Debug, Clone, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub enum UnicodeErrorObject {
    /// A decode error's input `bytes`.
    Bytes(Vec<u8>),
    /// An encode error's input `str`.
    Str(String),
}

impl UnicodeErrorData {
    /// Payload size cap: unicode errors on objects larger than this carry no
    /// structured data (hosts fall back to the message-only `ValueError`).
    /// Exception payloads live outside the sandbox's resource tracker once
    /// the exception escapes, so the cap bounds how much untracked memory a
    /// single raise can pin.
    pub const MAX_OBJECT_LEN: usize = 64 * 1024;

    /// Builds the payload for an encode error on `object`, or
    /// [`ExcData::None`] when `object` exceeds [`Self::MAX_OBJECT_LEN`].
    #[must_use]
    pub fn encode(encoding: &str, object: &str, start: usize, end: usize, reason: &str) -> ExcData {
        if object.len() <= Self::MAX_OBJECT_LEN {
            ExcData::Unicode(Box::new(Self {
                encoding: encoding.to_owned(),
                object: UnicodeErrorObject::Str(object.to_owned()),
                start,
                end,
                reason: reason.to_owned(),
            }))
        } else {
            ExcData::None
        }
    }

    /// Builds the payload for a decode error on `object`, or
    /// [`ExcData::None`] when `object` exceeds [`Self::MAX_OBJECT_LEN`].
    /// Public so `monty-fs` can build the payload for text-mode file reads.
    #[must_use]
    pub fn decode(encoding: &str, object: &[u8], start: usize, end: usize, reason: &str) -> ExcData {
        if object.len() <= Self::MAX_OBJECT_LEN {
            ExcData::Unicode(Box::new(Self {
                encoding: encoding.to_owned(),
                object: UnicodeErrorObject::Bytes(object.to_vec()),
                start,
                end,
                reason: reason.to_owned(),
            }))
        } else {
            ExcData::None
        }
    }

    /// Approximate byte footprint, used by the heap's memory accounting when
    /// an exception carrying this payload is stored on the sandbox heap.
    #[must_use]
    pub fn estimate_size(&self) -> usize {
        let object_len = match &self.object {
            UnicodeErrorObject::Bytes(b) => b.len(),
            UnicodeErrorObject::Str(s) => s.len(),
        };
        mem::size_of::<Self>() + self.encoding.len() + object_len + self.reason.len()
    }
}

/// Structured fields of a `json.JSONDecodeError`, mirroring CPython's `msg` /
/// `doc` / `pos` / `lineno` / `colno` exception attributes.
///
/// As with [`UnicodeErrorData`], the payload exists so host bindings can
/// construct a real `json.JSONDecodeError` instead of falling back to a plain
/// `ValueError`; sandboxed code never sees these fields. `lineno`/`colno` are
/// carried explicitly rather than recomputed from `doc` because `doc` may be
/// absent (see [`JsonErrorData::MAX_DOC_LEN`]).
#[derive(Debug, Clone, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct JsonErrorData {
    /// The bare error message, without the `: line N column M (char K)`
    /// suffix the formatted exception message carries.
    pub msg: String,
    /// The document being parsed, matching CPython's `exc.doc`. `None` when
    /// the document exceeds [`JsonErrorData::MAX_DOC_LEN`] or is not valid
    /// UTF-8 (`json.loads` on `bytes` input).
    pub doc: Option<String>,
    /// Character index of the error in `doc`, matching CPython's `exc.pos`.
    pub pos: usize,
    /// 1-based line of the error, matching CPython's `exc.lineno`.
    pub lineno: usize,
    /// 1-based column of the error, matching CPython's `exc.colno`.
    pub colno: usize,
}

impl JsonErrorData {
    /// Document size cap, mirroring [`UnicodeErrorData::MAX_OBJECT_LEN`]:
    /// exception payloads live outside the sandbox's resource tracker once
    /// the exception escapes, so `doc` is dropped (not truncated — a partial
    /// document would misplace `pos`) for larger inputs.
    pub const MAX_DOC_LEN: usize = 64 * 1024;

    /// Builds the payload for a decode error on `doc`, omitting the document
    /// when it exceeds [`Self::MAX_DOC_LEN`] or is not valid UTF-8.
    #[must_use]
    pub fn build(msg: &str, doc: &[u8], pos: usize, lineno: usize, colno: usize) -> ExcData {
        let doc = if doc.len() <= Self::MAX_DOC_LEN {
            str::from_utf8(doc).ok().map(ToOwned::to_owned)
        } else {
            None
        };
        ExcData::Json(Box::new(Self {
            msg: msg.to_owned(),
            doc,
            pos,
            lineno,
            colno,
        }))
    }

    /// Approximate byte footprint, used by the heap's memory accounting when
    /// an exception carrying this payload is stored on the sandbox heap.
    #[must_use]
    pub fn estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.msg.len() + self.doc.as_ref().map_or(0, String::len)
    }
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    ///
    /// Stored as `Arc<str>` rather than `String` so that consecutive frames
    /// referencing the same source line — typical of recursion and tight
    /// helper-function loops — share a single allocation. Without sharing, a
    /// 1000-deep recursive call into code on a long line would clone the
    /// entire line into each frame and amplify memory usage by the call
    /// depth. Serialization roundtrips lose the sharing (each frame gets
    /// its own `Arc`), but that is bounded by the wire size of the
    /// traceback so does not regress the amplification.
    pub preview_line: Option<Arc<str>>,
    /// Whether to hide the caret marker in the traceback for this frame.
    ///
    /// Set to `true` for:
    /// - `raise` statements (CPython doesn't show carets for raise)
    /// - `AttributeError` on attribute access (CPython doesn't show carets for these)
    pub hide_caret: bool,
    /// Whether to hide the `, in <name>` part of the frame line.
    ///
    /// Set to `true` for `SyntaxError` where CPython doesn't show the frame name.
    /// CPython's SyntaxError format: `  File "...", line N`
    /// vs runtime error format: `  File "...", line N, in <module>`
    pub hide_frame_name: bool,
}

impl fmt::Display for StackFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SyntaxError format: `  File "...", line N`
        // Runtime error format: `  File "...", line N, in <module>`
        if self.hide_frame_name {
            write!(f, r#"  File "{}", line {}"#, self.filename, self.start.line)?;
        } else {
            write!(f, r#"  File "{}", line {}, in "#, self.filename, self.start.line)?;
            if let Some(frame_name) = &self.frame_name {
                f.write_str(frame_name)?;
            } else {
                f.write_str("<module>")?;
            }
        }

        if let Some(line) = &self.preview_line {
            if self.start.line != self.end.line {
                // Multi-line statement range: `preview_line` holds a
                // pre-rendered, dedented block (see `SourceMap::multiline_preview`).
                // CPython prints each line at the 4-space frame indent with no
                // caret markers.
                f.write_char('\n')?;
                for block_line in line.lines() {
                    writeln!(f, "    {block_line}")?;
                }
                return Ok(());
            }
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
                // Always render at least one caret, even for zero-length ranges
                // (e.g. a SyntaxError pointing just past the end of a truncated token).
                let caret_len = (self.end.column - self.start.column).max(1) as usize;
                writeln!(f, "{}", "~".repeat(caret_len))?;
            }
        } else {
            f.write_char('\n')?;
        }
        Ok(())
    }
}

/// A line and column position in source code.
///
/// Uses 1-based indexing for both line and column to match Python's conventions.
///
/// `u32` matches `ruff_text_size::TextSize`, which underpins all source ranges
/// returned by the parser, so conversions between the two are zero-cost.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CodeLoc {
    /// Line number (1-based).
    pub line: u32,
    /// Column number (1-based), counted in characters (not bytes).
    pub column: u32,
}

impl Default for CodeLoc {
    fn default() -> Self {
        Self { line: 1, column: 1 }
    }
}

impl CodeLoc {
    /// Creates a new CodeLoc from 0-based values.
    ///
    /// Lines and columns numbers are 1-indexed for display, hence `+ 1`.
    /// Saturates at `u32::MAX` rather than panicking — overflow here is
    /// already unreachable for any source ruff will accept (it caps source
    /// size at 4 GiB), and saturation keeps the parser panic-free even if
    /// that ever changes.
    #[must_use]
    pub fn new(line: u32, column: u32) -> Self {
        Self {
            line: line.saturating_add(1),
            column: column.saturating_add(1),
        }
    }
}

/// Formats the message for a `UnicodeDecodeError` covering the byte range
/// `start..end`: CPython's single-byte form (`byte 0x{first_byte:02x} in
/// position {start}`) when the range is one byte, otherwise the range form
/// (`bytes in position {start}-{end - 1}`).
///
/// A free function (rather than folded into `ExcType::unicode_decode_error`),
/// public and re-exported at the crate root, so `monty-fs` can produce the
/// identical wording when converting a `MountError::InvalidUtf8` from a
/// text-mode file read into an exception.
#[must_use]
pub fn unicode_decode_error_msg(codec: &str, first_byte: u8, start: usize, end: usize, reason: &str) -> String {
    // Callers must pass a non-empty range; checked in debug builds only so a
    // wrong caller can't panic the VM in release (it gets a garbled message
    // position instead, which is harmless).
    debug_assert!(
        end > start,
        "unicode_decode_error_msg: end ({end}) must be > start ({start})"
    );
    if end - start == 1 {
        format!("'{codec}' codec can't decode byte 0x{first_byte:02x} in position {start}: {reason}")
    } else {
        let last = end - 1;
        format!("'{codec}' codec can't decode bytes in position {start}-{last}: {reason}")
    }
}
