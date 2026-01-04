//! Python bindings for the Monty sandboxed Python interpreter.
//!
//! This module provides a Python interface to Monty, allowing execution of
//! sandboxed Python code with configurable resource limits and external
//! function callbacks.

mod convert;
mod exceptions;
mod external;
mod limits;
mod monty_cls;

// Use `::monty` to refer to the external crate (not the pymodule)
use pyo3::prelude::*;

pub use monty_cls::{PyMonty, PyMontyComplete, PyMontySnapshot};

/// Monty - A sandboxed Python interpreter written in Rust.
#[pymodule]
mod monty {
    use pyo3::prelude::*;
    use pyo3::types::PyDict;

    #[pymodule_export]
    use super::PyMonty as Monty;

    #[pymodule_export]
    use super::PyMontySnapshot as MontySnapshot;

    #[pymodule_export]
    use super::PyMontyComplete as MontyComplete;

    /// Creates the ResourceLimits TypedDict and adds it to the module.
    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        let py = m.py();

        // Create ResourceLimits TypedDict by executing Python code
        let locals = PyDict::new(py);
        py.run(
            c"
from typing import TypedDict

class ResourceLimits(TypedDict, total=False):
    \"\"\"
    Configuration for resource limits during code execution.

    All limits are optional. Omit a key to disable that limit.
    \"\"\"
    max_allocations: int
    max_duration_secs: float
    max_memory: int
    gc_interval: int
    max_recursion_depth: int
",
            None,
            Some(&locals),
        )?;

        let resource_limits = locals.get_item("ResourceLimits")?.unwrap();
        m.add("ResourceLimits", resource_limits)?;

        Ok(())
    }
}
