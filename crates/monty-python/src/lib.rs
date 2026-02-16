//! Python bindings for the Monty sandboxed Python interpreter.
//!
//! This module provides a Python interface to Monty, allowing execution of
//! sandboxed Python code with configurable resource limits and external
//! function callbacks.

mod convert;
mod dataclass;
mod exceptions;
mod external;
mod limits;
mod monty_cls;

use std::sync::OnceLock;

// Use `::monty` to refer to the external crate (not the pymodule)
pub use exceptions::{MontyError, MontyRuntimeError, MontySyntaxError, MontyTypingError, PyFrame};
pub use monty_cls::{PyMonty, PyMontyComplete, PyMontyFutureSnapshot, PyMontyRepl, PyMontySnapshot};
use pyo3::prelude::*;

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

/// Monty - A sandboxed Python interpreter written in Rust.
#[pymodule]
mod _monty {
    use pyo3::prelude::*;

    #[pymodule_export]
    use super::MontyError;
    #[pymodule_export]
    use super::MontyRuntimeError;
    #[pymodule_export]
    use super::MontySyntaxError;
    #[pymodule_export]
    use super::MontyTypingError;
    #[pymodule_export]
    use super::PyFrame as Frame;
    #[pymodule_export]
    use super::PyMonty as Monty;
    #[pymodule_export]
    use super::PyMontyComplete as MontyComplete;
    #[pymodule_export]
    use super::PyMontyFutureSnapshot as MontyFutureSnapshot;
    #[pymodule_export]
    use super::PyMontyRepl as MontyRepl;
    #[pymodule_export]
    use super::PyMontySnapshot as MontySnapshot;
    use super::get_version;

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("__version__", get_version())?;
        Ok(())
    }
}
