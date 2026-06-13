//! External function callback support.
//!
//! Allows Python code running in Monty to call back to host Python functions.
//! External functions are registered by name and called when Monty execution
//! reaches a call to that function.

use ::monty::{ExtFunctionResult, MontyObject};
use pyo3::{
    exceptions::{PyAttributeError, PyRuntimeError},
    prelude::*,
    types::{PyDict, PyTuple},
};

use crate::{
    convert::{monty_to_py, py_to_monty, py_to_monty_value},
    dataclass::DcRegistry,
    exceptions::exc_py_to_monty,
};

/// Dispatches a dataclass method call back to the original Python object.
///
/// The first arg is the dataclass `self`; this converts it back to Python,
/// calls `getattr(self, name)(*rest, **kwargs)`, and converts the result back
/// to Monty format.
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

/// `PyResult`-returning core of [`dispatch_method_call`].
fn dispatch_method_call_inner(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> PyResult<MontyObject> {
    validate_host_method_name(function_name)?;
    // First arg is the dataclass self.
    let mut args_iter = args.iter();
    let self_obj = args_iter
        .next()
        .ok_or_else(|| PyRuntimeError::new_err("Method call missing self argument"))?;
    let py_self = monty_to_py(py, self_obj, dc_registry)?;

    let method = py_self.bind(py).getattr(function_name)?;

    let result = if args.len() == 1 && kwargs.is_empty() {
        method.call0()?
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
        method.call(&py_args_tuple, py_kwargs.as_ref())?
    };

    py_to_monty(&result, dc_registry, 0)
}

/// Maps external function names to Python callables, dispatching calls when
/// Monty pauses at an external function. Dataclass types in return values are
/// auto-registered into `dc_registry` transparently.
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

    /// Calls an external function by name, converting args/kwargs from Monty
    /// format and the result back. A raised exception becomes a Monty exception
    /// that will be re-raised inside Monty execution.
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

    /// `PyResult`-returning core of [`call`](Self::call); `Ok(None)` means the
    /// function name was not found.
    fn call_inner(
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> PyResult<Option<MontyObject>> {
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

        py_to_monty(&result, self.dc_registry, 0).map(Some)
    }

    /// Like [`call`](Self::call) but returns `CallResult::Coroutine` (for the
    /// async loop to spawn) when the callable returns a coroutine.
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

    /// Core of [`call_or_coroutine`](Self::call_or_coroutine), returning the raw
    /// Python result so the caller can check for a coroutine.
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

/// Result of calling a Python function with coroutine detection, letting the
/// async dispatch loop distinguish ready return values from coroutines to await.
pub enum CallResult {
    /// Synchronous result ready to resume the VM immediately.
    Sync(ExtFunctionResult),
    /// Python coroutine to convert via `pyo3_async_runtimes::into_future()` and
    /// spawn as a task.
    Coroutine(Py<PyAny>),
}

/// Like [`dispatch_method_call`] but returns `CallResult::Coroutine` when the
/// method returns a coroutine for the async loop to await.
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

/// Core of [`dispatch_method_call_or_coroutine`], returning the raw Python
/// result so the caller can check for a coroutine.
fn dispatch_method_call_inner_raw<'py>(
    py: Python<'py>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> PyResult<Bound<'py, PyAny>> {
    validate_host_method_name(function_name)?;
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

/// Rejects private/dunder method dispatch from a worker-controlled name.
fn validate_host_method_name(function_name: &str) -> PyResult<()> {
    if function_name.starts_with('_') {
        Err(PyAttributeError::new_err(format!(
            "host dataclass method '{function_name}' is not exposed"
        )))
    } else {
        Ok(())
    }
}

/// Wraps a Python result as `Coroutine` if it is one, else converts it to a
/// synchronous `ExtFunctionResult`.
fn result_to_call_result(py: Python<'_>, result: &Bound<'_, PyAny>, dc_registry: &DcRegistry) -> CallResult {
    if is_coroutine(py, result) {
        CallResult::Coroutine(result.clone().unbind())
    } else {
        match py_to_monty_value(result, dc_registry) {
            Ok(monty_obj) => CallResult::Sync(ExtFunctionResult::Return(monty_obj)),
            Err(exc) => CallResult::Sync(ExtFunctionResult::Error(exc)),
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

/// Converts an exception from a spawned async external function into an
/// `ExtFunctionResult` for the async dispatch loop.
pub fn py_err_to_ext_result(py: Python<'_>, err: &PyErr) -> ExtFunctionResult {
    ExtFunctionResult::Error(exc_py_to_monty(py, err))
}

/// Converts a successful async external function result into an
/// `ExtFunctionResult`. Routes conversion failures through `py_to_monty_value`
/// so a bad return value produces the same exception shape whether the function
/// was sync or async.
pub fn py_obj_to_ext_result(obj: &Bound<'_, PyAny>, dc_registry: &DcRegistry) -> ExtFunctionResult {
    match py_to_monty_value(obj, dc_registry) {
        Ok(monty_obj) => ExtFunctionResult::Return(monty_obj),
        Err(exc) => ExtFunctionResult::Error(exc),
    }
}
