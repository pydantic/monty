//! External function callback support.
//!
//! Allows Python code running in Monty to call back to host Python functions.
//! External functions are registered by name and called when Monty execution
//! reaches a call to that function.

use ::monty::{ExtFunctionResult, MontyObject};
use pyo3::{
    exceptions::PyRuntimeError,
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
/// `FrameExit::MethodCall` containing the method name (e.g. `"my_method"`)
/// and the dataclass instance as the first arg. This function:
/// 1. Converts the first arg (dataclass `self`) back to a Python object
/// 2. Calls `getattr(self_obj, method_name)(*remaining_args, **kwargs)`
/// 3. Converts the result back to Monty format
pub fn dispatch_method_call(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> ExtFunctionResult {
    match dispatch_method_call_inner(py, function_name, args, kwargs, dc_registry) {
        Ok(result) => ExtFunctionResult::Return(result),
        Err(err) => ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
    }
}

/// Inner implementation of method dispatch that returns `PyResult` for error handling.
fn dispatch_method_call_inner(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> PyResult<MontyObject> {
    // First arg is the dataclass self
    let mut args_iter = args.iter();
    let self_obj = args_iter
        .next()
        .ok_or_else(|| PyRuntimeError::new_err("Method call missing self argument"))?;
    let py_self = monty_to_py(py, self_obj, dc_registry)?;

    // Get the method from the object
    let method = py_self.bind(py).getattr(function_name)?;

    let result = if args.len() == 1 && kwargs.is_empty() {
        method.call0()?
    } else {
        // Convert remaining positional arguments
        let remaining_args: PyResult<Vec<Py<PyAny>>> = args_iter.map(|arg| monty_to_py(py, arg, dc_registry)).collect();
        let py_args_tuple = PyTuple::new(py, remaining_args?)?;

        // Call the method
        let py_kwargs = if kwargs.is_empty() {
            None
        } else {
            // Convert keyword arguments
            let py_kwargs = PyDict::new(py);
            for (key, value) in kwargs {
                let py_key = monty_to_py(py, key, dc_registry)?;
                let py_value = monty_to_py(py, value, dc_registry)?;
                py_kwargs.set_item(py_key, py_value)?;
            }
            Some(py_kwargs)
        };
        method.call(&py_args_tuple, py_kwargs.as_ref())?
    };

    py_to_monty(&result, dc_registry)
}

/// Registry that maps external function names to Python callables.
///
/// Passed to the execution loop and used to dispatch calls when Monty
/// execution pauses at an external function. The `dc_registry` is a
/// GIL-protected `PyDict` wrapper, so auto-registration of dataclass types
/// encountered in return values is transparent to callers.
pub struct ExternalFunctionRegistry<'a, 'py> {
    py: Python<'py>,
    functions: &'py Bound<'py, PyDict>,
    dc_registry: &'a DcRegistry,
}

impl<'a, 'py> ExternalFunctionRegistry<'a, 'py> {
    /// Creates a new registry from a Python dict of `name -> callable`.
    pub fn new(py: Python<'py>, functions: &'py Bound<'py, PyDict>, dc_registry: &'a DcRegistry) -> Self {
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
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> ExtFunctionResult {
        match self.call_inner(function_name, args, kwargs) {
            Ok(Some(result)) => ExtFunctionResult::Return(result),
            Ok(None) => ExtFunctionResult::NotFound(function_name.to_owned()),
            Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
        }
    }

    /// Inner implementation that returns `PyResult` for error handling.
    fn call_inner(
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> PyResult<Option<MontyObject>> {
        // Look up the callable
        let Some(callable) = self.functions.get_item(function_name)? else {
            return Ok(None);
        };

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
        py_to_monty(&result, self.dc_registry).map(Some)
    }

    /// Calls an external function, detecting coroutines for async dispatch.
    ///
    /// Like `call()` but when the Python callable returns a coroutine, it is
    /// returned as `CallResult::Coroutine` instead of being converted to a
    /// `MontyObject`. This allows the async dispatch loop to spawn the
    /// coroutine as a tokio task.
    pub fn call_or_coroutine(
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> CallResult {
        match self.call_inner_raw(function_name, args, kwargs) {
            Ok(Some(result)) => result_to_call_result(self.py, &result, self.dc_registry),
            Ok(None) => CallResult::Sync(ExtFunctionResult::NotFound(function_name.to_owned())),
            Err(err) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err))),
        }
    }

    /// Inner implementation that calls the function and returns the raw Python result.
    fn call_inner_raw<'b>(
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> PyResult<Option<Bound<'b, PyAny>>>
    where
        'py: 'b,
    {
        let Some(callable) = self.functions.get_item(function_name)? else {
            return Ok(None);
        };

        let py_args: PyResult<Vec<Py<PyAny>>> = args
            .iter()
            .map(|arg| monty_to_py(self.py, arg, self.dc_registry))
            .collect();
        let py_args_tuple = PyTuple::new(self.py, py_args?)?;

        let py_kwargs = PyDict::new(self.py);
        for (key, value) in kwargs {
            let py_key = monty_to_py(self.py, key, self.dc_registry)?;
            let py_value = monty_to_py(self.py, value, self.dc_registry)?;
            py_kwargs.set_item(py_key, py_value)?;
        }

        let result = if py_kwargs.is_empty() {
            callable.call1(&py_args_tuple)?
        } else {
            callable.call(&py_args_tuple, Some(&py_kwargs))?
        };

        Ok(Some(result))
    }
}

/// Result of calling a Python function with coroutine detection.
///
/// Used by the async dispatch loop to distinguish synchronous return values
/// from Python coroutines that need to be awaited on the event loop.
pub enum CallResult {
    /// Synchronous result ready to resume the VM immediately.
    Sync(ExtFunctionResult),
    /// Python coroutine that needs to be awaited asynchronously.
    ///
    /// The coroutine should be converted to a Rust future via
    /// `pyo3_async_runtimes::into_future()` and spawned as a task.
    Coroutine(Py<PyAny>),
}

/// Dispatches a dataclass method call, detecting coroutines for async dispatch.
///
/// Like `dispatch_method_call()` but returns `CallResult::Coroutine` when the
/// method returns a Python coroutine, allowing the async dispatch loop to
/// await it on the event loop.
pub fn dispatch_method_call_or_coroutine(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> CallResult {
    match dispatch_method_call_inner_raw(py, function_name, args, kwargs, dc_registry) {
        Ok(result) => result_to_call_result(py, &result, dc_registry),
        Err(err) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(py, &err))),
    }
}

/// Inner implementation of method dispatch that returns the raw Python result.
fn dispatch_method_call_inner_raw<'py>(
    py: Python<'py>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> PyResult<Bound<'py, PyAny>> {
    let mut args_iter = args.iter();
    let self_obj = args_iter
        .next()
        .ok_or_else(|| PyRuntimeError::new_err("Method call missing self argument"))?;
    let py_self = monty_to_py(py, self_obj, dc_registry)?;

    let method = py_self.bind(py).getattr(function_name)?;

    if args.len() == 1 && kwargs.is_empty() {
        method.call0()
    } else {
        let remaining_args: PyResult<Vec<Py<PyAny>>> = args_iter.map(|arg| monty_to_py(py, arg, dc_registry)).collect();
        let py_args_tuple = PyTuple::new(py, remaining_args?)?;

        let py_kwargs = if kwargs.is_empty() {
            None
        } else {
            let py_kwargs = PyDict::new(py);
            for (key, value) in kwargs {
                let py_key = monty_to_py(py, key, dc_registry)?;
                let py_value = monty_to_py(py, value, dc_registry)?;
                py_kwargs.set_item(py_key, py_value)?;
            }
            Some(py_kwargs)
        };
        method.call(&py_args_tuple, py_kwargs.as_ref())
    }
}

/// Checks if a Python result is a coroutine and returns the appropriate `CallResult`.
fn result_to_call_result(py: Python<'_>, result: &Bound<'_, PyAny>, dc_registry: &DcRegistry) -> CallResult {
    // Check if the result is a coroutine using inspect.iscoroutine
    if is_coroutine(py, result) {
        CallResult::Coroutine(result.clone().unbind())
    } else {
        match py_to_monty(result, dc_registry) {
            Ok(monty_obj) => CallResult::Sync(ExtFunctionResult::Return(monty_obj)),
            Err(err) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(py, &err))),
        }
    }
}

/// Checks whether a Python object is a coroutine via `inspect.iscoroutine()`.
fn is_coroutine(py: Python<'_>, obj: &Bound<'_, PyAny>) -> bool {
    py.import("inspect")
        .and_then(|inspect| inspect.getattr("iscoroutine"))
        .and_then(|is_coro| is_coro.call1((obj,)))
        .and_then(|result| result.is_truthy())
        .unwrap_or(false)
}

/// Converts a Python exception from an async external function into an `ExtFunctionResult`.
///
/// Used by the async dispatch loop when a spawned coroutine raises an exception.
pub fn py_err_to_ext_result(py: Python<'_>, err: &PyErr) -> ExtFunctionResult {
    ExtFunctionResult::Error(exc_py_to_monty(py, err))
}

/// Converts a Python object from an async external function result into an `ExtFunctionResult`.
///
/// Used by the async dispatch loop when a spawned coroutine completes successfully.
/// Uses `exc_py_to_monty` for conversion errors, matching the sync path in
/// `result_to_call_result` so that the same bad return value produces the same
/// exception shape regardless of whether the external function was sync or async.
pub fn py_obj_to_ext_result(py: Python<'_>, obj: &Bound<'_, PyAny>, dc_registry: &DcRegistry) -> ExtFunctionResult {
    match py_to_monty(obj, dc_registry) {
        Ok(monty_obj) => ExtFunctionResult::Return(monty_obj),
        Err(err) => ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
    }
}
