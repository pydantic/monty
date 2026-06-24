//! Python dataclass-style types mirroring the wire protocol's `ParentRequest`
//! and `ChildEvent` oneof arms (see `proto/monty/v1/monty.proto`), plus the
//! payload types they carry.
//!
//! Each class is frozen and value-comparable (`#[pyclass(frozen, eq)]`). Value
//! payloads (`args`, `inputs`, `Complete.value`, ...) are stored as
//! [`MontyObject`] and converted to/from native Python objects at the
//! boundary: construction (the encode side) validates eagerly so a bad value
//! raises at `StartSession(...)`/`Feed(...)` time, and getters convert back lazily.
//!
//! The two envelope unions are exposed to Python as the type aliases
//! `ParentRequest` and `ChildEvent`; [`crate::wire`] turns these objects into
//! `pb::ParentRequest` / `pb::ChildEvent` and back.

use std::fmt::Display;

use monty::{CodeLoc, ExcType, MontyException, MontyObject, StackFrame};
use monty_proto::{WireObject, pb};
use pb::ext_function_result::Kind;
use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
    types::{PyDict, PyList, PyMapping, PyString, PyType},
};

use crate::{
    convert::{monty_to_py, py_to_monty_value},
    dataclass::DcRegistry,
    exceptions::{exc_monty_to_py, exc_py_to_monty},
};

// =============================================================================
// Value conversion helpers
//
// Each helper spins up a fresh `DcRegistry`: the codec is stateless, so
// dataclasses round-trip by value (a decoded dataclass becomes an
// `UnknownDataclass` unless the caller wires its own registry in a future
// revision). The host-side `type_id` is preserved on the wire but is only
// meaningful within the process that produced it.
// =============================================================================

/// Converts one native Python object into a [`MontyObject`], raising the same
/// `TypeError` the in-process API would for unsupported types.
pub(crate) fn to_monty(obj: &Bound<'_, PyAny>) -> PyResult<MontyObject> {
    let registry = DcRegistry::new(obj.py());
    py_to_monty_value(obj, &registry).map_err(|e| exc_monty_to_py(obj.py(), e))
}

/// Converts a [`MontyObject`] back into a native Python object.
pub(crate) fn from_monty(py: Python<'_>, obj: &MontyObject) -> PyResult<Py<PyAny>> {
    let registry = DcRegistry::new(py);
    monty_to_py(py, obj, &registry)
}

/// Converts a Python iterable of values into a `Vec<MontyObject>`.
pub(crate) fn to_monty_seq(seq: &Bound<'_, PyAny>) -> PyResult<Vec<MontyObject>> {
    seq.try_iter()?.map(|item| to_monty(&item?)).collect()
}

/// Converts a `&[MontyObject]` into a Python `list`.
pub(crate) fn from_monty_seq(py: Python<'_>, items: &[MontyObject]) -> PyResult<Py<PyList>> {
    let objs: Vec<Py<PyAny>> = items.iter().map(|obj| from_monty(py, obj)).collect::<PyResult<_>>()?;
    Ok(PyList::new(py, objs)?.unbind())
}

/// Converts a Python `dict` (or any mapping) of `str -> value` keyword
/// arguments into ordered `(key, value)` `MontyObject` pairs. Non-string keys
/// are rejected — kwargs names are always strings.
pub(crate) fn to_monty_kwargs(kwargs: &Bound<'_, PyAny>) -> PyResult<Vec<(MontyObject, MontyObject)>> {
    let mapping = kwargs.cast::<PyMapping>()?;
    mapping
        .items()?
        .try_iter()?
        .map(|item| {
            let pair = item?;
            let (key, value): (Bound<'_, PyAny>, Bound<'_, PyAny>) = pair.extract()?;
            let key = key
                .cast::<PyString>()
                .map_err(|_| PyTypeError::new_err("keyword argument names must be strings"))?;
            Ok((MontyObject::String(key.extract()?), to_monty(&value)?))
        })
        .collect()
}

/// Converts `(key, value)` `MontyObject` pairs back into a Python `dict`,
/// rendering keys with their natural Python representation.
pub(crate) fn from_monty_kwargs(py: Python<'_>, pairs: &[(MontyObject, MontyObject)]) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    for (key, value) in pairs {
        dict.set_item(from_monty(py, key)?, from_monty(py, value)?)?;
    }
    Ok(dict.unbind())
}

// =============================================================================
// Mounts
// =============================================================================

/// A host directory to expose into the sandbox for one [`Feed`].
///
/// Unlike `pydantic_monty.MountDir`, this is *pure data*: it performs no
/// filesystem validation, because `host_path` names a directory on the
/// *server* that runs the sandbox, which the client building the request
/// cannot see. The server is responsible for validating and constraining
/// mounts it accepts from a client.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Mount {
    #[pyo3(get)]
    pub(crate) virtual_path: String,
    #[pyo3(get)]
    pub(crate) host_path: String,
    /// One of `"read-only"`, `"read-write"`, `"overlay"`.
    #[pyo3(get)]
    pub(crate) mode: String,
    #[pyo3(get)]
    pub(crate) write_bytes_limit: Option<u64>,
}

#[pymethods]
impl Mount {
    #[new]
    #[pyo3(signature = (virtual_path, host_path, *, mode = "overlay", write_bytes_limit = None))]
    fn new(virtual_path: String, host_path: String, mode: &str, write_bytes_limit: Option<u64>) -> PyResult<Self> {
        // Validate the mode string up front so a typo fails at construction
        // rather than as an opaque "no mode" error inside the server.
        mount_mode_to_proto(mode)?;
        Ok(Self {
            virtual_path,
            host_path,
            mode: mode.to_owned(),
            write_bytes_limit,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "Mount(virtual_path={:?}, host_path={:?}, mode={:?})",
            self.virtual_path, self.host_path, self.mode
        )
    }
}

/// Maps a mode string onto the proto `MountMode` enum value (1/2/3).
pub(crate) fn mount_mode_to_proto(mode: &str) -> PyResult<i32> {
    match mode {
        "read-only" => Ok(pb::MountMode::ReadOnly as i32),
        "read-write" => Ok(pb::MountMode::ReadWrite as i32),
        "overlay" => Ok(pb::MountMode::Overlay as i32),
        other => Err(PyValueError::new_err(format!(
            "invalid mount mode '{other}'; expected 'read-only', 'read-write', or 'overlay'"
        ))),
    }
}

/// Maps a proto `MountMode` enum value back onto a mode string.
pub(crate) fn mount_mode_from_proto(mode: i32) -> PyResult<&'static str> {
    match pb::MountMode::try_from(mode) {
        Ok(pb::MountMode::ReadOnly) => Ok("read-only"),
        Ok(pb::MountMode::ReadWrite) => Ok("read-write"),
        Ok(pb::MountMode::Overlay) => Ok("overlay"),
        _ => Err(PyValueError::new_err(format!(
            "mount has unspecified or unknown mode {mode}"
        ))),
    }
}

// =============================================================================
// Exceptions
// =============================================================================

/// One frame of a [`RaisedException`] traceback. Mirrors `monty.StackFrame`,
/// with the start/end `CodeLoc`s flattened into line/column pairs.
#[pyclass(name = "StackFrame", frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct WireStackFrame {
    #[pyo3(get)]
    pub(crate) filename: String,
    #[pyo3(get)]
    pub(crate) line: u32,
    #[pyo3(get)]
    pub(crate) column: u32,
    #[pyo3(get)]
    pub(crate) end_line: u32,
    #[pyo3(get)]
    pub(crate) end_column: u32,
    #[pyo3(get)]
    pub(crate) function_name: Option<String>,
    #[pyo3(get)]
    pub(crate) preview_line: Option<String>,
    #[pyo3(get)]
    pub(crate) hide_caret: bool,
    #[pyo3(get)]
    pub(crate) hide_frame_name: bool,
}

#[pymethods]
impl WireStackFrame {
    #[new]
    #[pyo3(signature = (
        filename, line, column, end_line, end_column,
        *, function_name = None, preview_line = None, hide_caret = false, hide_frame_name = false,
    ))]
    #[expect(clippy::too_many_arguments, reason = "mirrors the wire StackFrame fields")]
    fn new(
        filename: String,
        line: u32,
        column: u32,
        end_line: u32,
        end_column: u32,
        function_name: Option<String>,
        preview_line: Option<String>,
        hide_caret: bool,
        hide_frame_name: bool,
    ) -> Self {
        Self {
            filename,
            line,
            column,
            end_line,
            end_column,
            function_name,
            preview_line,
            hide_caret,
            hide_frame_name,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "StackFrame(filename={:?}, line={}, function_name={:?})",
            self.filename,
            self.line,
            self.function_name.as_deref().unwrap_or("<module>")
        )
    }
}

impl WireStackFrame {
    fn from_monty(frame: &StackFrame) -> Self {
        Self {
            filename: frame.filename.clone(),
            line: frame.start.line,
            column: frame.start.column,
            end_line: frame.end.line,
            end_column: frame.end.column,
            function_name: frame.frame_name.clone(),
            preview_line: frame.preview_line.as_ref().map(ToString::to_string),
            hide_caret: frame.hide_caret,
            hide_frame_name: frame.hide_frame_name,
        }
    }

    fn to_monty(&self) -> StackFrame {
        // The fields are already 1-based, so construct `CodeLoc` directly —
        // `CodeLoc::new` takes 0-based values and adds 1.
        StackFrame {
            filename: self.filename.clone(),
            start: CodeLoc {
                line: self.line,
                column: self.column,
            },
            end: CodeLoc {
                line: self.end_line,
                column: self.end_column,
            },
            frame_name: self.function_name.clone(),
            preview_line: self.preview_line.as_deref().map(Into::into),
            hide_caret: self.hide_caret,
            hide_frame_name: self.hide_frame_name,
        }
    }
}

/// A raised Python exception crossing the wire: type name, message, and
/// traceback. Wraps a `monty.MontyException`; build one directly, or convert a
/// caught Python exception with [`RaisedException::from_exception`].
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct RaisedException {
    pub(crate) inner: MontyException,
}

#[pymethods]
impl RaisedException {
    #[new]
    #[pyo3(signature = (exc_type, message = None, traceback = None))]
    fn new(exc_type: &str, message: Option<String>, traceback: Option<Vec<WireStackFrame>>) -> PyResult<Self> {
        let exc_type: ExcType = exc_type
            .parse()
            .map_err(|_| PyValueError::new_err(format!("unknown exception type '{exc_type}'")))?;
        let frames = traceback
            .unwrap_or_default()
            .iter()
            .map(WireStackFrame::to_monty)
            .collect();
        Ok(Self {
            inner: MontyException::with_traceback(exc_type, message, frames),
        })
    }

    /// Builds a `RaisedException` from a caught Python exception (its type and
    /// `str()`). The traceback is not captured.
    #[classmethod]
    fn from_exception(_cls: &Bound<'_, PyType>, py: Python<'_>, exc: Py<PyAny>) -> Self {
        let err = PyErr::from_value(exc.into_bound(py));
        Self {
            inner: exc_py_to_monty(py, &err),
        }
    }

    #[getter]
    fn exc_type(&self) -> String {
        self.inner.exc_type().to_string()
    }

    #[getter]
    fn message(&self) -> Option<String> {
        self.inner.message().map(ToOwned::to_owned)
    }

    #[getter]
    fn traceback(&self) -> Vec<WireStackFrame> {
        self.inner.traceback().iter().map(WireStackFrame::from_monty).collect()
    }

    /// Reconstructs the corresponding native Python exception instance (e.g. a
    /// real `ValueError`), so a server can re-raise what the sandbox raised.
    fn as_exception(&self, py: Python<'_>) -> Py<PyAny> {
        exc_monty_to_py(py, self.inner.clone()).into_value(py).into_any()
    }

    fn __repr__(&self) -> String {
        match self.inner.message() {
            Some(msg) => format!("RaisedException({}: {msg})", self.inner.exc_type()),
            None => format!("RaisedException({})", self.inner.exc_type()),
        }
    }
}

impl RaisedException {
    pub(crate) fn from_monty(exc: MontyException) -> Self {
        Self { inner: exc }
    }

    pub(crate) fn to_proto(&self) -> pb::RaisedException {
        (&self.inner).into()
    }
}

// =============================================================================
// External-function results (answers to FunctionCall / OsCall suspensions)
// =============================================================================

/// The internal sum mirroring `monty::ExtFunctionResult`, but `Clone +
/// PartialEq` so it can live inside a frozen, comparable pyclass.
#[derive(Clone, PartialEq)]
pub(crate) enum ExtResult {
    Return(MontyObject),
    Error(MontyException),
    Future(u32),
    NotFound(String),
}

/// The outcome of a host-side function/OS call, sent back in a [`ResumeCall`]
/// or [`FutureResult`]. Construct via the classmethods.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct ExtFunctionResult {
    pub(crate) result: ExtResult,
}

#[pymethods]
impl ExtFunctionResult {
    /// The call returned `value`.
    #[classmethod]
    fn returns(_cls: &Bound<'_, PyType>, value: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            result: ExtResult::Return(to_monty(value)?),
        })
    }

    /// The call raised `exception`.
    #[classmethod]
    fn error(_cls: &Bound<'_, PyType>, exception: RaisedException) -> Self {
        Self {
            result: ExtResult::Error(exception.inner),
        }
    }

    /// The call is asynchronous: register a future for `call_id` (the id from
    /// the suspension event) and resolve it later via `ResumeFutures`.
    #[classmethod]
    fn future(_cls: &Bound<'_, PyType>, call_id: u32) -> Self {
        Self {
            result: ExtResult::Future(call_id),
        }
    }

    /// No handler exists for the called name — the sandbox raises `NameError`.
    #[classmethod]
    fn not_found(_cls: &Bound<'_, PyType>, name: String) -> Self {
        Self {
            result: ExtResult::NotFound(name),
        }
    }

    /// Which arm this result is: `"return"`, `"error"`, `"future"`, or
    /// `"not_found"`. A child resuming its own call dispatches on this to decide
    /// which payload getter below to read.
    #[getter]
    fn kind(&self) -> &'static str {
        match &self.result {
            ExtResult::Return(_) => "return",
            ExtResult::Error(_) => "error",
            ExtResult::Future(_) => "future",
            ExtResult::NotFound(_) => "not_found",
        }
    }

    /// The returned value for a `"return"` result, else `None`. `None` is
    /// ambiguous with a returned Python `None`; check `kind` to disambiguate.
    #[getter]
    fn value(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        match &self.result {
            ExtResult::Return(value) => Ok(Some(from_monty(py, value)?)),
            _ => Ok(None),
        }
    }

    /// The carried exception for an `"error"` result, else `None`.
    #[getter]
    fn exception(&self) -> Option<RaisedException> {
        match &self.result {
            ExtResult::Error(exc) => Some(RaisedException::from_monty(exc.clone())),
            _ => None,
        }
    }

    /// The missing name for a `"not_found"` result, else `None`.
    #[getter]
    fn name(&self) -> Option<String> {
        match &self.result {
            ExtResult::NotFound(name) => Some(name.clone()),
            _ => None,
        }
    }

    /// The pending call id for a `"future"` result, else `None`.
    #[getter]
    fn future_call_id(&self) -> Option<u32> {
        match &self.result {
            ExtResult::Future(call_id) => Some(*call_id),
            _ => None,
        }
    }

    fn __repr__(&self) -> String {
        match &self.result {
            ExtResult::Return(_) => "ExtFunctionResult.returns(...)".to_owned(),
            ExtResult::Error(exc) => format!("ExtFunctionResult.error({})", exc.exc_type()),
            ExtResult::Future(id) => format!("ExtFunctionResult.future({id})"),
            ExtResult::NotFound(name) => format!("ExtFunctionResult.not_found({name:?})"),
        }
    }
}

impl ExtFunctionResult {
    pub(crate) fn to_proto(&self) -> pb::ExtFunctionResult {
        let kind = match &self.result {
            ExtResult::Return(value) => Kind::ReturnValue(value.clone().into()),
            ExtResult::Error(exc) => Kind::Error(exc.into()),
            ExtResult::Future(call_id) => Kind::Future(*call_id),
            ExtResult::NotFound(name) => Kind::NotFound(name.clone()),
        };
        pb::ExtFunctionResult { kind: Some(kind) }
    }

    pub(crate) fn from_proto(result: pb::ExtFunctionResult) -> PyResult<Self> {
        let kind = result
            .kind
            .ok_or_else(|| PyValueError::new_err("ExtFunctionResult has no kind"))?;
        let result = match kind {
            Kind::ReturnValue(value) => ExtResult::Return(wire_object_into(value)?),
            Kind::Error(err) => ExtResult::Error(MontyException::try_from(err).map_err(proto_err)?),
            Kind::Future(call_id) => ExtResult::Future(call_id),
            Kind::NotFound(name) => ExtResult::NotFound(name),
        };
        Ok(Self { result })
    }
}

/// A resolved future: the `call_id` from a `ResolveFutures` suspension and the
/// [`ExtFunctionResult`] for it. Used inside [`ResumeFutures`].
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct FutureResult {
    #[pyo3(get)]
    pub(crate) call_id: u32,
    #[pyo3(get)]
    pub(crate) result: ExtFunctionResult,
}

#[pymethods]
impl FutureResult {
    #[new]
    fn new(call_id: u32, result: ExtFunctionResult) -> Self {
        Self { call_id, result }
    }

    fn __repr__(&self) -> String {
        format!("FutureResult(call_id={})", self.call_id)
    }
}

// =============================================================================
// Shared decode helpers
// =============================================================================

/// Unwraps a decoded `WireObject` into a `MontyObject`, rejecting an absent
/// value (an empty `MontyObject` message on the wire).
pub(crate) fn wire_object_into(obj: WireObject) -> PyResult<MontyObject> {
    obj.into_object().map_err(proto_err)
}

/// Maps a proto→Rust conversion failure onto a Python `ValueError`.
pub(crate) fn proto_err(err: impl Display) -> PyErr {
    PyValueError::new_err(format!("invalid wire message: {err}"))
}
