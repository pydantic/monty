//! Resolving names a sandbox snippet leaves undefined against the session's
//! `external_lookup` dict, plus method calls and lazy attribute lookups on
//! host class instances.
//!
//! [`ExternalLookup`] owns both halves of the lazy-resolution protocol — the
//! `NameLookup` that resolves a bare name and the `FunctionCall` that invokes a
//! resolved host function — so the callable-vs-value rule linking them lives in
//! one place. Host-routed calls (`dispatch_object_call*`) and lazy attribute
//! lookups (`resolve_object_attr`) are a separate concern: they route through
//! the session's [`InstanceStore`] to the original wrapped object or class,
//! not `external_lookup`.

use monty_proto::python::{InstanceStore, exc_py_to_monty, monty_to_py, py_to_monty, py_to_monty_value};
use monty_types::{ExtFunctionResult, MontyObject, MontyUuid, NameLookupResult};
use pyo3::{
    exceptions::PyAttributeError,
    prelude::*,
    types::{PyDict, PyTuple},
};

use crate::exceptions::MontyConversionError;

/// Dispatches a host-routed call — a method on a host class instance, or on
/// a host class type (a classmethod, or construction spelled `__call__`) —
/// routed by `object_id` through the session's [`InstanceStore`] to the
/// wrapper's `call_method`. The receiver is NOT in `args`.
pub fn dispatch_object_call(
    py: Python<'_>,
    function_name: &str,
    object_id: &MontyUuid,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    instances: &InstanceStore,
) -> ExtFunctionResult {
    match dispatch_object_call_inner(py, function_name, object_id, args, kwargs, instances) {
        Ok(result) => ExtFunctionResult::Return(result),
        Err(err) => ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
    }
}

/// `PyResult`-returning core of [`dispatch_object_call`].
fn dispatch_object_call_inner(
    py: Python<'_>,
    function_name: &str,
    object_id: &MontyUuid,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    instances: &InstanceStore,
) -> PyResult<MontyObject> {
    let result = call_object_method_raw(py, function_name, object_id, args, kwargs, instances)?;
    py_to_monty(&result, instances, 0)
}

/// Converts the wire args/kwargs and invokes `wrapper.call_method` through the
/// store, returning the raw Python result (shared by the sync and coroutine
/// dispatch paths).
fn call_object_method_raw<'py>(
    py: Python<'py>,
    function_name: &str,
    object_id: &MontyUuid,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    instances: &InstanceStore,
) -> PyResult<Bound<'py, PyAny>> {
    validate_host_method_name(function_name)?;
    let (py_args_tuple, py_kwargs) = wire_call_arguments(py, args, kwargs, instances)?;
    instances
        .call_method(py, object_id, function_name, &py_args_tuple, &py_kwargs)
        .map(|obj| obj.into_bound(py))
}

/// Converts wire args/kwargs into the Python tuple/dict a host call needs.
pub(crate) fn wire_call_arguments<'py>(
    py: Python<'py>,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    instances: &InstanceStore,
) -> PyResult<(Bound<'py, PyTuple>, Bound<'py, PyDict>)> {
    let py_args: PyResult<Vec<Py<PyAny>>> = args.iter().map(|arg| monty_to_py(py, arg, instances)).collect();
    let py_args_tuple = PyTuple::new(py, py_args?)?;

    let py_kwargs = PyDict::new(py);
    for (key, value) in kwargs {
        let py_key = monty_to_py(py, key, instances)?;
        let py_value = monty_to_py(py, value, instances)?;
        py_kwargs.set_item(py_key, py_value)?;
    }
    Ok((py_args_tuple, py_kwargs))
}

/// Answers a lazy attribute lookup on a host-backed object (`NameLookup`
/// with an `object_id` — an instance, or a class type for lazy class attrs).
/// `Undefined` means "not exposed" — a store miss, an underscore name, or an
/// `AttributeError` from the wrapper's policy — and the sandbox raises
/// `AttributeError`. Any other exception, or a value that cannot convert, is
/// raised inside the sandbox exactly as a failing method call would be.
pub fn resolve_object_attr(
    py: Python<'_>,
    name: &str,
    object_id: &MontyUuid,
    instances: &InstanceStore,
) -> NameLookupResult {
    if name.starts_with('_') {
        // Defensive re-check of the sandbox's underscore rule; wire frames
        // from a (possibly compromised) child are untrusted.
        NameLookupResult::Undefined
    } else {
        match instances.lookup_lazy_attr(py, object_id, name) {
            Ok(Some(value)) => match py_to_monty_value(value.bind(py), instances) {
                Ok(obj) => NameLookupResult::Value(obj),
                Err(exc) => NameLookupResult::Error(exc),
            },
            Ok(None) => NameLookupResult::Undefined,
            Err(err) => NameLookupResult::Error(exc_py_to_monty(py, &err)),
        }
    }
}

/// The session's `external_lookup` dict (`name -> value`, absent when the
/// caller passed none) plus the `Python` token and instance store every
/// resolution needs. Owns both halves of the lazy-resolution protocol:
/// [`resolve_name`](Self::resolve_name) answers a `NameLookup`, and
/// [`call`](Self::call) / [`call_or_coroutine`](Self::call_or_coroutine)
/// answer the follow-up `FunctionCall` by invoking the current dict entry —
/// which may have been replaced since it resolved, so calling a now
/// non-callable entry raises `TypeError` exactly as CPython would.
/// `ClassInstance` wrappers in return values register in `instances`
/// transparently.
pub struct ExternalLookup<'a, 'py> {
    py: Python<'py>,
    lookup: Option<&'py Bound<'py, PyDict>>,
    instances: &'a InstanceStore,
}

impl<'a, 'py> ExternalLookup<'a, 'py> {
    /// Wraps the `external_lookup` dict (`None` when the caller passed none, in
    /// which case every name resolves to `NameError` / `NotFound`).
    pub fn new(py: Python<'py>, lookup: Option<&'py Bound<'py, PyDict>>, instances: &'a InstanceStore) -> Self {
        Self { py, lookup, instances }
    }

    /// Resolves a bare-name lookup (a `NameLookup` event): a plain callable
    /// becomes a host function proxy invoked on the eventual `FunctionCall`,
    /// any other value is converted and returned directly, and an absent name
    /// (or absent dict) yields `None` → the sandbox raises `NameError`.
    ///
    /// [`py_to_monty_value`] decides callable-vs-other (notably a type object
    /// Monty models converts to `MontyObject::Type`, not a proxy); a function
    /// proxy is renamed to the lookup *key* (not the callable's `__name__`) so
    /// the `FunctionCall` hits the same dict entry. An unconvertible value
    /// rejects the turn via [`MontyConversionError::value_conversion_err`] —
    /// because `external_lookup` may hold untrusted values, an unrepresentable
    /// type surfaces as the dedicated `MontyConversionError` (a `MontyError`),
    /// not a masquerading `NameError`.
    pub fn resolve_name(&self, name: &str) -> PyResult<Option<MontyObject>> {
        let Some(lookup) = self.lookup else {
            return Ok(None);
        };
        let Some(value) = lookup.get_item(name)? else {
            return Ok(None);
        };
        let obj = match py_to_monty_value(&value, self.instances)
            .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc))?
        {
            MontyObject::Function { docstring, .. } => MontyObject::Function {
                name: name.to_owned(),
                docstring,
            },
            other => other,
        };
        Ok(Some(obj))
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
    /// name was not found (an absent dict or an absent key).
    fn call_inner(
        &self,
        function_name: &str,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> PyResult<Option<MontyObject>> {
        let Some(lookup) = self.lookup else {
            return Ok(None);
        };
        let Some(callable) = lookup.get_item(function_name)? else {
            return Ok(None);
        };
        let (py_args_tuple, py_kwargs) = wire_call_arguments(self.py, args, kwargs, self.instances)?;
        let result = callable.call(&py_args_tuple, Some(&py_kwargs))?;
        py_to_monty(&result, self.instances, 0).map(Some)
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
            Ok(Some(result)) => result_to_call_result(self.py, &result, self.instances),
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
        let Some(lookup) = self.lookup else {
            return Ok(None);
        };
        let Some(callable) = lookup.get_item(function_name)? else {
            return Ok(None);
        };
        let (py_args_tuple, py_kwargs) = wire_call_arguments(self.py, args, kwargs, self.instances)?;
        callable.call(&py_args_tuple, Some(&py_kwargs)).map(Some)
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

/// Like [`dispatch_object_call`] but returns `CallResult::Coroutine` when
/// the method returns a coroutine for the async loop to await.
pub fn dispatch_object_call_or_coroutine(
    py: Python<'_>,
    function_name: &str,
    object_id: &MontyUuid,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    instances: &InstanceStore,
) -> CallResult {
    match call_object_method_raw(py, function_name, object_id, args, kwargs, instances) {
        Ok(result) => result_to_call_result(py, &result, instances),
        Err(err) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(py, &err))),
    }
}

/// Rejects private/dunder method dispatch from a worker-controlled name.
///
/// `__call__` is the one dunder the sandbox legitimately suspends on (calling
/// a host object; the wrapper's own policy still gates it). Otherwise the
/// sandbox never suspends on `_`-prefixed names, so seeing one here means the
/// frame is forged; wire frames from a child are untrusted.
fn validate_host_method_name(function_name: &str) -> PyResult<()> {
    if function_name.starts_with('_') && function_name != "__call__" {
        Err(PyAttributeError::new_err(format!(
            "host method '{function_name}' is not exposed"
        )))
    } else {
        Ok(())
    }
}

/// Wraps a Python result as `Coroutine` if it is one, else converts it to a
/// synchronous `ExtFunctionResult`.
fn result_to_call_result(py: Python<'_>, result: &Bound<'_, PyAny>, instances: &InstanceStore) -> CallResult {
    if is_coroutine(py, result) {
        CallResult::Coroutine(result.clone().unbind())
    } else {
        match py_to_monty_value(result, instances) {
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
pub fn py_obj_to_ext_result(obj: &Bound<'_, PyAny>, instances: &InstanceStore) -> ExtFunctionResult {
    match py_to_monty_value(obj, instances) {
        Ok(monty_obj) => ExtFunctionResult::Return(monty_obj),
        Err(exc) => ExtFunctionResult::Error(exc),
    }
}
