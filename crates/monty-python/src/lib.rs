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
    use pyo3::types::{PyDict, PyDictMethods};

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

        // Create the TypedDict using Python's typing module
        let typing = py.import("typing")?;
        let typed_dict = typing.getattr("TypedDict")?;

        // Get base types
        let builtins = py.import("builtins")?;
        let int_type = builtins.getattr("int")?;
        let float_type = builtins.getattr("float")?;

        // Define the fields with their types (all Optional)
        let fields = PyDict::new(py);
        fields.set_item("max_allocations", &int_type)?;
        fields.set_item("max_duration_secs", &float_type)?;
        fields.set_item("max_memory", &int_type)?;
        fields.set_item("gc_interval", &int_type)?;
        fields.set_item("max_recursion_depth", &int_type)?;

        // Create the TypedDict class: TypedDict('ResourceLimits', {...}, total=False)
        let kwargs = PyDict::new(py);
        kwargs.set_item("total", false)?;
        let resource_limits = typed_dict.call(("ResourceLimits", fields), Some(&kwargs))?;

        // Set the docstring
        resource_limits.setattr(
            "__doc__",
            "Configuration for resource limits during code execution.\n\n\
             All limits are optional. Omit a key to disable that limit.",
        )?;

        // Add to module
        m.add("ResourceLimits", resource_limits)?;

        Ok(())
    }
}
