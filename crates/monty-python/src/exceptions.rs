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
//! ├── MontyCrashedError        # Raised when a worker process dies or times out
//! └── MontyConversionError     # A host value that can't be converted into the sandbox
//! ```

use std::sync::Arc;

use ::monty::{ExcType, MontyException};
use ahash::AHashMap;
use monty_proto::python::exc_monty_to_py;
use pyo3::{
    PyClassInitializer,
    exceptions::{self},
    prelude::*,
    py_format,
    sync::PyOnceLock,
    types::{PyDict, PyList, PyString},
};

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

/// Raised when a host value cannot be converted across the Monty/host boundary
/// — an `external_lookup` value or an `inputs` value of a type Monty cannot
/// represent. Inherits from `MontyError` (so `except MontyError` catches it) and
/// carries the "Cannot convert X to Monty value" message; the stored type is
/// `TypeError`, so `exception()` reconstructs a native `TypeError`.
#[pyclass(extends=MontyError, module="pydantic_monty")]
pub struct MontyConversionError;

impl MontyConversionError {
    /// Builds a `MontyConversionError` carrying `message`, stored as a
    /// `TypeError`. Raised via [`Self::value_conversion_err`] for a genuine
    /// unrepresentable-type failure.
    #[must_use]
    pub fn new_err(py: Python<'_>, message: String) -> PyErr {
        let base = MontyError::new(MontyException::new(ExcType::TypeError, Some(message)));
        let init = PyClassInitializer::from(base).add_subclass(Self);
        match Py::new(py, init) {
            Ok(err) => PyErr::from_value(err.into_bound(py).into_any()),
            Err(e) => e,
        }
    }

    /// Surfaces a failure from converting a host value into a Monty value
    /// (`py_to_monty_value`). An unrepresentable *type* (`TypeError`, "Cannot
    /// convert X to Monty value") becomes a `MontyConversionError`; any other
    /// exception the converter raises — notably the `RuntimeError` from
    /// exceeding the max input nesting depth — keeps its own type as a
    /// `MontyRuntimeError`, so a depth guard is not mislabeled a conversion
    /// error.
    #[must_use]
    pub fn value_conversion_err(py: Python<'_>, exc: MontyException) -> PyErr {
        if exc.exc_type() == ExcType::TypeError {
            Self::new_err(py, exc.into_message().unwrap_or_default())
        } else {
            MontyError::new_err(py, exc)
        }
    }
}

#[pymethods]
impl MontyConversionError {
    #[expect(clippy::needless_pass_by_value, reason = "required by macro")]
    fn __repr__(slf: PyRef<'_, Self>) -> String {
        format!("MontyConversionError({})", slf.as_super().message().unwrap_or_default())
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
