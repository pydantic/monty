//! Running fed snippets in embedded CPython, and the namespace the sandbox runs
//! in and reaches the parent through.
//!
//! The execution `globals` *is* the [`SandboxGlobals`] — a `dict` subclass, so
//! CPython resolves every unbound global name through its `__missing__`. That
//! resolves the name *eagerly* with a `NameLookup` round trip and branches on the
//! parent's answer: a host **function** becomes an [`ExternalFunction`] proxy
//! (whose `__call__` round-trips a `FunctionCall`), any other **value** is
//! converted and returned directly, and an **unknown** name raises `NameError`.
//! All the real work — name resolution, value conversion, the transport round
//! trips — happens in Rust on [`SandboxGlobals`]; the Python glue is intentionally
//! tiny (the REPL runner plus traceback extraction).
//!
//! SECURITY: this runs untrusted code in *full CPython*, which is not itself a
//! sandbox (the code can `import os` and do anything this process can). Isolation
//! is the deployment's responsibility — see this crate's `README.md`.

use std::{
    cell::{Cell, RefCell},
    ffi::CStr,
};

use ahash::AHashMap;
use monty::{ExtFunctionResult, MontyObject, NameLookupResult};
use monty_proto::{
    exceeds_max_value_depth, pb,
    python::{DcRegistry, exc_monty_to_py, monty_to_py, py_to_monty_value},
};
use pyo3::{
    exceptions::{PyKeyError, PyNameError, PyRuntimeError, PyValueError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyModule, PyString, PyTuple},
};

use crate::{
    events::{function_call_event, name_lookup_event, print_event},
    transport::{Incoming, SendError, SharedTransport},
};

/// Python glue, executed once per process. Defines the REPL runner (`run`) and
/// traceback extractor; everything else — the namespace, name resolution, host
/// calls — lives in Rust on [`SandboxGlobals`].
///
/// The source lives in `runner.py` (so it reads/edits/lints as real Python) and
/// is inlined here at compile time as a `&CStr` — `include_str!` embeds the file,
/// `concat!` appends the NUL `PyModule::from_code` requires, and the conversion is
/// `const`, so there is no runtime allocation or fallible parse.
const RUNNER: &CStr = match CStr::from_bytes_with_nul(concat!(include_str!("runner.py"), "\0").as_bytes()) {
    Ok(runner) => runner,
    Err(_) => panic!("runner.py must not contain a NUL byte"),
};

/// The compiled `RUNNER` module, providing the REPL runner (`run`) and the
/// traceback rebuilder (`extract_traceback`) the session calls.
pub struct Runner {
    run: Py<PyAny>,
    extract_traceback: Py<PyAny>,
}

impl Runner {
    /// Compiles [`RUNNER`] into the module the session drives feeds through.
    pub fn new(py: Python<'_>) -> PyResult<Self> {
        let module = PyModule::from_code(py, RUNNER, c"runner.py", c"runner")?;
        Ok(Self {
            run: module.getattr("run")?.unbind(),
            extract_traceback: module.getattr("extract_traceback")?.unbind(),
        })
    }

    /// Runs `code` in `namespace`, returning the trailing expression's value.
    /// The snippet compiles under an internal `<input-N>` filename (see
    /// `runner.py`'s `run`); the parent-visible name is applied when a traceback
    /// is rebuilt via [`Runner::extract_traceback`], or directly on syntax
    /// errors because they have no user traceback frame to rewrite.
    pub fn run<'py>(
        &self,
        py: Python<'py>,
        code: String,
        namespace: &Bound<'py, SandboxGlobals>,
        script_name: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.run.bind(py).call1((code, namespace, script_name))
    }

    /// Extracts a structured traceback from the given `traceback` object, using
    /// `script_name` as the filename for the traceback's frames.
    pub fn extract_traceback<'py>(
        &self,
        py: Python<'py>,
        traceback: &Bound<'py, PyAny>,
        script_name: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.extract_traceback.bind(py).call1((traceback, script_name))
    }
}

/// The sandbox's execution `globals`, and its bridge to the host for the names it
/// cannot resolve itself: resolving undefined names (`NameLookup`) and calling
/// host functions (`FunctionCall`). (`print()` output goes through [`Stdio`].)
///
/// It is a `dict` subclass (`extends=PyDict`) used directly as the feed's
/// `globals`: CPython routes any unbound global name through its
/// [`__missing__`](SandboxGlobals::__missing__), so no separate namespace wrapper
/// is needed. Owns the shared transport, the per-session call-id counter, and a
/// cache of resolved external-function proxies.
#[pyclass(extends=PyDict, unsendable)]
pub struct SandboxGlobals {
    transport: SharedTransport,
    dc: DcRegistry,
    next_call_id: Cell<u32>,
    /// Resolved external functions, keyed by name. Only *functions* are cached
    /// (so repeated references/calls skip the `NameLookup`); host *values* are
    /// re-read live on every reference, and unknown names re-raise `NameError`.
    functions: RefCell<AHashMap<String, Py<ExternalFunction>>>,
}

impl SandboxGlobals {
    /// Builds a bridge over `transport` for one session.
    pub fn new(py: Python<'_>, transport: SharedTransport) -> Self {
        Self {
            transport,
            dc: DcRegistry::new(py),
            next_call_id: Cell::new(0),
            functions: RefCell::new(AHashMap::new()),
        }
    }

    /// Sends a suspension `event` and blocks for the parent's next request,
    /// mapping transport failures onto Python exceptions. The caller matches the
    /// specific resume kind it expects (`ResumeCall` / `ResumeNameLookup`).
    ///
    /// The GIL is intentionally held across the blocking round trip: this child
    /// serves a single session on one thread, so nothing else needs it.
    fn suspend(&self, event: &pb::ChildEvent) -> PyResult<pb::ParentRequest> {
        let mut transport = self.transport.borrow_mut();
        if let Err(err) = transport.send(event) {
            return Err(send_error_to_py(&err));
        }
        match transport.recv() {
            Incoming::Request(request) => Ok(request),
            Incoming::Eof => Err(PyRuntimeError::new_err("parent disconnected during a host call")),
            Incoming::Malformed(msg) | Incoming::Fatal(msg) => Err(PyRuntimeError::new_err(format!(
                "transport error during a host call: {msg}"
            ))),
        }
    }

    /// Sends a `FunctionCall` and blocks for the matching `ResumeCall`.
    fn round_trip(&self, event: &pb::ChildEvent, call_id: u32) -> PyResult<ExtFunctionResult> {
        match self.suspend(event)?.kind {
            Some(pb::parent_request::Kind::ResumeCall(resume)) if resume.call_id == call_id => {
                let result = resume
                    .result
                    .ok_or_else(|| PyRuntimeError::new_err("ResumeCall has no result"))?;
                ExtFunctionResult::try_from(result)
                    .map_err(|err| PyValueError::new_err(format!("invalid ResumeCall result: {err}")))
            }
            Some(pb::parent_request::Kind::ResumeCall(resume)) => Err(PyRuntimeError::new_err(format!(
                "ResumeCall call_id mismatch: got {}, expected {call_id}",
                resume.call_id
            ))),
            _ => Err(PyRuntimeError::new_err(
                "expected a ResumeCall while suspended in a host call",
            )),
        }
    }

    /// Sends a `NameLookup` and blocks for the matching `ResumeNameLookup`.
    fn name_lookup(&self, event: &pb::ChildEvent) -> PyResult<NameLookupResult> {
        match self.suspend(event)?.kind {
            Some(pb::parent_request::Kind::ResumeNameLookup(resume)) => NameLookupResult::try_from(resume)
                .map_err(|err| PyValueError::new_err(format!("invalid ResumeNameLookup result: {err}"))),
            _ => Err(PyRuntimeError::new_err(
                "expected a ResumeNameLookup while suspended in a name lookup",
            )),
        }
    }

    /// Converts a Python argument to a wire value, rejecting over-deep nesting
    /// before it can produce an undecodable frame.
    fn to_wire(&self, py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<MontyObject> {
        let value = py_to_monty_value(obj, &self.dc).map_err(|exc| exc_monty_to_py(py, exc))?;
        if exceeds_max_value_depth(&value) {
            return Err(PyValueError::new_err(
                "value is too deeply nested to send over the wire",
            ));
        }
        Ok(value)
    }
}

#[pymethods]
impl SandboxGlobals {
    /// CPython calls this for any global name missing from the namespace (this
    /// object is the `dict` subclass used as `globals`). Builtins and dunders are
    /// not host names, so they raise `KeyError` to fall through to CPython's normal
    /// builtins resolution; every other name is resolved eagerly with a
    /// `NameLookup` round trip and branches on the parent's answer:
    /// - a host **function** → an [`ExternalFunction`] proxy whose `__call__`
    ///   round-trips a `FunctionCall` (cached so repeated references skip the
    ///   lookup; the parent signals "function" as `MontyObject::Function`);
    /// - any other **value** → the converted Python value (re-read live on every
    ///   reference, never cached);
    /// - **undefined** → `NameError` (matching CPython, which raises on reference).
    fn __missing__(slf: &Bound<'_, Self>, name: String) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        // Dunders and builtins fall through to normal resolution: raising
        // `KeyError` makes CPython's global lookup consult the real builtins.
        if name.starts_with("__") || import_builtins(py)?.bind(py).hasattr(name.as_str())? {
            return Err(PyKeyError::new_err(name));
        }
        let host = slf.borrow();
        if let Some(cached) = host.functions.borrow().get(&name) {
            return Ok(cached.clone_ref(py).into_any());
        }
        match host.name_lookup(&name_lookup_event(name.clone()))? {
            NameLookupResult::Value(MontyObject::Function { .. }) => {
                let func = Py::new(
                    py,
                    ExternalFunction {
                        host: slf.clone().unbind(),
                        name: name.clone(),
                    },
                )?;
                host.functions.borrow_mut().insert(name, func.clone_ref(py));
                Ok(func.into_any())
            }
            NameLookupResult::Value(obj) => monty_to_py(py, &obj, &host.dc),
            NameLookupResult::Undefined => Err(PyNameError::new_err(format!("name '{name}' is not defined"))),
        }
    }
}

/// A sandbox output stream, installed as `sys.stdout` and `sys.stderr`. A minimal
/// text sink: each `write` forwards the chunk to the parent as a `Print` event
/// tagged with its [`stream`](pb::PrintStream), and `flush` is a no-op (every
/// write is already flushed to the parent). Holds its own clone of the shared
/// transport, independent of the [`SandboxGlobals`] namespace.
#[pyclass(unsendable)]
pub struct Stdio {
    transport: SharedTransport,
    stream: pb::PrintStream,
}

impl Stdio {
    /// Builds a stream sink that tags its `Print` events as `stream`.
    pub fn new(transport: SharedTransport, stream: pb::PrintStream) -> Self {
        Self { transport, stream }
    }
}

#[pymethods]
impl Stdio {
    /// `write`: stream a `print()` chunk as a `Print` event on this stream.
    fn write(&self, text: &str) -> PyResult<usize> {
        if !text.is_empty() {
            let event = print_event(self.stream, text.to_owned());
            self.transport
                .borrow_mut()
                .send(&event)
                .map_err(|err| send_error_to_py(&err))?;
        }
        // CPython's `TextIOBase.write` returns the number of *characters*
        // written, not bytes, so count chars (matters for non-ASCII text).
        Ok(text.chars().count())
    }

    /// `flush`: a no-op — each write is already flushed to the parent.
    fn flush(&self) {
        let _ = self;
    }
}

pub fn import_builtins(py: Python<'_>) -> PyResult<&Py<PyModule>> {
    static BUILTINS: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

    BUILTINS.get_or_try_init(py, || py.import("builtins").map(Bound::unbind))
}

/// A proxy for a host **function**, returned by [`SandboxGlobals::__missing__`] when
/// a `NameLookup` resolves the name to a `MontyObject::Function`. It keeps a
/// reference to its [`SandboxGlobals`] and the name; calling it (`__call__`) converts
/// the arguments and round-trips a `FunctionCall` to the parent. Host *values*
/// and unknown names never produce one — they are returned/raised by
/// `__missing__` directly.
#[pyclass(unsendable)]
pub struct ExternalFunction {
    host: Py<SandboxGlobals>,
    name: String,
}

#[pymethods]
impl ExternalFunction {
    /// Calls the undefined name: convert the args/kwargs to wire values, round-trip
    /// a `FunctionCall` through the host, and return its result — or raise whatever
    /// the parent reported (`NameError` for a genuinely unknown name).
    #[pyo3(signature = (*args, **kwargs))]
    fn __call__(
        &self,
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<PyAny>> {
        let host = self.host.borrow(py);

        let mut wire_args = Vec::with_capacity(args.len());
        for arg in args.iter() {
            wire_args.push(host.to_wire(py, &arg)?);
        }
        let mut wire_kwargs = Vec::new();
        if let Some(kwargs) = kwargs {
            wire_kwargs.reserve(kwargs.len());
            for (key, value) in kwargs.iter() {
                let key: String = key
                    .cast::<PyString>()
                    .map_err(|_| PyValueError::new_err("keyword argument names must be strings"))?
                    .extract()?;
                wire_kwargs.push((MontyObject::String(key), host.to_wire(py, &value)?));
            }
        }

        let call_id = host.next_call_id.get();
        host.next_call_id.set(call_id.wrapping_add(1));
        let event = function_call_event(self.name.clone(), wire_args, wire_kwargs, call_id);

        match host.round_trip(&event, call_id)? {
            ExtFunctionResult::Return(obj) => monty_to_py(py, &obj, &host.dc),
            ExtFunctionResult::Error(exc) => Err(exc_monty_to_py(py, exc)),
            ExtFunctionResult::NotFound(name) => Err(PyNameError::new_err(format!("name '{name}' is not defined"))),
            ExtFunctionResult::Future(_) => Err(PyRuntimeError::new_err(
                "async host functions are not supported by the CPython worker",
            )),
        }
    }
}

/// Maps a transport send failure onto the Python exception the sandbox sees.
fn send_error_to_py(err: &SendError) -> PyErr {
    match err {
        SendError::TooLarge { len, max } => {
            PyValueError::new_err(format!("value frame of {len} bytes exceeds the maximum of {max} bytes"))
        }
        SendError::Io(msg) => PyRuntimeError::new_err(format!("failed to send to parent: {msg}")),
    }
}
