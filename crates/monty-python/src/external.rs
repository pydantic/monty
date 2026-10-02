//! Resolving names a sandbox snippet leaves undefined against the session's
//! `external_lookup` dict, its imports against `external_modules`, plus
//! method calls and lazy attribute lookups on host class instances.
//!
//! [`ExternalLookup`] owns both halves of the lazy-resolution protocol — the
//! `NameLookup` that resolves a bare name and the `FunctionCall` that invokes a
//! resolved host function — so the callable-vs-value rule linking them lives in
//! one place. It also answers the `__import__` call that binds a module and
//! the dotted calls (`tools.add`) into it. Host-routed calls
//! (`dispatch_object_call*`) and lazy attribute lookups (`resolve_object_attr`)
//! are a separate concern: they route through the session's [`InstanceStore`]
//! to the original wrapped object or class, not `external_lookup`.

use monty_proto::python::{
    DecodedArena, InstanceStore, exc_py_to_monty, is_class_instance_wrapper, py_to_monty, py_to_monty_value,
};
use monty_types::{
    CallArgs, ExtFunctionResult, IMPORT_FUNCTION, MontyObject, MontyUuid, NameLookupResult,
    unstable::{self, MontyNode},
};
use pyo3::{
    exceptions::{PyAttributeError, PyRuntimeError, PyTypeError},
    prelude::*,
    types::{PyDict, PyModule, PyString, PyTuple, PyType},
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
    /// What each factory entry of `modules` returned, by module name: a
    /// factory runs once per feed, at the first import or call that needs it.
    pub(crate) resolved_modules: Option<Py<PyDict>>,
}

impl HostNames {
    /// Captures the dicts a feed was called with. Every `external_modules`
    /// entry must be a dict, a module, a `ClassInstance` or a zero-argument
    /// callable returning one: those are the shapes whose public attributes
    /// are deliberately a module's, where `dir()` of an arbitrary object would
    /// expose whatever it happens to carry.
    pub(crate) fn capture(lookup: Option<&Bound<'_, PyDict>>, modules: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        if let Some(modules) = modules {
            for (name, module) in modules.iter() {
                check_module_entry(&name, &module)?;
            }
        }
        Ok(Self {
            lookup: lookup.map(|d| d.clone().unbind()),
            modules: modules.map(|d| d.clone().unbind()),
            resolved_modules: modules.map(|d| PyDict::new(d.py()).unbind()),
        })
    }

    /// A second owner of the same dicts, for a snapshot's drive context.
    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            lookup: self.lookup.as_ref().map(|d| d.clone_ref(py)),
            modules: self.modules.as_ref().map(|d| d.clone_ref(py)),
            resolved_modules: self.resolved_modules.as_ref().map(|d| d.clone_ref(py)),
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
/// non-callable entry raises `TypeError` exactly as CPython would. They also
/// answer `__import__` from `external_modules`, and a dotted name
/// (`tools.add`) as that module's attribute. `ClassInstance` wrappers in
/// return values register in `instances` transparently.
pub struct ExternalLookup<'a, 'py> {
    py: Python<'py>,
    lookup: Option<&'py Bound<'py, PyDict>>,
    modules: Option<&'py Bound<'py, PyDict>>,
    resolved_modules: Option<&'py Bound<'py, PyDict>>,
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
            resolved_modules: names.resolved_modules.as_ref().map(|d| d.bind(py)),
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
            return self.import_module_or_coroutine(args);
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
    /// (a host function bound by an import is named `<module>.<attr>`). Module
    /// names and dict keys may both contain dots, so the module is the longest
    /// prefix `external_modules` has. `None` when neither has it.
    fn callable(&self, function_name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        if function_name.contains('.') {
            for (dot, _) in function_name.rmatch_indices('.') {
                if let Some(module) = self.module(&function_name[..dot])? {
                    // a `ClassInstance` module routes its calls by uuid under the wrapper's
                    // policy, so a name-based call into it (which only a non-conforming
                    // worker sends) finds nothing
                    return if is_class_instance_wrapper(&module)? {
                        Ok(None)
                    } else {
                        module_attr(&module, &function_name[dot + 1..])
                    };
                }
            }
            Ok(None)
        } else {
            match self.lookup {
                Some(lookup) => lookup.get_item(function_name),
                None => Ok(None),
            }
        }
    }

    /// The `external_modules` entry for `name`, if any, with a factory's
    /// result installed; a factory that returned a coroutine is an error here,
    /// since only [`import_module_or_coroutine`](Self::import_module_or_coroutine)
    /// can have it awaited.
    fn module(&self, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        match self.resolve_module(name)? {
            Some(Resolved::Module(module)) => Ok(Some(module)),
            Some(Resolved::Coroutine(coro)) => Err(sync_module_coroutine_error(name, &coro)),
            None => Ok(None),
        }
    }

    /// The `external_modules` entry for `name`, if any. A factory entry is
    /// called the first time the feed needs the module and its result kept
    /// in `resolved_modules`, so an import, a re-import and the calls of the
    /// module's functions all see one module; a coroutine it returns is handed
    /// back for the async loop to await and [`install_module`](Self::install_module).
    fn resolve_module(&self, name: &str) -> PyResult<Option<Resolved<'py>>> {
        let (Some(modules), Some(resolved)) = (self.modules, self.resolved_modules) else {
            return Ok(None);
        };
        let Some(entry) = modules.get_item(name)? else {
            return Ok(None);
        };
        if !entry.is_callable() {
            return Ok(Some(Resolved::Module(entry)));
        }
        if let Some(module) = resolved.get_item(name)? {
            return Ok(Some(Resolved::Module(module)));
        }
        let result = callback_context::call(self.py, || entry.call0())?;
        if is_coroutine(self.py, &result) {
            Ok(Some(Resolved::Coroutine(result)))
        } else {
            self.install_module(name, result).map(Resolved::Module).map(Some)
        }
    }

    /// Keeps what module `name`'s factory produced for the rest of the feed,
    /// refusing anything that is not a module shape.
    fn install_module(&self, name: &str, module: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        if !is_module_shape(&module)? {
            return Err(PyTypeError::new_err(format!(
                "external_modules['{name}']() returned {}, not a dict, a module or a ClassInstance",
                module.get_type().name()?
            )));
        }
        self.resolved_modules
            .expect("a module was resolved, so the cache exists")
            .set_item(name, &module)?;
        Ok(module)
    }

    /// Finishes an import whose factory returned a coroutine, once the async
    /// loop has awaited it to `result`: the module is installed and sent as
    /// [`import_module`](Self::import_module) would have sent it.
    pub(crate) fn finish_import(&self, name: &str, result: PyResult<Bound<'py, PyAny>>) -> ExtFunctionResult {
        match result.and_then(|module| self.install_module(name, module)) {
            Ok(module) => match self.module_value(name, &module) {
                Ok(value) => ExtFunctionResult::Return(value),
                Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
            },
            Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
        }
    }

    /// [`import_module`](Self::import_module) for the async loop: a factory's
    /// coroutine comes back as [`CallResult::ModuleCoroutine`] to await.
    fn import_module_or_coroutine(&self, args: &CallArgs) -> CallResult {
        let Some(name) = args.args().next().and_then(|arg| arg.as_str()) else {
            return CallResult::Sync(ExtFunctionResult::NotFound(IMPORT_FUNCTION.to_owned()));
        };
        match self.resolve_module(name) {
            Ok(Some(Resolved::Coroutine(coro))) => CallResult::ModuleCoroutine {
                name: name.to_owned(),
                coro: coro.unbind(),
            },
            Ok(Some(Resolved::Module(module))) => CallResult::Sync(match self.module_value(name, &module) {
                Ok(value) => ExtFunctionResult::Return(value),
                Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
            }),
            Ok(None) => CallResult::Sync(ExtFunctionResult::NotFound(IMPORT_FUNCTION.to_owned())),
            Err(err) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err))),
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
    /// wrapper crosses as itself, its methods routing back by uuid; a dict or
    /// a module becomes a host object named after the module whose public
    /// attributes are sent eagerly: callables as host functions named
    /// `<module>.<attr>`, other values converted.
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

/// An `external_modules` entry after its factory (if any) ran.
enum Resolved<'py> {
    /// A module shape, installed in the feed's cache when a factory made it.
    Module(Bound<'py, PyAny>),
    /// A factory's coroutine, still to be awaited by the async loop.
    Coroutine(Bound<'py, PyAny>),
}

/// The error for a factory coroutine met where nothing can await it: the sync
/// pool, or a module function called before the module was imported. The
/// coroutine is closed so it does not warn that it was never awaited.
pub(crate) fn sync_module_coroutine_error(name: &str, coro: &Bound<'_, PyAny>) -> PyErr {
    let _ = coro.call_method0("close");
    PyRuntimeError::new_err(format!(
        "external_modules['{name}']() returned a coroutine; async module factories require AsyncMonty"
    ))
}

/// `TypeError` unless `name` is a `str` and `entry` a module shape (see
/// [`is_module_shape`]) or a callable returning one; see [`HostNames::capture`].
/// A class is callable but constructs an instance, never a module shape, so
/// it is refused here with a message naming it rather than at the import.
fn check_module_entry(name: &Bound<'_, PyAny>, entry: &Bound<'_, PyAny>) -> PyResult<()> {
    const SHAPES: &str = "must be a dict, a module, a ClassInstance or a callable returning one";
    if !name.is_instance_of::<PyString>() {
        Err(PyTypeError::new_err("external_modules keys must be str"))
    } else if let Ok(class) = entry.cast::<PyType>() {
        Err(PyTypeError::new_err(format!(
            "external_modules[{}] {SHAPES}, not the class {}",
            name.repr()?,
            class.name()?
        )))
    } else if is_module_shape(entry)? || entry.is_callable() {
        Ok(())
    } else {
        Err(PyTypeError::new_err(format!(
            "external_modules[{}] {SHAPES}, not {}",
            name.repr()?,
            entry.get_type().name()?
        )))
    }
}

/// Whether `value` can stand for a module: a dict, a module or a
/// `ClassInstance` wrapper.
fn is_module_shape(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    Ok(value.is_instance_of::<PyDict>() || value.is_instance_of::<PyModule>() || is_class_instance_wrapper(value)?)
}

/// The public attribute `attr` of an `external_modules` entry: a dict's item,
/// or a module's attribute; `None` when absent.
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
/// items (which must have `str` keys), or `dir()` of a module.
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

/// A stable identity for the host object standing in for module `name`, so
/// every `import` of it — including after a dump is restored — carries the
/// same uuid: FNV-1a over `kind:name`, folded into the two uuid halves.
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
    /// A module factory's coroutine answering `import <name>`: awaited as a
    /// value (an import cannot take a future) and finished with
    /// [`ExternalLookup::finish_import`].
    ModuleCoroutine { name: String, coro: Py<PyAny> },
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
