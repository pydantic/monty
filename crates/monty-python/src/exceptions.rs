//! Custom exception types for the Monty Python interpreter.
//!
//! Provides a hierarchy of exception types that wrap Monty's internal exceptions,
//! preserving traceback information and allowing Python code to distinguish
//! between syntax errors and runtime errors from Monty-executed code.
//!
//! ## Exception Hierarchy
//!
//! ```text
//! MontyError(Exception)        # Base class for all Monty exceptions
//! ├── MontySyntaxError         # Raised when syntax is invalid or Monty can't parse the code
//! └── MontyRuntimeError        # Raised when code fails during execution
//! ```

use ::monty::{ExcType, MontyException, StackFrame};
use pyo3::{
    exceptions,
    prelude::*,
    types::{PyList, PyString},
};

use crate::dataclass::get_frozen_instance_error;

// ============================================================================
// MontyError - Base exception class
// ============================================================================

/// Base exception for all Monty interpreter errors.
///
/// This is the parent class for both `MontySyntaxError` and `MontyRuntimeError`.
/// Catching `MontyError` will catch any exception raised by Monty.
#[pyclass(extends=pyo3::exceptions::PyException, module="monty", subclass)]
#[derive(Clone)]
pub struct MontyError {
    /// The exception type name (e.g., "ValueError", "SyntaxError").
    exc_type_name: String,
    /// The exception message, if any.
    message: Option<String>,
}

impl MontyError {
    /// Creates a new `MontyError` with the given exception type and message.
    #[must_use]
    pub fn new(exc_type_name: String, message: Option<String>) -> Self {
        Self { exc_type_name, message }
    }
}

#[pymethods]
impl MontyError {
    #[new]
    #[pyo3(signature = (exc_type_name, message=None))]
    fn py_new(exc_type_name: String, message: Option<String>) -> Self {
        Self::new(exc_type_name, message)
    }

    /// Returns the inner exception as a Python exception object.
    ///
    /// This recreates a native Python exception (e.g., `ValueError`, `TypeError`)
    /// from the stored exception type and message.
    fn exception(&self, py: Python<'_>) -> Py<PyAny> {
        create_python_exception(py, &self.exc_type_name, self.message.as_deref())
    }

    /// Returns formatted exception string.
    ///
    /// Args:
    ///     show: 'traceback' - full traceback with exception (same as 'type-msg' for base class)
    ///           'type-msg' - 'ExceptionType: message' format
    ///           'msg' - just the message
    #[pyo3(signature = (show = "traceback"))]
    fn display(&self, show: &str) -> PyResult<String> {
        match show {
            "traceback" | "type-msg" => {
                if let Some(msg) = &self.message {
                    Ok(format!("{}: {msg}", self.exc_type_name))
                } else {
                    Ok(self.exc_type_name.clone())
                }
            }
            "msg" => Ok(self.message.clone().unwrap_or_default()),
            _ => Err(pyo3::exceptions::PyValueError::new_err(format!(
                "Invalid display mode: '{show}'. Expected 'traceback', 'type-msg', or 'msg'"
            ))),
        }
    }

    fn __str__(&self) -> String {
        self.message.clone().unwrap_or_default()
    }

    fn __repr__(&self) -> String {
        if let Some(msg) = &self.message {
            format!("MontyError({}: {})", self.exc_type_name, msg)
        } else {
            format!("MontyError({})", self.exc_type_name)
        }
    }
}

// ============================================================================
// MontySyntaxError - For syntax/parse errors
// ============================================================================

/// Raised when Python code has syntax errors or cannot be parsed by Monty.
///
/// Inherits from `MontyError`. The inner exception is always a `SyntaxError`.
#[pyclass(extends=MontyError, module="monty")]
#[derive(Clone)]
pub struct MontySyntaxError;

#[pymethods]
impl MontySyntaxError {
    #[new]
    fn py_new(message: String) -> (Self, MontyError) {
        (Self, MontyError::new("SyntaxError".to_string(), Some(message)))
    }

    fn __repr__(slf: PyRef<'_, Self>) -> String {
        let parent = slf.as_super();
        if let Some(msg) = &parent.message {
            format!("MontySyntaxError({msg})")
        } else {
            "MontySyntaxError()".to_string()
        }
    }
}

// ============================================================================
// MontyRuntimeError - For runtime execution errors
// ============================================================================

/// Raised when Monty code fails during execution.
///
/// Inherits from `MontyError`. Additionally provides `traceback()` to access
/// the Monty stack frames where the error occurred.
#[pyclass(extends=MontyError, module="monty")]
pub struct MontyRuntimeError {
    /// The traceback frames where the error occurred.
    frames: Py<PyList>,
    /// Pre-computed full traceback display string.
    display_str: String,
    /// Pre-computed summary string ("ExcType: message").
    summary: String,
}

impl MontyRuntimeError {
    /// Creates a new `MontyRuntimeError` from the given exception data.
    #[must_use]
    pub fn new(
        exc_type_name: String,
        message: Option<String>,
        frames: Py<PyList>,
        display_str: String,
        summary: String,
    ) -> (Self, MontyError) {
        (
            Self {
                frames,
                display_str,
                summary,
            },
            MontyError::new(exc_type_name, message),
        )
    }
}

#[pymethods]
impl MontyRuntimeError {
    #[new]
    #[pyo3(signature = (exc_type_name, message, frames, display_str, summary))]
    fn py_new(
        exc_type_name: String,
        message: Option<String>,
        frames: Py<PyList>,
        display_str: String,
        summary: String,
    ) -> (Self, MontyError) {
        Self::new(exc_type_name, message, frames, display_str, summary)
    }

    /// Returns the Monty traceback as a list of Frame objects.
    fn traceback(&self, py: Python<'_>) -> Py<PyList> {
        self.frames.clone_ref(py)
    }

    /// Returns formatted exception string.
    ///
    /// Overrides the base class to provide the full traceback when show='traceback'.
    #[pyo3(signature = (show = "traceback"))]
    fn display(slf: PyRef<'_, Self>, show: &str) -> PyResult<String> {
        match show {
            "traceback" => Ok(slf.display_str.clone()),
            "type-msg" => Ok(slf.summary.clone()),
            "msg" => Ok(slf.as_super().message.clone().unwrap_or_default()),
            _ => Err(pyo3::exceptions::PyValueError::new_err(format!(
                "Invalid display mode: '{show}'. Expected 'traceback', 'type-msg', or 'msg'"
            ))),
        }
    }

    fn __str__(slf: PyRef<'_, Self>) -> String {
        slf.as_super().message.clone().unwrap_or_default()
    }

    fn __repr__(slf: PyRef<'_, Self>) -> String {
        let parent = slf.as_super();
        if let Some(msg) = &parent.message {
            format!("MontyRuntimeError({}: {})", parent.exc_type_name, msg)
        } else {
            format!("MontyRuntimeError({})", parent.exc_type_name)
        }
    }
}

// ============================================================================
// PyFrame - A single frame in a Monty traceback
// ============================================================================

/// A single frame in a Monty traceback.
///
/// Contains all the information needed to display a traceback line:
/// the file location, function name, and optional source code preview.
#[pyclass(name = "Frame", module = "monty", frozen)]
#[derive(Debug, Clone)]
pub struct PyFrame {
    /// The filename where the code is located.
    #[pyo3(get)]
    pub filename: String,
    /// Line number (1-based).
    #[pyo3(get)]
    pub line: u16,
    /// Column number (1-based).
    #[pyo3(get)]
    pub column: u16,
    /// End line number (1-based).
    #[pyo3(get)]
    pub end_line: u16,
    /// End column number (1-based).
    #[pyo3(get)]
    pub end_column: u16,
    /// The name of the function, or None for module-level code.
    #[pyo3(get)]
    pub function_name: Option<String>,
    /// The source code line for preview in the traceback.
    #[pyo3(get)]
    pub source_line: Option<String>,
}

#[pymethods]
impl PyFrame {
    fn __repr__(&self) -> String {
        let func = self.function_name.as_ref().map_or("<module>".to_string(), Clone::clone);
        format!(
            "Frame(filename='{}', line={}, column={}, function_name='{}')",
            self.filename, self.line, self.column, func
        )
    }
}

impl PyFrame {
    /// Creates a `PyFrame` from Monty's `StackFrame`.
    #[must_use]
    pub fn from_stack_frame(frame: &StackFrame) -> Self {
        Self {
            filename: frame.filename.clone(),
            line: frame.start.line,
            column: frame.start.column,
            end_line: frame.end.line,
            end_column: frame.end.column,
            function_name: frame.frame_name.clone(),
            source_line: frame.preview_line.clone(),
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Creates a native Python exception from the exception type name and message.
///
/// This is used for both:
/// - The `exception()` method on `MontyError` subclasses
/// - Converting exception *values* back to Python (not raised exceptions)
pub fn create_python_exception(py: Python<'_>, exc_type_name: &str, message: Option<&str>) -> Py<PyAny> {
    let msg = message.unwrap_or_default().to_string();

    let exc = match exc_type_name {
        "Exception" => exceptions::PyException::new_err(msg),
        "BaseException" => exceptions::PyBaseException::new_err(msg),
        "SystemExit" => exceptions::PySystemExit::new_err(msg),
        "KeyboardInterrupt" => exceptions::PyKeyboardInterrupt::new_err(msg),
        "ArithmeticError" => exceptions::PyArithmeticError::new_err(msg),
        "OverflowError" => exceptions::PyOverflowError::new_err(msg),
        "ZeroDivisionError" => exceptions::PyZeroDivisionError::new_err(msg),
        "LookupError" => exceptions::PyLookupError::new_err(msg),
        "IndexError" => exceptions::PyIndexError::new_err(msg),
        "KeyError" => exceptions::PyKeyError::new_err(msg),
        "RuntimeError" => exceptions::PyRuntimeError::new_err(msg),
        "NotImplementedError" => exceptions::PyNotImplementedError::new_err(msg),
        "RecursionError" => exceptions::PyRecursionError::new_err(msg),
        "AssertionError" => exceptions::PyAssertionError::new_err(msg),
        "AttributeError" => exceptions::PyAttributeError::new_err(msg),
        "FrozenInstanceError" => {
            if let Ok(exc_cls) = get_frozen_instance_error(py) {
                if let Ok(exc_instance) = exc_cls.call1((PyString::new(py, &msg),)) {
                    return exc_instance.into_any().unbind();
                }
            }
            // Fallback to AttributeError
            exceptions::PyAttributeError::new_err(msg)
        }
        "MemoryError" => exceptions::PyMemoryError::new_err(msg),
        "NameError" => exceptions::PyNameError::new_err(msg),
        "SyntaxError" => exceptions::PySyntaxError::new_err(msg),
        "TimeoutError" => exceptions::PyTimeoutError::new_err(msg),
        "TypeError" => exceptions::PyTypeError::new_err(msg),
        "ValueError" => exceptions::PyValueError::new_err(msg),
        _ => exceptions::PyException::new_err(msg),
    };

    exc.value(py).clone().into_any().unbind()
}

/// Converts a Monty exception to a `PyErr`.
///
/// For `SyntaxError` exceptions, creates a `MontySyntaxError`.
/// For all other exceptions, creates a `MontyRuntimeError` with all the exception
/// information preserved, including the traceback frames and display string.
pub fn exc_monty_to_py(py: Python<'_>, exc: MontyException) -> PyErr {
    let exc_type = exc.exc_type();

    // Syntax errors get their own exception type
    if exc_type == ExcType::SyntaxError {
        let message = exc.message().map(ToString::to_string).unwrap_or_default();
        let (syntax_error, base_error) = MontySyntaxError::py_new(message);
        let init = pyo3::PyClassInitializer::from(base_error).add_subclass(syntax_error);
        return match Py::new(py, init) {
            Ok(err) => PyErr::from_value(err.into_bound(py).into_any()),
            Err(e) => e,
        };
    }

    // All other errors are runtime errors
    let display_str = exc.to_string();
    let summary = exc.summary();
    let message = exc.message().map(ToString::to_string);
    let exc_type_str: &'static str = exc_type.into();

    // Convert stack frames to PyFrame objects
    let frames: Vec<Py<PyFrame>> = exc
        .traceback()
        .iter()
        .map(|f| Py::new(py, PyFrame::from_stack_frame(f)).expect("failed to create PyFrame"))
        .collect();

    // Create the list of frames
    let frames_list = PyList::new(py, &frames).expect("failed to create frames list").unbind();

    // Create the MontyRuntimeError with proper initialization
    let (runtime_error, base_error) =
        MontyRuntimeError::new(exc_type_str.to_string(), message, frames_list, display_str, summary);

    let init = pyo3::PyClassInitializer::from(base_error).add_subclass(runtime_error);
    match Py::new(py, init) {
        Ok(err) => PyErr::from_value(err.into_bound(py).into_any()),
        Err(e) => e,
    }
}

/// Converts a python exception to monty.
///
/// Used when resuming execution with an exception from Python.
pub fn exc_py_to_monty(py: Python<'_>, py_err: PyErr) -> MontyException {
    let exc = py_err.value(py);
    let exc_type = py_err_to_exc_type(exc);
    let arg = exc.str().ok().map(|s| s.to_string_lossy().into_owned());

    MontyException::new(exc_type, arg)
}

/// Converts a Python exception to Monty's `MontyObject::Exception`.
pub fn exc_to_monty_object(exc: &Bound<'_, exceptions::PyBaseException>) -> ::monty::MontyObject {
    let exc_type = py_err_to_exc_type(exc);
    let arg = exc.str().ok().map(|s| s.to_string_lossy().into_owned());

    ::monty::MontyObject::Exception { exc_type, arg }
}

/// Maps a Python exception type to Monty's `ExcType` enum.
///
/// NOTE: order matters here as some exceptions are subclasses of others!
/// In general we group exceptions by their type hierarchy to improve performance.
fn py_err_to_exc_type(exc: &Bound<'_, exceptions::PyBaseException>) -> ExcType {
    use pyo3::PyTypeCheck;

    // Exception hierarchy
    if exceptions::PyException::type_check(exc) {
        // put the most commonly used exceptions first
        if exceptions::PyTypeError::type_check(exc) {
            ExcType::TypeError
        } else if exceptions::PyValueError::type_check(exc) {
            ExcType::ValueError
        } else if exceptions::PyAssertionError::type_check(exc) {
            ExcType::AssertionError
        } else if exceptions::PySyntaxError::type_check(exc) {
            ExcType::SyntaxError
        // LookupError hierarchy
        } else if exceptions::PyLookupError::type_check(exc) {
            if exceptions::PyKeyError::type_check(exc) {
                ExcType::KeyError
            } else if exceptions::PyIndexError::type_check(exc) {
                ExcType::IndexError
            } else {
                ExcType::LookupError
            }
        // ArithmeticError hierarchy
        } else if exceptions::PyArithmeticError::type_check(exc) {
            if exceptions::PyZeroDivisionError::type_check(exc) {
                ExcType::ZeroDivisionError
            } else if exceptions::PyOverflowError::type_check(exc) {
                ExcType::OverflowError
            } else {
                ExcType::ArithmeticError
            }
        // RuntimeError hierarchy
        } else if exceptions::PyRuntimeError::type_check(exc) {
            if exceptions::PyNotImplementedError::type_check(exc) {
                ExcType::NotImplementedError
            } else if exceptions::PyRecursionError::type_check(exc) {
                ExcType::RecursionError
            } else {
                ExcType::RuntimeError
            }
        // AttributeError hierarchy
        } else if exceptions::PyAttributeError::type_check(exc) {
            if is_frozen_instance_error(exc) {
                ExcType::FrozenInstanceError
            } else {
                ExcType::AttributeError
            }
        // other standalone exception types
        } else if exceptions::PyNameError::type_check(exc) {
            ExcType::NameError
        } else if exceptions::PyTimeoutError::type_check(exc) {
            ExcType::TimeoutError
        } else if exceptions::PyMemoryError::type_check(exc) {
            ExcType::MemoryError
        } else {
            ExcType::Exception
        }
    // BaseException direct subclasses
    } else if exceptions::PySystemExit::type_check(exc) {
        ExcType::SystemExit
    } else if exceptions::PyKeyboardInterrupt::type_check(exc) {
        ExcType::KeyboardInterrupt
    // Catch-all for BaseException
    } else {
        ExcType::BaseException
    }
}

/// Checks if an exception is an instance of `dataclasses.FrozenInstanceError`.
///
/// Since `FrozenInstanceError` is not a built-in PyO3 exception type, we need to
/// check using Python's isinstance against the imported class.
fn is_frozen_instance_error(exc: &Bound<'_, exceptions::PyBaseException>) -> bool {
    if let Ok(frozen_error_cls) = get_frozen_instance_error(exc.py()) {
        exc.is_instance(frozen_error_cls).unwrap_or(false)
    } else {
        false
    }
}
