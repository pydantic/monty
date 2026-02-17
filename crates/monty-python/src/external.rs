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
    dataclass::DcRegistry,
    exceptions::exc_py_to_monty,
};

/// Dispatches a dataclass method call back to the original Python object.
///
/// When Monty encounters a call like `dc.my_method(args)`, the VM pauses with a
/// `FrameExit::MethodCall` containing a qualified name like `"ClassName.method_name"`
/// and the dataclass instance as the first arg. This function:
/// 1. Splits the qualified name to get the method name
/// 2. Converts the first arg (dataclass `self`) back to a Python object
/// 3. Calls `getattr(self_obj, method_name)(*remaining_args, **kwargs)`
/// 4. Converts the result back to Monty format
pub fn dispatch_method_call(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &mut DcRegistry,
) -> ExternalResult {
    match dispatch_method_call_inner(py, function_name, args, kwargs, dc_registry) {
        Ok(result) => ExternalResult::Return(result),
        Err(err) => ExternalResult::Error(exc_py_to_monty(py, &err)),
    }
}

/// Inner implementation of method dispatch that returns `PyResult` for error handling.
fn dispatch_method_call_inner(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &mut DcRegistry,
) -> PyResult<MontyObject> {
    // Split "ClassName.method_name" to get the method name
    let method_name = function_name.split('.').next_back().unwrap_or(function_name);

    // First arg is the dataclass self
    let self_obj = args
        .first()
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("Method call missing self argument"))?;
    let py_self = monty_to_py(py, self_obj, dc_registry)?;

    // Get the method from the object
    let method = py_self.bind(py).getattr(method_name)?;

    // Convert remaining positional arguments
    let remaining_args: PyResult<Vec<Py<PyAny>>> =
        args[1..].iter().map(|arg| monty_to_py(py, arg, dc_registry)).collect();
    let py_args_tuple = PyTuple::new(py, remaining_args?)?;

    // Convert keyword arguments
    let py_kwargs = PyDict::new(py);
    for (key, value) in kwargs {
        let py_key = monty_to_py(py, key, dc_registry)?;
        let py_value = monty_to_py(py, value, dc_registry)?;
        py_kwargs.set_item(py_key, py_value)?;
    }

    // Call the method
    let result = if py_kwargs.is_empty() {
        method.call1(&py_args_tuple)?
    } else {
        method.call(&py_args_tuple, Some(&py_kwargs))?
    };

    py_to_monty(&result, dc_registry)
}

/// Registry that maps external function names to Python callables.
///
/// Passed to the execution loop and used to dispatch calls when Monty
/// execution pauses at an external function. The `dc_registry` is mutable
/// because converting the return value back to Monty format (`py_to_monty`)
/// may auto-register new dataclass types encountered in the result.
pub struct ExternalFunctionRegistry<'a, 'py> {
    py: Python<'py>,
    functions: &'py Bound<'py, PyDict>,
    dc_registry: &'a mut DcRegistry,
}

impl<'a, 'py> ExternalFunctionRegistry<'a, 'py> {
    /// Creates a new registry from a Python dict of `name -> callable`.
    pub fn new(py: Python<'py>, functions: &'py Bound<'py, PyDict>, dc_registry: &'a mut DcRegistry) -> Self {
        Self {
            py,
            functions,
            dc_registry,
        }
    }

    /// Calls an external function by name with Monty arguments.
    ///
    /// Converts args/kwargs from Monty format, calls the Python callable
    /// with unpacked `*args, **kwargs`, and converts the result back to Monty format.
    ///
    /// If the Python function raises an exception, it's converted to a Monty
    /// exception that will be raised inside Monty execution.
    pub fn call(
        &mut self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> ExternalResult {
        match self.call_inner(function_name, args, kwargs) {
            Ok(result) => ExternalResult::Return(result),
            Err(err) => ExternalResult::Error(exc_py_to_monty(self.py, &err)),
        }
    }

    /// Inner implementation that returns `PyResult` for error handling.
    fn call_inner(
        &mut self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> PyResult<MontyObject> {
        // Look up the callable
        let callable = self
            .functions
            .get_item(function_name)?
            .ok_or_else(|| PyKeyError::new_err(format!("External function '{function_name}' not found")))?;

        // Convert positional arguments to Python objects
        let py_args: PyResult<Vec<Py<PyAny>>> = args
            .iter()
            .map(|arg| monty_to_py(self.py, arg, self.dc_registry))
            .collect();
        let py_args_tuple = PyTuple::new(self.py, py_args?)?;

        // Convert keyword arguments to Python dict
        let py_kwargs = PyDict::new(self.py);
        for (key, value) in kwargs {
            // Keys in kwargs should be strings
            let py_key = monty_to_py(self.py, key, self.dc_registry)?;
            let py_value = monty_to_py(self.py, value, self.dc_registry)?;
            py_kwargs.set_item(py_key, py_value)?;
        }

        // Call the function with unpacked *args, **kwargs
        let result = if py_kwargs.is_empty() {
            callable.call1(&py_args_tuple)?
        } else {
            callable.call(&py_args_tuple, Some(&py_kwargs))?
        };

        // Convert result back to Monty format
        py_to_monty(&result, self.dc_registry)
    }
}
