//! Python bindings for the Monty sandboxed Python interpreter.
//!
//! Execution always happens in `monty` worker subprocesses (via the
//! `monty-pool` crate): a monty process can never be made fully crash-proof
//! against memory errors triggered by adversarial input, so crash isolation
//! is not optional. [`PyMonty`] (`Monty`) drives workers synchronously;
//! [`PyAsyncMonty`] (`AsyncMonty`) drives them from an asyncio event loop and
//! supports coroutine external functions.

mod async_dispatch;
mod build;
mod convert;
mod dataclass;
mod exceptions;
mod external;
mod limits;
mod mount;
mod pool;
mod print_target;

use std::sync::OnceLock;

// Use `::monty` to refer to the external crate (not the pymodule)
pub use convert::PyMontyFileHandle;
pub use exceptions::{MontyCrashedError, MontyError, MontyRuntimeError, MontySyntaxError, MontyTypingError, PyFrame};
pub use mount::PyMountDir;
pub use pool::{PyAsyncMonty, PyAsyncMontySession, PyMonty, PyMontySession};
pub use print_target::{PyCollectStreams, PyCollectString};
use pyo3::{prelude::*, sync::PyOnceLock, types::PyAny};

/// Copied from `get_pydantic_core_version` in pydantic
fn get_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();

    VERSION.get_or_init(|| {
        let version = env!("CARGO_PKG_VERSION");
        // cargo uses "1.0-alpha1" etc. while python uses "1.0.0a1", this is not full compatibility,
        // but it's good enough for now
        // see https://docs.rs/semver/1.0.9/semver/struct.Version.html#method.parse for rust spec
        // see https://peps.python.org/pep-0440/ for python spec
        // it seems the dot after "alpha/beta" e.g. "-alpha.1" is not necessary, hence why this works
        version.replace("-alpha", "a").replace("-beta", "b")
    })
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
    use pyo3::prelude::*;

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
    use super::PyAsyncMonty as AsyncMonty;
    #[pymodule_export]
    use super::PyAsyncMontySession as AsyncMontySession;
    #[pymodule_export]
    use super::PyCollectStreams as CollectStreams;
    #[pymodule_export]
    use super::PyCollectString as CollectString;
    #[pymodule_export]
    use super::PyFrame as Frame;
    #[pymodule_export]
    use super::PyMonty as Monty;
    #[pymodule_export]
    use super::PyMontyFileHandle as MontyFileHandle;
    #[pymodule_export]
    use super::PyMontySession as MontySession;
    #[pymodule_export]
    use super::PyMountDir as MountDir;
    use super::{get_not_handled, get_version};

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        let py = m.py();
        m.add("__version__", get_version())?;
        m.add("NOT_HANDLED", get_not_handled(py)?.clone_ref(py))?;
        Ok(())
    }
}
