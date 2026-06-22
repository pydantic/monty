//! The `ChildEvent` oneof arms as Python classes (child → parent).
//!
//! Every event also carries the envelope's two timing fields
//! (`total_execution_micros`, `max_duration_micros`) as plain getters; they
//! default to `0` / `None` for hand-built events and are preserved across a
//! decode → encode round trip. [`crate::wire::encode_child_event`] downcasts a
//! Python object to the matching arm; [`crate::wire::decode_child_event`]
//! dispatches on `kind`.

use monty::{MontyException, MontyObject};
use monty_proto::pb;
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyDict, PyList},
};

use crate::messages::{
    RaisedException, from_monty, from_monty_kwargs, from_monty_seq, proto_err, to_monty, to_monty_kwargs, to_monty_seq,
    wire_object_into,
};

/// The two timing fields carried on every `ChildEvent` envelope; a small
/// carrier used only to thread them from a decoded event into each arm's
/// constructor.
#[derive(Clone, Copy)]
pub(crate) struct Timing {
    total_execution_micros: u64,
    max_duration_micros: Option<u64>,
}

impl Timing {
    /// Reads the envelope timing off a decoded `ChildEvent`.
    fn of(event: &pb::ChildEvent) -> Self {
        Self {
            total_execution_micros: event.total_execution_micros,
            max_duration_micros: event.max_duration_micros,
        }
    }
}

/// Wraps an event `kind` plus its timing into a `pb::ChildEvent` envelope.
fn envelope(
    kind: pb::child_event::Kind,
    total_execution_micros: u64,
    max_duration_micros: Option<u64>,
) -> pb::ChildEvent {
    pb::ChildEvent {
        kind: Some(kind),
        total_execution_micros,
        max_duration_micros,
    }
}

/// Streamed sandbox `print()` output, flushed at line granularity.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Print {
    /// `"stdout"` or `"stderr"`.
    #[pyo3(get)]
    stream: String,
    #[pyo3(get)]
    text: String,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl Print {
    #[new]
    #[pyo3(signature = (stream, text, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(
        stream: &str,
        text: String,
        total_execution_micros: u64,
        max_duration_micros: Option<u64>,
    ) -> PyResult<Self> {
        print_stream_to_proto(stream)?;
        Ok(Self {
            stream: stream.to_owned(),
            text,
            total_execution_micros,
            max_duration_micros,
        })
    }

    fn __repr__(&self) -> String {
        format!("Print(stream={:?}, text={:?})", self.stream, self.text)
    }
}

impl Print {
    pub(crate) fn to_event(&self) -> PyResult<pb::ChildEvent> {
        let kind = pb::child_event::Kind::Print(pb::Print {
            stream: print_stream_to_proto(&self.stream)?,
            text: self.text.clone(),
        });
        Ok(envelope(kind, self.total_execution_micros, self.max_duration_micros))
    }

    pub(crate) fn from_proto(print: pb::Print, timing: Timing) -> PyResult<Self> {
        Ok(Self {
            stream: print_stream_from_proto(print.stream)?.to_owned(),
            text: print.text,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        })
    }
}

/// Suspension: the sandbox called an external function. Answer with
/// `ResumeCall`. When `method_call` is true the first arg is the receiver.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct FunctionCall {
    #[pyo3(get)]
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
    #[pyo3(get)]
    call_id: u32,
    #[pyo3(get)]
    method_call: bool,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl FunctionCall {
    #[new]
    #[pyo3(signature = (
        function_name, *, args = None, kwargs = None, call_id = 0, method_call = false,
        total_execution_micros = 0, max_duration_micros = None,
    ))]
    fn new(
        function_name: String,
        args: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyAny>>,
        call_id: u32,
        method_call: bool,
        total_execution_micros: u64,
        max_duration_micros: Option<u64>,
    ) -> PyResult<Self> {
        Ok(Self {
            function_name,
            args: args.map(to_monty_seq).transpose()?.unwrap_or_default(),
            kwargs: kwargs.map(to_monty_kwargs).transpose()?.unwrap_or_default(),
            call_id,
            method_call,
            total_execution_micros,
            max_duration_micros,
        })
    }

    #[getter]
    fn args(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        from_monty_seq(py, &self.args)
    }

    #[getter]
    fn kwargs(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        from_monty_kwargs(py, &self.kwargs)
    }

    fn __repr__(&self) -> String {
        format!(
            "FunctionCall(function_name={:?}, call_id={}, method_call={})",
            self.function_name, self.call_id, self.method_call
        )
    }
}

impl FunctionCall {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::FunctionCall(monty_proto::WireFunctionCall {
            function_name: self.function_name.clone(),
            args: self.args.clone(),
            kwargs: self.kwargs.clone(),
            call_id: self.call_id,
            method_call: self.method_call,
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(call: monty_proto::WireFunctionCall, timing: Timing) -> Self {
        Self {
            function_name: call.function_name,
            args: call.args,
            kwargs: call.kwargs,
            call_id: call.call_id,
            method_call: call.method_call,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        }
    }
}

/// Suspension: the sandbox performed an OS operation no mount handled. Answer
/// with `ResumeCall`.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct OsCall {
    #[pyo3(get)]
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
    #[pyo3(get)]
    call_id: u32,
    /// The exception the sandbox would raise if nothing handles this call; a
    /// caller with no handler should resume with this error.
    #[pyo3(get)]
    not_handled_error: Option<RaisedException>,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl OsCall {
    #[new]
    #[pyo3(signature = (
        function_name, *, args = None, kwargs = None, call_id = 0, not_handled_error = None,
        total_execution_micros = 0, max_duration_micros = None,
    ))]
    fn new(
        function_name: String,
        args: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyAny>>,
        call_id: u32,
        not_handled_error: Option<RaisedException>,
        total_execution_micros: u64,
        max_duration_micros: Option<u64>,
    ) -> PyResult<Self> {
        Ok(Self {
            function_name,
            args: args.map(to_monty_seq).transpose()?.unwrap_or_default(),
            kwargs: kwargs.map(to_monty_kwargs).transpose()?.unwrap_or_default(),
            call_id,
            not_handled_error,
            total_execution_micros,
            max_duration_micros,
        })
    }

    #[getter]
    fn args(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        from_monty_seq(py, &self.args)
    }

    #[getter]
    fn kwargs(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        from_monty_kwargs(py, &self.kwargs)
    }

    fn __repr__(&self) -> String {
        format!(
            "OsCall(function_name={:?}, call_id={})",
            self.function_name, self.call_id
        )
    }
}

impl OsCall {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::OsCall(monty_proto::WireOsCall {
            function_name: self.function_name.clone(),
            args: self.args.clone(),
            kwargs: self.kwargs.clone(),
            call_id: self.call_id,
            not_handled_error: self.not_handled_error.as_ref().map(RaisedException::to_proto),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(call: monty_proto::WireOsCall, timing: Timing) -> PyResult<Self> {
        let not_handled_error = call
            .not_handled_error
            .map(|err| {
                Ok::<_, PyErr>(RaisedException::from_monty(
                    MontyException::try_from(err).map_err(proto_err)?,
                ))
            })
            .transpose()?;
        Ok(Self {
            function_name: call.function_name,
            args: call.args,
            kwargs: call.kwargs,
            call_id: call.call_id,
            not_handled_error,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        })
    }
}

/// Suspension: the sandbox read an undefined name. Answer with
/// `ResumeNameLookup`.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct NameLookup {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl NameLookup {
    #[new]
    #[pyo3(signature = (name, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(name: String, total_execution_micros: u64, max_duration_micros: Option<u64>) -> Self {
        Self {
            name,
            total_execution_micros,
            max_duration_micros,
        }
    }

    fn __repr__(&self) -> String {
        format!("NameLookup(name={:?})", self.name)
    }
}

impl NameLookup {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::NameLookup(pb::NameLookup {
            name: self.name.clone(),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(lookup: pb::NameLookup, timing: Timing) -> Self {
        Self {
            name: lookup.name,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        }
    }
}

/// Suspension: every sandbox task is blocked on external futures. Answer with
/// `ResumeFutures`.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct ResolveFutures {
    #[pyo3(get)]
    pending_call_ids: Vec<u32>,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl ResolveFutures {
    #[new]
    #[pyo3(signature = (pending_call_ids, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(pending_call_ids: Vec<u32>, total_execution_micros: u64, max_duration_micros: Option<u64>) -> Self {
        Self {
            pending_call_ids,
            total_execution_micros,
            max_duration_micros,
        }
    }

    fn __repr__(&self) -> String {
        format!("ResolveFutures(pending_call_ids={:?})", self.pending_call_ids)
    }
}

impl ResolveFutures {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::ResolveFutures(pb::ResolveFutures {
            pending_call_ids: self.pending_call_ids.clone(),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(resolve: pb::ResolveFutures, timing: Timing) -> Self {
        Self {
            pending_call_ids: resolve.pending_call_ids,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        }
    }
}

/// Turn end: the snippet completed with this value.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Complete {
    value: MontyObject,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl Complete {
    #[new]
    #[pyo3(signature = (value, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(value: &Bound<'_, PyAny>, total_execution_micros: u64, max_duration_micros: Option<u64>) -> PyResult<Self> {
        Ok(Self {
            value: to_monty(value)?,
            total_execution_micros,
            max_duration_micros,
        })
    }

    #[getter]
    fn value(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        from_monty(py, &self.value)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "Complete(value={})",
            from_monty(py, &self.value)?.bind(py).repr()?
        ))
    }
}

impl Complete {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::Complete(pb::Complete {
            value: Some(self.value.clone().into()),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(complete: pb::Complete, timing: Timing) -> PyResult<Self> {
        let value = complete.value.ok_or_else(|| proto_err("Complete has no value"))?;
        Ok(Self {
            value: wire_object_into(value)?,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        })
    }
}

/// Turn end: the snippet failed with a Python exception. The session survives.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Error {
    #[pyo3(get)]
    exception: RaisedException,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl Error {
    #[new]
    #[pyo3(signature = (exception, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(exception: RaisedException, total_execution_micros: u64, max_duration_micros: Option<u64>) -> Self {
        Self {
            exception,
            total_execution_micros,
            max_duration_micros,
        }
    }

    fn __repr__(&self) -> String {
        format!("Error({})", self.exception.inner.exc_type())
    }
}

impl Error {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::Error(pb::Error {
            exception: Some(self.exception.to_proto()),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(error: pb::Error, timing: Timing) -> PyResult<Self> {
        let exception = error.exception.ok_or_else(|| proto_err("Error has no exception"))?;
        Ok(Self {
            exception: RaisedException::from_monty(MontyException::try_from(exception).map_err(proto_err)?),
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        })
    }
}

/// Turn end: type checking rejected the fed snippet (not executed).
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct TypingError {
    #[pyo3(get)]
    diagnostics: String,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl TypingError {
    #[new]
    #[pyo3(signature = (diagnostics, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(diagnostics: String, total_execution_micros: u64, max_duration_micros: Option<u64>) -> Self {
        Self {
            diagnostics,
            total_execution_micros,
            max_duration_micros,
        }
    }

    fn __repr__(&self) -> String {
        format!("TypingError({:?})", self.diagnostics)
    }
}

impl TypingError {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::TypingError(pb::TypingError {
            diagnostics: self.diagnostics.clone(),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(error: pb::TypingError, timing: Timing) -> Self {
        Self {
            diagnostics: error.diagnostics,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        }
    }
}

/// Reply to `Dump`: the opaque, version-pinned snapshot bytes.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct DumpResult {
    #[pyo3(get)]
    state: Vec<u8>,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl DumpResult {
    #[new]
    #[pyo3(signature = (state, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(state: Vec<u8>, total_execution_micros: u64, max_duration_micros: Option<u64>) -> Self {
        Self {
            state,
            total_execution_micros,
            max_duration_micros,
        }
    }

    fn __repr__(&self) -> String {
        format!("DumpResult(<{} bytes>)", self.state.len())
    }
}

impl DumpResult {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::DumpResult(pb::DumpResult {
            state: self.state.clone(),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(dump: pb::DumpResult, timing: Timing) -> Self {
        Self {
            state: dump.state,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        }
    }
}

/// Generic acknowledgement for `StartSession` / `Load` / `Reset` / `Shutdown`.
///
/// The Rust type is `OkEvent` (the Python name is `Ok`) so it does not shadow
/// the `Ok` variant of `Result` inside this module.
#[pyclass(name = "Ok", frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct OkEvent {
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl OkEvent {
    #[new]
    #[pyo3(signature = (*, total_execution_micros = 0, max_duration_micros = None))]
    fn new(total_execution_micros: u64, max_duration_micros: Option<u64>) -> Self {
        Self {
            total_execution_micros,
            max_duration_micros,
        }
    }

    fn __repr__(&self) -> &'static str {
        let _ = self;
        "Ok()"
    }
}

impl OkEvent {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        envelope(
            pb::child_event::Kind::Ok(pb::Ok {}),
            self.total_execution_micros,
            self.max_duration_micros,
        )
    }

    pub(crate) fn from_proto(timing: Timing) -> Self {
        Self {
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        }
    }
}

/// The child hit an unrecoverable error and exits immediately after this. EOF
/// *without* a `FatalError` means the child crashed hard.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct FatalError {
    #[pyo3(get)]
    message: String,
    #[pyo3(get)]
    total_execution_micros: u64,
    #[pyo3(get)]
    max_duration_micros: Option<u64>,
}

#[pymethods]
impl FatalError {
    #[new]
    #[pyo3(signature = (message, *, total_execution_micros = 0, max_duration_micros = None))]
    fn new(message: String, total_execution_micros: u64, max_duration_micros: Option<u64>) -> Self {
        Self {
            message,
            total_execution_micros,
            max_duration_micros,
        }
    }

    fn __repr__(&self) -> String {
        format!("FatalError({:?})", self.message)
    }
}

impl FatalError {
    pub(crate) fn to_event(&self) -> pb::ChildEvent {
        let kind = pb::child_event::Kind::FatalError(pb::FatalError {
            message: self.message.clone(),
        });
        envelope(kind, self.total_execution_micros, self.max_duration_micros)
    }

    pub(crate) fn from_proto(fatal: pb::FatalError, timing: Timing) -> Self {
        Self {
            message: fatal.message,
            total_execution_micros: timing.total_execution_micros,
            max_duration_micros: timing.max_duration_micros,
        }
    }
}

// =============================================================================
// Decode dispatch + print-stream enum mapping
// =============================================================================

/// Builds the matching Python event object from a decoded `pb::ChildEvent`.
pub(crate) fn event_from_proto(py: Python<'_>, event: pb::ChildEvent) -> PyResult<Py<PyAny>> {
    let timing = Timing::of(&event);
    let kind = event.kind.ok_or_else(|| proto_err("ChildEvent has no kind"))?;
    let obj = match kind {
        pb::child_event::Kind::Print(p) => Py::new(py, Print::from_proto(p, timing)?)?.into_any(),
        pb::child_event::Kind::FunctionCall(c) => Py::new(py, FunctionCall::from_proto(c, timing))?.into_any(),
        pb::child_event::Kind::OsCall(c) => Py::new(py, OsCall::from_proto(c, timing)?)?.into_any(),
        pb::child_event::Kind::NameLookup(l) => Py::new(py, NameLookup::from_proto(l, timing))?.into_any(),
        pb::child_event::Kind::ResolveFutures(r) => Py::new(py, ResolveFutures::from_proto(r, timing))?.into_any(),
        pb::child_event::Kind::Complete(c) => Py::new(py, Complete::from_proto(c, timing)?)?.into_any(),
        pb::child_event::Kind::Error(e) => Py::new(py, Error::from_proto(e, timing)?)?.into_any(),
        pb::child_event::Kind::TypingError(e) => Py::new(py, TypingError::from_proto(e, timing))?.into_any(),
        pb::child_event::Kind::DumpResult(d) => Py::new(py, DumpResult::from_proto(d, timing))?.into_any(),
        pb::child_event::Kind::Ok(_) => Py::new(py, OkEvent::from_proto(timing))?.into_any(),
        pb::child_event::Kind::FatalError(f) => Py::new(py, FatalError::from_proto(f, timing))?.into_any(),
    };
    Ok(obj)
}

/// Maps a stream string onto the proto `PrintStream` enum value.
fn print_stream_to_proto(stream: &str) -> PyResult<i32> {
    match stream {
        "stdout" => Ok(pb::PrintStream::Stdout as i32),
        "stderr" => Ok(pb::PrintStream::Stderr as i32),
        other => Err(PyValueError::new_err(format!(
            "invalid print stream '{other}'; expected 'stdout' or 'stderr'"
        ))),
    }
}

/// Maps a proto `PrintStream` enum value back onto a stream string.
fn print_stream_from_proto(stream: i32) -> PyResult<&'static str> {
    match pb::PrintStream::try_from(stream) {
        Ok(pb::PrintStream::Stdout) => Ok("stdout"),
        Ok(pb::PrintStream::Stderr) => Ok("stderr"),
        _ => Err(PyValueError::new_err(format!(
            "print event has unspecified or unknown stream {stream}"
        ))),
    }
}
