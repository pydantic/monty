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

// Use `::monty` to refer to the external crate (not the pymodule)
pub use monty_cls::{PyMonty, PyMontyComplete, PyMontySnapshot};
use pyo3::prelude::*;

/// Monty - A sandboxed Python interpreter written in Rust.
#[pymodule]
mod monty {
    use pyo3::prelude::*;

    #[pymodule_export]
    use super::PyMonty as Monty;
    #[pymodule_export]
    use super::PyMontyComplete as MontyComplete;
    #[pymodule_export]
    use super::PyMontySnapshot as MontySnapshot;
    use crate::limits::create_resource_limits_class;

    /// Registers the ResourceLimits TypedDict in the module.
    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("ResourceLimits", create_resource_limits_class(m.py())?)?;
        Ok(())
    }
}
