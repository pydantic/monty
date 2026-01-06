//! External function callback support.
//!
//! Allows Python code running in Monty to call back to host Python functions.
//! External functions are registered by name and called when Monty execution
//! reaches a call to that function.

use ::monty::{ExternalResult, MontyObject};
use pyo3::{
    exceptions::PyKeyError,
    prelude::*,
    types::{PyDict, PyTuple},
};

use crate::{
    convert::{monty_to_py, py_to_monty},
    exceptions::exc_py_to_monty,
};

/// Registry that maps external function names to Python callables.
///
/// Passed to the execution loop and used to dispatch calls when Monty
/// execution pauses at an external function.
pub struct ExternalFunctionRegistry<'py> {
    py: Python<'py>,
    functions: &'py Bound<'py, PyDict>,
}

impl<'py> ExternalFunctionRegistry<'py> {
    /// Creates a new registry from a Python dict of `name -> callable`.
    pub fn new(py: Python<'py>, functions: &'py Bound<'py, PyDict>) -> Self {
        Self { py, functions }
    }

    /// Calls an external function by name with Monty arguments.
    ///
    /// Converts args/kwargs from Monty format, calls the Python callable
    /// with unpacked `*args, **kwargs`, and converts the result back to Monty format.
    ///
    /// If the Python function raises an exception, it's converted to a Monty
    /// exception that will be raised inside Monty execution.
    pub fn call(
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> ExternalResult {
        match self.call_inner(function_name, args, kwargs) {
            Ok(result) => ExternalResult::Return(result),
            Err(err) => ExternalResult::Error(exc_py_to_monty(self.py, err)),
        }
    }

    /// Inner implementation that returns `PyResult` for error handling.
    fn call_inner(
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> PyResult<MontyObject> {
        // Look up the callable
        let callable = self
            .functions
            .get_item(function_name)?
            .ok_or_else(|| PyErr::new::<PyKeyError, _>(format!("External function '{function_name}' not found")))?;

        // Convert positional arguments to Python objects
        let py_args: PyResult<Vec<Py<PyAny>>> = args.iter().map(|arg| monty_to_py(self.py, arg)).collect();
        let py_args_tuple = PyTuple::new(self.py, py_args?)?;

        // Convert keyword arguments to Python dict
        let py_kwargs = PyDict::new(self.py);
        for (key, value) in kwargs {
            // Keys in kwargs should be strings
            let py_key = monty_to_py(self.py, key)?;
            let py_value = monty_to_py(self.py, value)?;
            py_kwargs.set_item(py_key, py_value)?;
        }

        // Call the function with unpacked *args, **kwargs
        let result = if py_kwargs.is_empty() {
            callable.call1(&py_args_tuple)?
        } else {
            callable.call(&py_args_tuple, Some(&py_kwargs))?
        };

        // Convert result back to Monty format
        py_to_monty(&result)
    }
}
