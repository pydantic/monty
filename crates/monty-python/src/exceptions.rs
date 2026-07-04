//! Custom exception types for the Monty Python interpreter.
//!
//! Provides a hierarchy of exception types that wrap Monty's internal exceptions,
//! preserving traceback information and allowing Python code to distinguish
//! between syntax errors, runtime errors, and type checking errors from Monty-executed code.
//!
//! ## Exception Hierarchy
//!
//! ```text
//! MontyError(Exception)        # Base class for all Monty exceptions
//! ├── MontySyntaxError         # Raised when syntax is invalid or Monty can't parse the code
//! ├── MontyRuntimeError        # Raised when code fails during execution
//! ├── MontyTypingError         # Raised when type checking finds errors in the code
//! └── MontyCrashedError        # Raised when a worker process dies or times out
//! ```

use std::sync::Arc;

use ::monty::{ExcData, ExcType, MontyException, UnicodeErrorObject};
use ahash::AHashMap;
use pyo3::{
    PyClassInitializer, PyTypeCheck,
    exceptions::{self},
    prelude::*,
    py_format,
    sync::PyOnceLock,
    types::{PyBytes, PyDict, PyList, PyString},
};

use crate::dataclass::get_frozen_instance_error;

/// Base exception for all Monty interpreter errors; catching it catches any
/// exception raised by Monty.
#[pyclass(extends=exceptions::PyException, module="pydantic_monty", subclass, skip_from_py_object)]
#[derive(Clone)]
pub struct MontyError {
    /// The underlying Monty exception.
    exc: MontyException,
}

impl MontyError {
    /// Converts a Monty exception to a `PyErr`: `MontySyntaxError` for syntax
    /// errors, `MontyRuntimeError` (preserving traceback frames) otherwise.
    #[must_use]
    pub fn new_err(py: Python<'_>, exc: MontyException) -> PyErr {
        if exc.exc_type() == ExcType::SyntaxError {
            MontySyntaxError::new_err(py, exc)
        } else {
            MontyRuntimeError::new_err(py, exc)
        }
    }
}

impl MontyError {
    /// Creates a new `MontyError` wrapping a `MontyException`.
    #[must_use]
    pub fn new(exc: MontyException) -> Self {
        Self { exc }
    }

    /// Returns the exception type.
    fn exc_type(&self) -> ExcType {
        self.exc.exc_type()
    }

    /// Returns the exception message, if any.
    fn message(&self) -> Option<&str> {
        self.exc.message()
    }
}

#[pymethods]
impl MontyError {
    /// Recreates the inner exception as a native Python exception object (e.g.
    /// `ValueError`, `TypeError`) from the stored type and message.
    fn exception(&self, py: Python<'_>) -> Py<PyAny> {
        let py_err = exc_monty_to_py(py, self.exc.clone());
        py_err.into_value(py).into_any()
    }

    fn __str__(&self) -> String {
        self.message().unwrap_or_default().to_string()
    }

    fn __repr__(&self) -> String {
        let exc_type_name = self.exc_type();
        if let Some(msg) = self.message() {
            format!("MontyError({exc_type_name}: {msg})")
        } else {
            format!("MontyError({exc_type_name})")
        }
    }
}

/// Raised when type checking rejects a fed snippet.
///
/// Inherits from `MontyError`. Type checking runs inside the worker
/// subprocess; the diagnostics arrive pre-rendered as text (the structured
/// diagnostics cannot cross the process boundary).
#[pyclass(extends=MontyError, module="pydantic_monty")]
pub struct MontyTypingError {
    rendered: String,
}

impl MontyTypingError {
    /// Creates a `MontyTypingError` from diagnostics rendered in the worker.
    #[must_use]
    pub fn new_err(py: Python<'_>, rendered: String) -> PyErr {
        // we need a MontyException to create the base, but it shouldn't be visible anywhere
        let base = MontyError::new(MontyException::new(ExcType::TypeError, None));
        let init = PyClassInitializer::from(base).add_subclass(Self { rendered });
        match Py::new(py, init) {
            Ok(err) => PyErr::from_value(err.into_bound(py).into_any()),
            Err(e) => e,
        }
    }
}

#[pymethods]
impl MontyTypingError {
    /// Returns the rendered type-check diagnostics.
    fn display(&self) -> &str {
        &self.rendered
    }

    fn __str__(&self) -> String {
        self.rendered.clone()
    }

    fn __repr__(&self) -> String {
        format!("MontyTypingError({})", self.rendered)
    }
}

/// Raised when Python code has syntax errors or cannot be parsed by Monty.
///
/// Inherits from `MontyError`. The inner exception is always a `SyntaxError`.
///
/// As with [`MontyRuntimeError`], the traceback `PyFrame` list is materialized
/// lazily on the first `traceback()` call and cached for subsequent calls.
#[pyclass(extends=MontyError, module="pydantic_monty", skip_from_py_object)]
pub struct MontySyntaxError {
    traceback: PyOnceLock<Py<PyList>>,
}

impl MontySyntaxError {
    /// Creates a new `MontySyntaxError` with the given message.
    #[must_use]
    pub fn new_err(py: Python<'_>, exc: MontyException) -> PyErr {
        let base_error = MontyError::new(exc);
        let init = PyClassInitializer::from(base_error).add_subclass(Self {
            traceback: PyOnceLock::new(),
        });
        match Py::new(py, init) {
            Ok(err) => PyErr::from_value(err.into_bound(py).into_any()),
            Err(e) => e,
        }
    }
}

#[pymethods]
impl MontySyntaxError {
    /// Returns the Monty traceback as a list of Frame objects.
    ///
    /// Built on the first call and cached, so repeated calls return the same
    /// list and frame objects. See [`build_traceback_list`] for the
    /// source-line dedup details.
    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn traceback(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = slf
            .traceback
            .get_or_try_init(py, || build_traceback_list(py, &slf.as_super().exc))?;
        Ok(list.clone_ref(py))
    }

    /// Returns formatted exception string.
    #[pyo3(signature = (format = "traceback"))]
    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn display(slf: PyRef<'_, Self>, format: &str) -> PyResult<String> {
        match format {
            "traceback" => Ok(slf.as_super().exc.to_string()),
            "type-msg" => Ok(slf.as_super().exc.summary()),
            "msg" => Ok(slf.as_super().message().unwrap_or_default().to_string()),
            _ => Err(exceptions::PyValueError::new_err(format!(
                "Invalid display format: '{format}'. Expected 'traceback', 'type-msg', or 'msg'"
            ))),
        }
    }

    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn __str__(slf: PyRef<'_, Self>) -> String {
        slf.as_super().message().unwrap_or_default().to_string()
    }

    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn __repr__(slf: PyRef<'_, Self>) -> String {
        let parent = slf.as_super();
        if let Some(msg) = parent.message() {
            format!("MontySyntaxError({msg})")
        } else {
            "MontySyntaxError()".to_string()
        }
    }
}

/// Raised when Monty code fails during execution. Inherits from `MontyError`
/// and provides `traceback()` for the Monty stack frames.
///
/// `PyFrame` objects are materialized lazily on the first `traceback()` call
/// (not at construction), bounding exception-propagation cost: deeply recursive
/// code referencing a long line can't force the embedder to allocate
/// `O(depth × line_len)` bytes just by triggering the exception. The result is
/// cached, matching the stable-object semantics of CPython's `exc.__traceback__`.
#[pyclass(extends=MontyError, module="pydantic_monty")]
pub struct MontyRuntimeError {
    traceback: PyOnceLock<Py<PyList>>,
}

impl MontyRuntimeError {
    /// Creates a `MontyRuntimeError` from the given exception. O(1) — the
    /// `MontyException` is stored on the base; frames are built on demand by
    /// `traceback()`.
    #[must_use]
    pub fn new_err(py: Python<'_>, exc: MontyException) -> PyErr {
        let base_error = MontyError::new(exc);
        let init = PyClassInitializer::from(base_error).add_subclass(Self {
            traceback: PyOnceLock::new(),
        });
        match Py::new(py, init) {
            Ok(err) => PyErr::from_value(err.into_bound(py).into_any()),
            Err(e) => e,
        }
    }
}

#[pymethods]
impl MontyRuntimeError {
    /// Returns the Monty traceback as a list of Frame objects.
    ///
    /// Built on the first call and cached, so repeated calls return the same
    /// list and frame objects. See [`build_traceback_list`] for the
    /// source-line dedup details.
    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn traceback(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = slf
            .traceback
            .get_or_try_init(py, || build_traceback_list(py, &slf.as_super().exc))?;
        Ok(list.clone_ref(py))
    }

    /// Returns formatted exception string.
    #[pyo3(signature = (format = "traceback"))]
    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn display(slf: PyRef<'_, Self>, format: &str) -> PyResult<String> {
        match format {
            "traceback" => Ok(slf.as_super().exc.to_string()),
            "type-msg" => Ok(slf.as_super().exc.summary()),
            "msg" => Ok(slf.as_super().message().unwrap_or_default().to_string()),
            _ => Err(exceptions::PyValueError::new_err(format!(
                "Invalid display format: '{format}'. Expected 'traceback', 'type-msg', or 'msg'"
            ))),
        }
    }

    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn __str__(slf: PyRef<'_, Self>) -> String {
        let parent = slf.as_super();
        let exc_type_name = parent.exc_type();
        if let Some(msg) = parent.message()
            && !msg.is_empty()
        {
            return format!("{exc_type_name}: {msg}");
        }
        format!("{exc_type_name}")
    }

    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn __repr__(slf: PyRef<'_, Self>) -> String {
        let parent = slf.as_super();
        let exc_type_name = parent.exc_type();
        if let Some(msg) = parent.message()
            && !msg.is_empty()
        {
            return format!("MontyRuntimeError({exc_type_name}: {msg})");
        }
        format!("MontyRuntimeError({exc_type_name})")
    }
}

/// Raised when a worker process died (segfault, abort, external kill) or
/// was killed by the pool's request-timeout watchdog.
///
/// This is exactly the failure mode subprocess pools exist to contain: the
/// sandbox process is gone, but the host process is unharmed and the pool
/// replaces the worker — catch this error and retry or report.
#[pyclass(extends=MontyError, module="pydantic_monty")]
pub struct MontyCrashedError {
    /// `True` when the pool's `request_timeout` watchdog killed the worker.
    #[pyo3(get)]
    timed_out: bool,
    /// Exit code of the dead worker, when the OS reported one (signal deaths
    /// on unix report `None`).
    #[pyo3(get)]
    exit_status: Option<i32>,
}

impl MontyCrashedError {
    /// Creates a `MontyCrashedError` with the given description.
    #[must_use]
    pub fn new_err(py: Python<'_>, message: String, timed_out: bool, exit_status: Option<i32>) -> PyErr {
        let base = MontyError::new(MontyException::new(ExcType::RuntimeError, Some(message)));
        let init = PyClassInitializer::from(base).add_subclass(Self { timed_out, exit_status });
        match Py::new(py, init) {
            Ok(err) => PyErr::from_value(err.into_bound(py).into_any()),
            Err(e) => e,
        }
    }
}

#[pymethods]
impl MontyCrashedError {
    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn __str__(slf: PyRef<'_, Self>) -> String {
        slf.as_super().message().unwrap_or_default().to_owned()
    }

    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn __repr__(slf: PyRef<'_, Self>) -> String {
        format!("MontyCrashedError({})", slf.as_super().message().unwrap_or_default())
    }
}

/// Builds the `PyList` of `PyFrame` objects for a `MontyException`'s traceback.
///
/// `Frame.source_line` is backed by a `Py<PyString>` that is deduplicated
/// across frames resolving to the same underlying `Arc<str>` preview line.
/// For deep recursion where every frame points at the same line, this
/// allocates one `PyString` instead of one per frame.
fn build_traceback_list(py: Python<'_>, exc: &MontyException) -> PyResult<Py<PyList>> {
    let mut line_cache: AHashMap<usize, Py<PyString>> = AHashMap::new();
    let frames: Vec<Py<PyFrame>> = exc
        .traceback()
        .iter()
        .map(|f| {
            let source_line = f.preview_line.as_ref().map(|arc| {
                let key = Arc::as_ptr(arc).cast::<()>() as usize;
                line_cache
                    .entry(key)
                    .or_insert_with(|| PyString::new(py, arc).unbind())
                    .clone_ref(py)
            });
            Py::new(
                py,
                PyFrame {
                    filename: f.filename.clone(),
                    line: f.start.line,
                    column: f.start.column,
                    end_line: f.end.line,
                    end_column: f.end.column,
                    function_name: f.frame_name.clone(),
                    source_line,
                },
            )
        })
        .collect::<PyResult<_>>()?;
    Ok(PyList::new(py, &frames)?.unbind())
}

/// A single frame in a Monty traceback: file location, function name, and an
/// optional source preview.
///
/// `source_line` is a `Py<PyString>` so frames sharing one underlying source
/// line in a single `traceback()` call share one Python string object, turning
/// `O(depth × line_len)` peak memory into a single allocation for deep recursion.
#[pyclass(name = "Frame", module = "pydantic_monty", frozen, skip_from_py_object)]
#[derive(Debug)]
pub struct PyFrame {
    /// The filename where the code is located.
    #[pyo3(get)]
    pub filename: String,
    /// Line number (1-based).
    #[pyo3(get)]
    pub line: u32,
    /// Column number (1-based).
    #[pyo3(get)]
    pub column: u32,
    /// End line number (1-based).
    #[pyo3(get)]
    pub end_line: u32,
    /// End column number (1-based).
    #[pyo3(get)]
    pub end_column: u32,
    /// The name of the function, or None for module-level code.
    #[pyo3(get)]
    pub function_name: Option<String>,
    /// The source code line for preview in the traceback.
    #[pyo3(get)]
    pub source_line: Option<Py<PyString>>,
}

#[pymethods]
impl PyFrame {
    fn dict<'py>(&self, py: Python<'py>) -> Bound<'py, PyDict> {
        let dict = PyDict::new(py);
        dict.set_item("filename", &self.filename).unwrap();
        dict.set_item("line", self.line).unwrap();
        dict.set_item("column", self.column).unwrap();
        dict.set_item("end_line", self.end_line).unwrap();
        dict.set_item("end_column", self.end_column).unwrap();
        dict.set_item("function_name", self.function_name.as_ref()).unwrap();

        dict.set_item("source_line", self.source_line.as_ref()).unwrap();
        dict
    }

    fn __repr__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyString>> {
        let func = self.function_name.as_deref().unwrap_or("<module>");
        py_format!(
            py,
            "Frame(filename='{}', line={}, column={}, function_name='{}')",
            self.filename,
            self.line,
            self.column,
            func
        )
    }
}

/// Converts Monty's `MontyException` to the matching Python exception value.
/// Traceback info is folded into the message, since PyO3 doesn't expose direct
/// traceback manipulation.
#[must_use]
pub fn exc_monty_to_py(py: Python<'_>, mut exc: MontyException) -> PyErr {
    let exc_type = exc.exc_type();
    let exc_data = exc.take_data();
    let msg = exc.into_message().unwrap_or_default();

    match exc_type {
        ExcType::Exception => exceptions::PyException::new_err(msg),
        ExcType::BaseException => exceptions::PyBaseException::new_err(msg),
        ExcType::SystemExit => exceptions::PySystemExit::new_err(msg),
        ExcType::KeyboardInterrupt => exceptions::PyKeyboardInterrupt::new_err(msg),
        ExcType::ArithmeticError => exceptions::PyArithmeticError::new_err(msg),
        ExcType::OverflowError => exceptions::PyOverflowError::new_err(msg),
        ExcType::ZeroDivisionError => exceptions::PyZeroDivisionError::new_err(msg),
        ExcType::LookupError => exceptions::PyLookupError::new_err(msg),
        ExcType::IndexError => exceptions::PyIndexError::new_err(msg),
        ExcType::KeyError => exceptions::PyKeyError::new_err(msg),
        ExcType::RuntimeError => exceptions::PyRuntimeError::new_err(msg),
        ExcType::NotImplementedError => exceptions::PyNotImplementedError::new_err(msg),
        ExcType::RecursionError => exceptions::PyRecursionError::new_err(msg),
        ExcType::AssertionError => exceptions::PyAssertionError::new_err(msg),
        ExcType::AttributeError => exceptions::PyAttributeError::new_err(msg),
        ExcType::FrozenInstanceError => {
            if let Ok(exc_cls) = get_frozen_instance_error(py)
                && let Ok(exc_instance) = exc_cls.call1((PyString::new(py, &msg),))
            {
                return PyErr::from_value(exc_instance);
            }
            // if creating the right exception fails, fallback to AttributeError which it's a subclass of
            exceptions::PyAttributeError::new_err(msg)
        }
        ExcType::MemoryError => exceptions::PyMemoryError::new_err(msg),
        ExcType::NameError => exceptions::PyNameError::new_err(msg),
        ExcType::UnboundLocalError => exceptions::PyUnboundLocalError::new_err(msg),
        ExcType::StopIteration => exceptions::PyStopIteration::new_err(msg),
        ExcType::SyntaxError => exceptions::PySyntaxError::new_err(msg),
        ExcType::TimeoutError => exceptions::PyTimeoutError::new_err(msg),
        ExcType::TypeError => exceptions::PyTypeError::new_err(msg),
        ExcType::ValueError => exceptions::PyValueError::new_err(msg),
        ExcType::UnicodeDecodeError | ExcType::UnicodeEncodeError => unicode_error_to_py(py, exc_type, exc_data, msg),
        ExcType::JsonDecodeError => {
            if let Ok(json_decode_error) = get_json_decode_error(py)
                && let Ok(exc_instance) = json_decode_error.call1((PyString::new(py, &msg),))
            {
                PyErr::from_value(exc_instance)
            } else {
                exceptions::PyValueError::new_err(msg)
            }
        }
        ExcType::ImportError => exceptions::PyImportError::new_err(msg),
        ExcType::ModuleNotFoundError => exceptions::PyModuleNotFoundError::new_err(msg),
        ExcType::OSError => exceptions::PyOSError::new_err(msg),
        ExcType::FileNotFoundError => exceptions::PyFileNotFoundError::new_err(msg),
        ExcType::FileExistsError => exceptions::PyFileExistsError::new_err(msg),
        ExcType::IsADirectoryError => exceptions::PyIsADirectoryError::new_err(msg),
        ExcType::NotADirectoryError => exceptions::PyNotADirectoryError::new_err(msg),
        ExcType::PermissionError => exceptions::PyPermissionError::new_err(msg),
        ExcType::UnsupportedOperation => {
            if let Ok(exc_cls) = get_unsupported_operation(py)
                && let Ok(exc_instance) = exc_cls.call1((PyString::new(py, &msg),))
            {
                PyErr::from_value(exc_instance)
            } else {
                // Fall back to OSError — the parent we model in `is_subclass_of`.
                exceptions::PyOSError::new_err(msg)
            }
        }
        ExcType::RePatternError => {
            if let Ok(re_pattern_error) = get_re_pattern_error(py)
                && let Ok(exc_instance) = re_pattern_error.call1((PyString::new(py, &msg),))
            {
                PyErr::from_value(exc_instance)
            } else {
                exceptions::PyRuntimeError::new_err(msg)
            }
        }
    }
}

/// Builds a real `UnicodeDecodeError` / `UnicodeEncodeError` from the
/// structured fields Monty attaches to codec errors, calling CPython's
/// five-argument constructor (`encoding, object, start, end, reason`).
///
/// Falls back to a plain `ValueError` carrying the formatted message when the
/// payload is absent — an exception raised manually inside the sandbox
/// (`raise UnicodeDecodeError('msg')`), or a codec error on an object larger
/// than `UnicodeErrorData::MAX_OBJECT_LEN` — or when construction fails
/// (e.g. a decode payload carrying a `str` object). `except ValueError:`
/// catches both forms; only `isinstance` and the attributes differ.
fn unicode_error_to_py(py: Python<'_>, exc_type: ExcType, exc_data: ExcData, msg: String) -> PyErr {
    if let ExcData::Unicode(data) = exc_data {
        let exc_cls = if exc_type == ExcType::UnicodeDecodeError {
            py.get_type::<exceptions::PyUnicodeDecodeError>()
        } else {
            py.get_type::<exceptions::PyUnicodeEncodeError>()
        };
        let object = match &data.object {
            UnicodeErrorObject::Bytes(bytes) => PyBytes::new(py, bytes).into_any(),
            UnicodeErrorObject::Str(s) => PyString::new(py, s).into_any(),
        };
        if let Ok(exc_instance) = exc_cls.call1((data.encoding, object, data.start, data.end, data.reason)) {
            return PyErr::from_value(exc_instance);
        }
    }
    exceptions::PyValueError::new_err(msg)
}

/// Converts a python exception to monty.
///
/// Used when resuming execution with an exception from Python.
pub fn exc_py_to_monty(py: Python<'_>, py_err: &PyErr) -> MontyException {
    let exc = py_err.value(py);
    let exc_type = py_err_to_exc_type(exc);
    let arg = exc.str().ok().map(|s| s.to_string_lossy().into_owned());

    MontyException::new(exc_type, arg)
}

/// Converts a Python exception to Monty's `MontyObject::Exception`.
#[must_use]
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
    // Exception hierarchy
    if exceptions::PyException::type_check(exc) {
        // put the most commonly used exceptions first
        if exceptions::PyTypeError::type_check(exc) {
            ExcType::TypeError
        // ValueError hierarchy (check UnicodeDecodeError/UnicodeEncodeError first as they're subclasses)
        } else if exceptions::PyValueError::type_check(exc) {
            if is_json_decode_error(exc) {
                ExcType::JsonDecodeError
            } else if exceptions::PyUnicodeDecodeError::type_check(exc) {
                ExcType::UnicodeDecodeError
            } else if exceptions::PyUnicodeEncodeError::type_check(exc) {
                ExcType::UnicodeEncodeError
            } else if is_unsupported_operation(exc) {
                // `io.UnsupportedOperation` inherits from both `OSError` and `ValueError`
                ExcType::UnsupportedOperation
            } else {
                ExcType::ValueError
            }
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
        // NameError hierarchy (check UnboundLocalError first as it's a subclass)
        } else if exceptions::PyNameError::type_check(exc) {
            if exceptions::PyUnboundLocalError::type_check(exc) {
                ExcType::UnboundLocalError
            } else {
                ExcType::NameError
            }
        // `io.UnsupportedOperation` inherits from `OSError` but is covered above
        } else if exceptions::PyOSError::type_check(exc) {
            if exceptions::PyFileNotFoundError::type_check(exc) {
                ExcType::FileNotFoundError
            } else if exceptions::PyFileExistsError::type_check(exc) {
                ExcType::FileExistsError
            } else if exceptions::PyIsADirectoryError::type_check(exc) {
                ExcType::IsADirectoryError
            } else if exceptions::PyNotADirectoryError::type_check(exc) {
                ExcType::NotADirectoryError
            } else if exceptions::PyPermissionError::type_check(exc) {
                ExcType::PermissionError
            } else {
                ExcType::OSError
            }
        // other standalone exception types
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

/// Checks if an exception is a `dataclasses.FrozenInstanceError` (not a built-in
/// PyO3 type, so this isinstance-checks against the imported class).
fn is_frozen_instance_error(exc: &Bound<'_, exceptions::PyBaseException>) -> bool {
    if let Ok(frozen_error_cls) = get_frozen_instance_error(exc.py()) {
        exc.is_instance(frozen_error_cls).unwrap_or(false)
    } else {
        false
    }
}

/// Checks if an exception is a `json.JSONDecodeError` (a stdlib class, not a
/// PyO3 built-in, so looked up lazily and cached).
fn is_json_decode_error(exc: &Bound<'_, exceptions::PyBaseException>) -> bool {
    if let Ok(json_decode_error_cls) = get_json_decode_error(exc.py()) {
        exc.is_instance(json_decode_error_cls).unwrap_or(false)
    } else {
        false
    }
}

fn get_re_pattern_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static RE_PATTERN_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    if cfg!(Py_3_13) {
        RE_PATTERN_ERROR.import(py, "re", "PatternError")
    } else {
        RE_PATTERN_ERROR.import(py, "re", "error")
    }
}

/// Returns the cached `json.JSONDecodeError` class.
///
/// This avoids repeated imports while still using the stdlib-defined subclass
/// of `ValueError` rather than fabricating a plain `ValueError`.
fn get_json_decode_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static JSON_DECODE_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    JSON_DECODE_ERROR.import(py, "json", "JSONDecodeError")
}

/// Returns the cached `io.UnsupportedOperation` class.
///
/// Lives in Python's standard library (not in PyO3's built-in wrappers) and
/// is a subclass of both `OSError` and `ValueError` in CPython. Monty raises
/// the real CPython class here so user code can `isinstance(e,
/// io.UnsupportedOperation)`; both parents are modelled by
/// [`ExcType::is_subclass_of`], so `except OSError:` and `except ValueError:`
/// catch it just like in CPython.
fn get_unsupported_operation(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static UNSUPPORTED_OPERATION: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    UNSUPPORTED_OPERATION.import(py, "io", "UnsupportedOperation")
}

/// Checks if an exception is an instance of `io.UnsupportedOperation`.
fn is_unsupported_operation(exc: &Bound<'_, exceptions::PyBaseException>) -> bool {
    if let Ok(cls) = get_unsupported_operation(exc.py()) {
        exc.is_instance(cls).unwrap_or(false)
    } else {
        false
    }
}
