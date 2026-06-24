//! Running fed snippets in embedded CPython, and the bridge the sandbox uses to
//! reach the parent.
//!
//! The execution `globals` is a `dict` subclass (`_CallbackGlobals`, defined in
//! [`PREAMBLE`]) so CPython resolves every unbound global name through
//! `__missing__`: builtins and dunders fall through, any other name becomes a
//! proxy that calls back to the parent. All the real work — value conversion and
//! the transport round trip — happens in Rust on [`HostBridge`]; the Python glue
//! is intentionally tiny.
//!
//! SECURITY: this runs untrusted code in *full CPython*, which is not itself a
//! sandbox (the code can `import os` and do anything this process can). Isolation
//! is the deployment's responsibility — see this crate's `README.md`.

use std::{cell::Cell, ffi::CStr};

use _monty::{
    convert::{monty_to_py, py_to_monty_value},
    dataclass::DcRegistry,
    exceptions::exc_monty_to_py,
};
use monty::{ExtFunctionResult, MontyObject};
use monty_proto::{exceeds_max_value_depth, pb};
use pyo3::{
    exceptions::{PyNameError, PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyDict, PyModule, PyString, PyTuple},
};

use crate::{
    events::{function_call_event, print_event},
    transport::{Incoming, SendError, SharedTransport},
};

/// Python glue, executed once per process. Defines the sandbox namespace type
/// and the REPL runner; everything else lives in Rust via the `host` object.
const PREAMBLE: &CStr = cr#"
import ast as _ast, builtins as _builtins, asyncio as _asyncio

# inspect.CO_COROUTINE — set on a code object the compiler turned into a coroutine
# because it contains a top-level `await`. A bare expression that merely *evaluates*
# to a coroutine (e.g. calling an `async def`) does NOT get this flag, so we only
# auto-run genuine top-level-await snippets, never arbitrary coroutine values.
_CO_COROUTINE = 0x80
# Allow `await`/`async for`/`async with` at module level (the flag the asyncio
# REPL and IPython use); the compiled unit then needs driving to completion.
_TOP_LEVEL_AWAIT = _ast.PyCF_ALLOW_TOP_LEVEL_AWAIT


class _CallbackGlobals(dict):
    """Execution globals whose missing-name lookups become host calls.

    Because this is a `dict` *subclass*, CPython resolves unbound global names
    through `__missing__`. Builtins and dunders fall through (raise `KeyError`);
    any other unbound name becomes a proxy that calls back to the parent.
    """

    def __init__(self, host):
        super().__init__()
        self._host = host

    def __missing__(self, name):
        if name.startswith('__') or hasattr(_builtins, name):
            raise KeyError(name)
        host = self._host

        def _ext(*args, **kwargs):
            return host.call(name, args, kwargs)

        return _ext


def _run(code, ns):
    """Execute `code` REPL-style: a trailing expression becomes the value.

    Mirrors how IPython/the stdlib REPL split a cell — run the body in `exec`
    mode, then evaluate a trailing *expression* statement separately so its value
    can be returned. The split node keeps its original location, so a traceback
    from the trailing expression still points at the right line.

    Top-level `await` is supported: both halves are compiled with
    `PyCF_ALLOW_TOP_LEVEL_AWAIT`, and any half that the compiler turned into a
    coroutine is driven to completion with `asyncio.run`. Purely synchronous
    snippets never touch asyncio.
    """
    module = _ast.parse(code, '<sandbox>', 'exec')
    trailing_expr = None
    if module.body and isinstance(module.body[-1], _ast.Expr):
        trailing_expr = module.body.pop().value
    _drive(compile(module, '<sandbox>', 'exec', flags=_TOP_LEVEL_AWAIT), ns)
    if trailing_expr is None:
        return None
    return _drive(compile(_ast.Expression(trailing_expr), '<sandbox>', 'eval', flags=_TOP_LEVEL_AWAIT), ns)


def _drive(code, ns):
    """Run a compiled code object, awaiting it if it's a top-level coroutine."""
    result = eval(code, ns)
    return _asyncio.run(result) if code.co_flags & _CO_COROUTINE else result
"#;

/// Compiles [`PREAMBLE`] into a module whose `_CallbackGlobals` and `_run`
/// the session uses to build namespaces and execute feeds.
pub fn init_runner(py: Python<'_>) -> PyResult<Py<PyModule>> {
    let module = PyModule::from_code(py, PREAMBLE, c"<monty-cpython-runner>", c"_monty_runner")?;
    Ok(module.unbind())
}

/// The bridge the sandbox calls into for everything it cannot do itself: host
/// functions (undefined names) and `print()` output. Also serves as the
/// sandbox's `sys.stdout`. Owns the shared transport and the per-session
/// call-id counter.
#[pyclass(unsendable)]
pub struct HostBridge {
    transport: SharedTransport,
    dc: DcRegistry,
    next_call_id: Cell<u32>,
}

impl HostBridge {
    /// Builds a bridge over `transport` for one session.
    pub fn new(py: Python<'_>, transport: SharedTransport) -> Self {
        Self {
            transport,
            dc: DcRegistry::new(py),
            next_call_id: Cell::new(0),
        }
    }

    /// Sends a `FunctionCall` and blocks for the matching `ResumeCall`.
    ///
    /// The GIL is intentionally held across the blocking round trip: this child
    /// serves a single session on one thread, so nothing else needs it.
    fn round_trip(&self, event: &pb::ChildEvent, call_id: u32) -> PyResult<ExtFunctionResult> {
        let mut transport = self.transport.borrow_mut();
        if let Err(err) = transport.send(event) {
            return Err(send_error_to_py(&err));
        }
        match transport.recv() {
            Incoming::Request(request) => match request.kind {
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
            },
            Incoming::Eof => Err(PyRuntimeError::new_err("parent disconnected during a host call")),
            Incoming::Malformed(msg) | Incoming::Fatal(msg) => Err(PyRuntimeError::new_err(format!(
                "transport error during a host call: {msg}"
            ))),
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
impl HostBridge {
    /// Runs a host function for an undefined name and returns its result, or
    /// raises whatever the parent reported (`NameError` for an unknown name).
    fn call(
        &self,
        py: Python<'_>,
        name: &str,
        args: &Bound<'_, PyTuple>,
        kwargs: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyAny>> {
        let mut wire_args = Vec::with_capacity(args.len());
        for arg in args.iter() {
            wire_args.push(self.to_wire(py, &arg)?);
        }
        let mut wire_kwargs = Vec::with_capacity(kwargs.len());
        for (key, value) in kwargs.iter() {
            let key: String = key
                .cast::<PyString>()
                .map_err(|_| PyValueError::new_err("keyword argument names must be strings"))?
                .extract()?;
            wire_kwargs.push((MontyObject::String(key), self.to_wire(py, &value)?));
        }

        let call_id = self.next_call_id.get();
        self.next_call_id.set(call_id.wrapping_add(1));
        let event = function_call_event(name.to_owned(), wire_args, wire_kwargs, call_id);

        match self.round_trip(&event, call_id)? {
            ExtFunctionResult::Return(obj) => monty_to_py(py, &obj, &self.dc),
            ExtFunctionResult::Error(exc) => Err(exc_monty_to_py(py, exc)),
            ExtFunctionResult::NotFound(name) => Err(PyNameError::new_err(format!("name '{name}' is not defined"))),
            ExtFunctionResult::Future(_) => Err(PyRuntimeError::new_err(
                "async host functions are not supported by the CPython worker",
            )),
        }
    }

    /// `sys.stdout.write`: stream a `print()` chunk as a `Print` event.
    fn write(&self, text: &str) -> PyResult<usize> {
        if !text.is_empty() {
            let event = print_event(text.to_owned());
            self.transport
                .borrow_mut()
                .send(&event)
                .map_err(|err| send_error_to_py(&err))?;
        }
        Ok(text.len())
    }

    /// `sys.stdout.flush`: a no-op — each write is already flushed to the parent.
    fn flush(&self) {
        let _ = self;
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
