#![doc = include_str!("../README.md")]

//! # Rust binding internals
//!
//! Execution always happens in `monty` worker subprocesses (via the
//! `monty-pool` crate): a monty process can never be made fully crash-proof
//! against memory errors triggered by adversarial input, so crash isolation
//! is not optional. [`PyMonty`] (`Monty`) drives workers synchronously;
//! [`PyAsyncMonty`] (`AsyncMonty`) drives them from an asyncio event loop and
//! supports coroutine external functions.

mod async_dispatch;
mod build;
pub mod exceptions;
mod external;
mod limits;
mod mount;
mod pool;
mod print_target;
mod snapshot;
mod version;

use std::sync::OnceLock;

pub use exceptions::{
    MontyConversionError, MontyCrashedError, MontyError, MontyRuntimeError, MontySyntaxError, MontyTypingError, PyFrame,
};
pub use mount::PyMountDir;
pub use pool::{PyAsyncMonty, PyAsyncMontySession, PyAsyncMontyWebsocket, PyMonty, PyMontySession};
pub use print_target::{PyCollectStreams, PyCollectString};
use pyo3::{prelude::*, sync::PyOnceLock, types::PyAny};
pub use snapshot::{
    MontyComplete, PyAsyncFunctionSnapshot, PyAsyncFutureSnapshot, PyAsyncNameLookupSnapshot, PyFunctionSnapshot,
    PyFutureSnapshot, PyNameLookupSnapshot,
};
use version::cargo_version_to_pep440;

/// The PEP 440 version exposed as `pydantic_monty.__version__`.
///
/// Copied from `get_pydantic_core_version` in pydantic.
fn get_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();

    VERSION.get_or_init(|| cargo_version_to_pep440(env!("CARGO_PKG_VERSION")))
}

/// Private Python object type used for the public `NOT_HANDLED` singleton.
///
/// Python OS callbacks return the singleton instance rather than creating fresh
/// values. The Rust bridge uses object identity to detect this sentinel and
/// apply the call's no-handler behavior.
#[pyclass(name = "_NotHandledSentinel", module = "pydantic_monty", frozen)]
struct NotHandledSentinel;

#[pymethods]
impl NotHandledSentinel {
    fn __repr__(&self) -> &'static str {
        let _ = self;
        "NOT_HANDLED"
    }
}

/// Returns the process-wide Python `NOT_HANDLED` singleton.
///
/// The singleton lives in Rust so callback dispatch can compare by identity
/// without importing Python helper modules. It is exported publicly from the
/// compiled `_monty` module and re-exported by the pure-Python package surface.
pub(crate) fn get_not_handled(py: Python<'_>) -> PyResult<&Py<PyAny>> {
    static NOT_HANDLED: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    NOT_HANDLED.get_or_try_init(py, || Py::new(py, NotHandledSentinel).map(Py::into_any))
}

/// Monty - A sandboxed Python interpreter written in Rust.
#[pymodule]
mod _monty {
    // `MontyFileHandle` is produced by the value-conversion layer (in
    // `monty_proto`, shared with the `monty-cpython` worker) whenever a
    // `MontyObject::FileHandle` crosses the boundary; export it as part of the
    // `pydantic_monty` surface.
    #[pymodule_export]
    use monty_proto::python::PyMontyFileHandle as MontyFileHandle;
    use pyo3::prelude::*;

    #[pymodule_export]
    use super::MontyComplete;
    #[pymodule_export]
    use super::MontyConversionError;
    #[pymodule_export]
    use super::MontyCrashedError;
    #[pymodule_export]
    use super::MontyError;
    #[pymodule_export]
    use super::MontyRuntimeError;
    #[pymodule_export]
    use super::MontySyntaxError;
    #[pymodule_export]
    use super::MontyTypingError;
    #[pymodule_export]
    use super::PyAsyncFunctionSnapshot as AsyncFunctionSnapshot;
    #[pymodule_export]
    use super::PyAsyncFutureSnapshot as AsyncFutureSnapshot;
    #[pymodule_export]
    use super::PyAsyncMonty as AsyncMonty;
    #[pymodule_export]
    use super::PyAsyncMontySession as AsyncMontySession;
    #[pymodule_export]
    use super::PyAsyncMontyWebsocket as AsyncMontyWebsocket;
    #[pymodule_export]
    use super::PyAsyncNameLookupSnapshot as AsyncNameLookupSnapshot;
    #[pymodule_export]
    use super::PyCollectStreams as CollectStreams;
    #[pymodule_export]
    use super::PyCollectString as CollectString;
    #[pymodule_export]
    use super::PyFrame as Frame;
    #[pymodule_export]
    use super::PyFunctionSnapshot as FunctionSnapshot;
    #[pymodule_export]
    use super::PyFutureSnapshot as FutureSnapshot;
    #[pymodule_export]
    use super::PyMonty as Monty;
    #[pymodule_export]
    use super::PyMontySession as MontySession;
    #[pymodule_export]
    use super::PyMountDir as MountDir;
    #[pymodule_export]
    use super::PyNameLookupSnapshot as NameLookupSnapshot;
    use super::{get_not_handled, get_version};

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        let py = m.py();
        m.add("__version__", get_version())?;
        m.add("NOT_HANDLED", get_not_handled(py)?.clone_ref(py))?;
        Ok(())
    }
}
