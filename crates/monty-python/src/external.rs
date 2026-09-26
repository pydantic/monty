//! Resolving names a sandbox snippet leaves undefined against the session's
//! `external_lookup` dict, its imports against `external_modules`, plus
//! method calls and lazy attribute lookups on host class instances.
//!
//! [`ExternalLookup`] owns both halves of the lazy-resolution protocol — the
//! `NameLookup` that resolves a bare name and the `FunctionCall` that invokes a
//! resolved host function — so the callable-vs-value rule linking them lives in
//! one place, and the `__import__` call that binds a module next to the dotted
//! calls into it. Host-routed calls (`dispatch_object_call*`) and lazy
//! attribute lookups (`resolve_object_attr`) are a separate concern: they
//! route through the session's [`InstanceStore`] to the original wrapped
//! object or class, not `external_lookup`.

use monty_proto::python::{
    DecodedArena, InstanceStore, exc_py_to_monty, is_class_instance_wrapper, py_to_monty, py_to_monty_value,
};
use monty_types::{
    CallArgs, ExtFunctionResult, IMPORT_FUNCTION, MontyObject, MontyUuid, NameLookupResult,
    unstable::{self, MontyNode},
};
use pyo3::{
    exceptions::{PyAttributeError, PyTypeError},
    prelude::*,
    types::{PyDict, PyTuple},
};

use crate::{callback_context, exceptions::MontyConversionError};

/// Dispatches a host-routed call — a method on a host class instance, or on
/// a host class type (a classmethod, or construction spelled `__call__`) —
/// routed by `object_id` through the session's [`InstanceStore`] to the
/// wrapper's `call_method`. The receiver is NOT in `args`.
pub fn dispatch_object_call(
    py: Python<'_>,
    function_name: &str,
    object_id: &MontyUuid,
    args: &CallArgs,
    instances: &InstanceStore,
) -> ExtFunctionResult {
    match dispatch_object_call_inner(py, function_name, object_id, args, instances) {
        Ok(result) => ExtFunctionResult::Return(result),
        Err(err) => ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
    }
}

/// `PyResult`-returning core of [`dispatch_object_call`].
fn dispatch_object_call_inner(
    py: Python<'_>,
    function_name: &str,
    object_id: &MontyUuid,
    args: &CallArgs,
    instances: &InstanceStore,
) -> PyResult<MontyObject> {
    let result = call_object_method_raw(py, function_name, object_id, args, instances)?;
    py_to_monty(&result, instances)
}

/// Converts the wire args/kwargs and invokes `wrapper.call_method` through the
/// store, returning the raw Python result (shared by the sync and coroutine
/// dispatch paths).
fn call_object_method_raw<'py>(
    py: Python<'py>,
    function_name: &str,
    object_id: &MontyUuid,
    args: &CallArgs,
    instances: &InstanceStore,
) -> PyResult<Bound<'py, PyAny>> {
    validate_host_method_name(function_name)?;
    let (py_args_tuple, py_kwargs) = wire_call_arguments(py, args, instances)?;
    callback_context::call(py, || {
        instances.call_method(py, object_id, function_name, &py_args_tuple, &py_kwargs)
    })
    .map(|obj| obj.into_bound(py))
}

/// Converts a call's arguments into the Python tuple/dict a host call needs.
/// The arena is decoded once, so an object passed twice is one Python object.
pub(crate) fn wire_call_arguments<'py>(
    py: Python<'py>,
    args: &CallArgs,
    instances: &InstanceStore,
) -> PyResult<(Bound<'py, PyTuple>, Bound<'py, PyDict>)> {
    let (graph, arg_ids, kwarg_ids) = unstable::call_args_parts(args);
    let arena = DecodedArena::new(py, graph, instances)?;
    let py_args_tuple = PyTuple::new(py, arg_ids.iter().map(|id| arena.get(py, *id)))?;
    let py_kwargs = PyDict::new(py);
    for (key, value) in kwarg_ids {
        py_kwargs.set_item(arena.get(py, *key), arena.get(py, *value))?;
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
        match callback_context::call(py, || instances.lookup_lazy_attr(py, object_id, name)) {
            Ok(Some(value)) => match py_to_monty_value(value.bind(py), instances) {
                Ok(obj) => NameLookupResult::Value(obj),
                Err(exc) => NameLookupResult::Error(exc),
            },
            Ok(None) => NameLookupResult::Undefined,
            Err(err) => NameLookupResult::Error(exc_py_to_monty(py, &err)),
        }
    }
}

/// The `external_lookup=` and `external_modules=` dicts one feed captured:
/// the host side of the names and imports a snippet leaves to it. Held as
/// owned references so a snapshot can keep them for `resume_auto`.
#[derive(Default)]
pub(crate) struct HostNames {
    /// `external_lookup=`: host values by the bare name the snippet reads.
    pub(crate) lookup: Option<Py<PyDict>>,
    /// `external_modules=`: module-like host values by the name the snippet
    /// imports.
    pub(crate) modules: Option<Py<PyDict>>,
}

impl HostNames {
    /// Captures the dicts a feed was called with.
    pub(crate) fn capture(lookup: Option<&Bound<'_, PyDict>>, modules: Option<&Bound<'_, PyDict>>) -> Self {
        Self {
            lookup: lookup.map(|d| d.clone().unbind()),
            modules: modules.map(|d| d.clone().unbind()),
        }
    }

    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            lookup: self.lookup.as_ref().map(|d| d.clone_ref(py)),
            modules: self.modules.as_ref().map(|d| d.clone_ref(py)),
        }
    }
}

/// The session's `external_lookup` and `external_modules` dicts (absent when
/// the caller passed none) plus the `Python` token and instance store every
/// resolution needs. Owns both halves of the lazy-resolution protocol:
/// [`resolve_name`](Self::resolve_name) answers a `NameLookup`, and
/// [`call`](Self::call) / [`call_or_coroutine`](Self::call_or_coroutine)
/// answer the follow-up `FunctionCall` by invoking the current dict entry —
/// which may have been replaced since it resolved, so calling a now
/// non-callable entry raises `TypeError` exactly as CPython would. The same
/// two methods answer an `import` (the `__import__` call) from
/// `external_modules`, and a dotted name (`tools.add`) as that module's
/// attribute. `ClassInstance` wrappers in return values register in
/// `instances` transparently.
pub struct ExternalLookup<'a, 'py> {
    py: Python<'py>,
    lookup: Option<&'py Bound<'py, PyDict>>,
    modules: Option<&'py Bound<'py, PyDict>>,
    instances: &'a InstanceStore,
}

impl<'a, 'py> ExternalLookup<'a, 'py> {
    /// Binds the captured dicts (each `None` when the caller passed none, in
    /// which case every name resolves to `NameError` / `NotFound`, and every
    /// import to `ModuleNotFoundError`).
    pub(crate) fn new(py: Python<'py>, names: &'py HostNames, instances: &'a InstanceStore) -> Self {
        Self {
            py,
            lookup: names.lookup.as_ref().map(|d| d.bind(py)),
            modules: names.modules.as_ref().map(|d| d.bind(py)),
            instances,
        }
    }

    /// Resolves a bare-name lookup (a `NameLookup` event): a plain callable
    /// becomes a host function proxy invoked on the eventual `FunctionCall`,
    /// any other value is converted and returned directly, and an absent name
    /// (or absent dict) yields `None` → the sandbox raises `NameError`.
    ///
    /// [`py_to_monty_value`] decides callable-vs-other (notably a type object
    /// Monty models converts to `MontyNode::Type`, not a proxy); a function
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
        let value = py_to_monty_value(&value, self.instances)
            .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc))?;
        let (mut graph, root) = unstable::into_graph_parts(value);
        if let MontyNode::Function { name: proxy_name, .. } = graph.node_mut(root) {
            name.clone_into(proxy_name);
        }
        Ok(Some(unstable::object_from_graph(graph, root).expect("root unchanged")))
    }

    /// Calls an external function by name, converting args/kwargs from Monty
    /// format and the result back. A raised exception becomes a Monty exception
    /// that will be re-raised inside Monty execution. An `import` (the
    /// `__import__` call) is answered from `external_modules` instead.
    pub fn call(&self, function_name: &str, args: &CallArgs) -> ExtFunctionResult {
        if function_name == IMPORT_FUNCTION {
            return self.import_module(args);
        }
        match self.call_inner(function_name, args) {
            Ok(Some(result)) => ExtFunctionResult::Return(result),
            Ok(None) => ExtFunctionResult::NotFound(function_name.to_owned()),
            Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
        }
    }

    /// `PyResult`-returning core of [`call`](Self::call); `Ok(None)` means the
    /// name was not found (an absent dict or an absent key).
    fn call_inner(&self, function_name: &str, args: &CallArgs) -> PyResult<Option<MontyObject>> {
        let Some(callable) = self.callable(function_name)? else {
            return Ok(None);
        };
        let (py_args_tuple, py_kwargs) = wire_call_arguments(self.py, args, self.instances)?;
        let result = callback_context::call(self.py, || callable.call(&py_args_tuple, Some(&py_kwargs)))?;
        py_to_monty(&result, self.instances).map(Some)
    }

    /// Like [`call`](Self::call) but returns `CallResult::Coroutine` (for the
    /// async loop to spawn) when the callable returns a coroutine.
    pub fn call_or_coroutine(&self, function_name: &str, args: &CallArgs) -> CallResult {
        if function_name == IMPORT_FUNCTION {
            return CallResult::Sync(self.import_module(args));
        }
        match self.call_inner_raw(function_name, args) {
            Ok(Some(result)) => result_to_call_result(self.py, &result, self.instances),
            Ok(None) => CallResult::Sync(ExtFunctionResult::NotFound(function_name.to_owned())),
            Err(err) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err))),
        }
    }

    /// Core of [`call_or_coroutine`](Self::call_or_coroutine), returning the raw
    /// Python result so the caller can check for a coroutine.
    fn call_inner_raw<'b>(&self, function_name: &str, args: &CallArgs) -> PyResult<Option<Bound<'b, PyAny>>>
    where
        'py: 'b,
    {
        let Some(callable) = self.callable(function_name)? else {
            return Ok(None);
        };
        let (py_args_tuple, py_kwargs) = wire_call_arguments(self.py, args, self.instances)?;
        callback_context::call(self.py, || callable.call(&py_args_tuple, Some(&py_kwargs))).map(Some)
    }

    /// The host callable `function_name` names: an entry of `external_lookup`,
    /// or, for a dotted name, that attribute of the `external_modules` entry
    /// (a host function bound by an import is named `<module>.<attr>`).
    /// `None` when neither has it.
    fn callable(&self, function_name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        if let Some((module, attr)) = function_name.split_once('.') {
            match self.module(module)? {
                Some(module) => module_attr(&module, attr),
                None => Ok(None),
            }
        } else {
            match self.lookup {
                Some(lookup) => lookup.get_item(function_name),
                None => Ok(None),
            }
        }
    }

    /// The `external_modules` entry for `name`, if any.
    fn module(&self, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        match self.modules {
            Some(modules) => modules.get_item(name),
            None => Ok(None),
        }
    }

    /// Answers the `__import__` call of `import <module>`: the
    /// `external_modules` entry of that name as a host object, or not found,
    /// which the sandbox raises as `ModuleNotFoundError`.
    fn import_module(&self, args: &CallArgs) -> ExtFunctionResult {
        let Some(name) = args.args().next().and_then(|arg| arg.as_str()) else {
            return ExtFunctionResult::NotFound(IMPORT_FUNCTION.to_owned());
        };
        let module = self
            .module(name)
            .and_then(|module| module.map(|module| self.module_value(name, &module)).transpose());
        match module {
            Ok(Some(value)) => ExtFunctionResult::Return(value),
            Ok(None) => ExtFunctionResult::NotFound(IMPORT_FUNCTION.to_owned()),
            Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
        }
    }

    /// The sandbox value of an `external_modules` entry. A `ClassInstance`
    /// wrapper crosses as itself, its methods routing back by uuid; anything
    /// else — a dict, a module, a namespace — becomes a host object named
    /// after the module whose public attributes are sent eagerly: callables as
    /// host functions named `<module>.<attr>`, other values converted.
    fn module_value(&self, name: &str, module: &Bound<'py, PyAny>) -> PyResult<MontyObject> {
        if is_class_instance_wrapper(module)? {
            return py_to_monty_value(module, self.instances)
                .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc));
        }
        let mut attrs = Vec::new();
        for (attr, value) in module_attrs(module)? {
            let value = if value.is_callable() {
                MontyObject::function(format!("{name}.{attr}"), None)
            } else {
                py_to_monty_value(&value, self.instances)
                    .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc))?
            };
            attrs.push((MontyObject::string(attr), value));
        }
        let class = MontyObject::class_type(name, module_uuid("class", name), true, false, []);
        Ok(MontyObject::class_instance(class, module_uuid("instance", name), attrs))
    }
}

/// The public attribute `attr` of an `external_modules` entry: a dict's item,
/// or any other object's attribute; `None` when absent.
fn module_attr<'py>(module: &Bound<'py, PyAny>, attr: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    if attr.starts_with('_') {
        Ok(None)
    } else if let Ok(dict) = module.cast::<PyDict>() {
        dict.get_item(attr)
    } else {
        match module.getattr(attr) {
            Ok(value) => Ok(Some(value)),
            Err(err) if err.is_instance_of::<PyAttributeError>(module.py()) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// The public attributes of an `external_modules` entry, in order: a dict's
/// items (which must have `str` keys), or `dir()` of any other object.
fn module_attrs<'py>(module: &Bound<'py, PyAny>) -> PyResult<Vec<(String, Bound<'py, PyAny>)>> {
    let mut attrs = Vec::new();
    if let Ok(dict) = module.cast::<PyDict>() {
        for (key, value) in dict.iter() {
            let Ok(key) = key.extract::<String>() else {
                return Err(PyTypeError::new_err("external_modules entries must have str keys"));
            };
            if !key.starts_with('_') {
                attrs.push((key, value));
            }
        }
    } else {
        for name in module.dir()?.iter() {
            let name: String = name.extract()?;
            if !name.starts_with('_') {
                let value = module.getattr(name.as_str())?;
                attrs.push((name, value));
            }
        }
    }
    Ok(attrs)
}

/// A stable identity for the host object standing in for module `name`, so a
/// second `import` of it — or one after a dump is restored — is the same
/// object: FNV-1a over `kind:name`, folded into the two uuid halves.
fn module_uuid(kind: &str, name: &str) -> MontyUuid {
    let fnv = |seed: u64| {
        let mut hash = seed;
        for byte in kind.bytes().chain(*b":").chain(name.bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash
    };
    MontyUuid::from_u128((u128::from(fnv(0xcbf2_9ce4_8422_2325)) << 64) | u128::from(fnv(0x8422_2325_cbf2_9ce4)))
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
    args: &CallArgs,
    instances: &InstanceStore,
) -> CallResult {
    match call_object_method_raw(py, function_name, object_id, args, instances) {
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
pub(crate) fn is_coroutine(py: Python<'_>, obj: &Bound<'_, PyAny>) -> bool {
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
