//! Resolving names a sandbox snippet leaves undefined against a feed's
//! `external_lookup` dict, its imports against the session's
//! `external_modules`, plus method calls and lazy attribute lookups on host
//! class instances.
//!
//! [`ExternalLookup`] owns both halves of the lazy-resolution protocol — the
//! `NameLookup` that resolves a bare name and the `FunctionCall` that invokes a
//! resolved host function — so the callable-vs-value rule linking them lives in
//! one place. It also answers the `__import__` call that binds a module and
//! the dotted calls (`tools.add`) into it. Host-routed calls
//! (`dispatch_object_call*`) and lazy attribute lookups (`resolve_object_attr`)
//! are a separate concern: they route through the session's [`InstanceStore`]
//! to the original wrapped object or class, not `external_lookup`.

use std::{collections::HashMap, sync::Arc};

use monty_proto::python::{
    DecodedArena, InstanceStore, exc_py_to_monty, is_class_instance_wrapper, is_class_type_wrapper, py_to_monty,
    py_to_monty_value,
};
use monty_types::{
    CallArgs, ExtFunctionResult, IMPORT_FUNCTION, ModuleStub, MontyObject, MontyUuid, NameLookupResult,
    unstable::{self, MontyNode},
    validate_module_name,
};
use pyo3::{
    exceptions::{PyAttributeError, PyRuntimeError, PyTypeError, PyValueError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyString, PyTuple, PyType},
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

/// The submodules of each module by its dotted path (`foo`, `foo.bar`): the
/// `modules` of its entry, each a dict or a `ClassInstance`.
type Submodules = HashMap<String, Vec<(String, Py<PyAny>)>>;

/// The `external_modules=` a checkout captured: the host side of the modules
/// every feed of the session may import.
pub(crate) struct HostModules {
    /// Each top-level entry's `module`, by the name the snippet imports: a
    /// dict, a `ClassInstance` or the factory returning one.
    modules: Py<PyDict>,
    /// What each factory returned, by module name: a factory runs once per
    /// session, at the first import or call that needs it.
    resolved: Py<PyDict>,
    /// See [`Submodules`].
    submodules: Arc<Submodules>,
    /// Each module's dotted path by the uuid of its dict stand-in (see
    /// [`module_uuid`]), so a method call on one is answered without a scan.
    stand_ins: Arc<HashMap<MontyUuid, String>>,
}

impl HostModules {
    /// Captures `external_modules`, returning the stubs its entries declare
    /// for the worker's type checker. Every entry must be an `ExternalModule`
    /// whose `module` is a dict, a `ClassInstance` or a zero-argument callable
    /// returning one: a dict names exactly what crosses, where a module or
    /// `dir()` of an arbitrary object would also expose its imports and
    /// whatever else it happens to carry. Names are identifiers, so an import
    /// and a host function name split into them unambiguously.
    pub(crate) fn capture(py: Python<'_>, entries: &Bound<'_, PyDict>) -> PyResult<(Self, Vec<ModuleStub>)> {
        let modules = PyDict::new(py);
        let mut tree = ModuleTree::default();
        for (name, entry) in entries.iter() {
            let name = module_key(&name, "external_modules")?;
            let name_str = name.to_cow()?;
            validate_module_name(&name_str).map_err(|err| PyValueError::new_err(err.to_string()))?;
            let module = tree.collect(&name_str, &entry, true)?;
            modules.set_item(&name, module)?;
        }
        let captured = Self {
            modules: modules.unbind(),
            resolved: PyDict::new(py).unbind(),
            submodules: Arc::new(tree.submodules),
            stand_ins: Arc::new(tree.stand_ins),
        };
        Ok((captured, tree.stubs))
    }

    /// A second owner of the same dicts, for a feed's drive context.
    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            modules: self.modules.clone_ref(py),
            resolved: self.resolved.clone_ref(py),
            submodules: Arc::clone(&self.submodules),
            stand_ins: Arc::clone(&self.stand_ins),
        }
    }
}

/// What walking the `external_modules` entries gathers besides the top-level
/// values: see [`HostModules`].
#[derive(Default)]
struct ModuleTree {
    stubs: Vec<ModuleStub>,
    submodules: Submodules,
    stand_ins: HashMap<MontyUuid, String>,
    /// The entries being walked, outermost first, by object identity and
    /// path: an entry met again inside itself would recurse forever.
    ancestors: Vec<(usize, String)>,
}

impl ModuleTree {
    /// Checks the `ExternalModule` at dotted `path` (see
    /// [`check_external_module`]), records its stub, stand-in and submodules,
    /// and returns its `module`. Only a top-level entry may be a factory: a
    /// submodule crosses with its parent, so there is nothing to defer. An
    /// entry nested inside itself is a `ValueError` rather than a stack overflow.
    fn collect(&mut self, path: &str, entry: &Bound<'_, PyAny>, top_level: bool) -> PyResult<Py<PyAny>> {
        let identity = entry.as_ptr() as usize;
        if let Some((_, ancestor)) = self.ancestors.iter().find(|(ptr, _)| *ptr == identity) {
            return Err(PyValueError::new_err(format!(
                "external_modules['{path}'] is external_modules['{ancestor}'] again: modules cannot nest cyclically"
            )));
        }
        let CheckedModule {
            module,
            stub,
            submodules,
        } = check_external_module(path, entry, top_level)?;
        self.stubs.extend(stub);
        self.stand_ins.insert(module_uuid("instance", path), path.to_owned());
        self.ancestors.push((identity, path.to_owned()));
        let mut children = Vec::new();
        for (name, child) in submodules {
            let child_path = format!("{path}.{name}");
            let value = self.collect(&child_path, &child, false)?;
            children.push((name, value));
        }
        self.ancestors.pop();
        if !children.is_empty() {
            self.submodules.insert(path.to_owned(), children);
        }
        Ok(module.unbind())
    }
}

/// The `external_lookup=` dict one feed captured and the session's modules:
/// the host side of the names and imports a snippet leaves to it. Held as
/// owned references so a snapshot can keep them for `resume_auto`.
#[derive(Default)]
pub(crate) struct HostNames {
    /// `external_lookup=`: host values by the bare name the snippet reads.
    pub(crate) lookup: Option<Py<PyDict>>,
    /// The session's `external_modules`, shared by all of its feeds.
    pub(crate) modules: Option<HostModules>,
}

impl HostNames {
    /// Captures the dict a feed was called with alongside the session's modules.
    pub(crate) fn capture(py: Python<'_>, lookup: Option<&Bound<'_, PyDict>>, modules: Option<&HostModules>) -> Self {
        Self {
            lookup: lookup.map(|d| d.clone().unbind()),
            modules: modules.map(|m| m.clone_ref(py)),
        }
    }

    /// A second owner of the same dicts, for a snapshot's drive context.
    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            lookup: self.lookup.as_ref().map(|d| d.clone_ref(py)),
            modules: self.modules.as_ref().map(|m| m.clone_ref(py)),
        }
    }
}

/// A feed's `external_lookup` and the session's `external_modules` (absent
/// when the caller passed none) plus the `Python` token and instance store every
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
    submodules: Option<&'py Submodules>,
    stand_ins: Option<&'py HashMap<MontyUuid, String>>,
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
            modules: names.modules.as_ref().map(|m| m.modules.bind(py)),
            resolved_modules: names.modules.as_ref().map(|m| m.resolved.bind(py)),
            submodules: names.modules.as_ref().map(|m| &*m.submodules),
            stand_ins: names.modules.as_ref().map(|m| &*m.stand_ins),
            instances,
        }
    }

    /// Answers a method call on a host object (a `FunctionCall` with an
    /// `object_id`). A dict module's stand-in has no host object behind it, so
    /// calling it is `TypeError` and any other method `AttributeError`, as for
    /// a value of its kind; everything else routes through the instance store.
    pub fn call_object(&self, function_name: &str, object_id: &MontyUuid, args: &CallArgs) -> ExtFunctionResult {
        match self.module_stand_in(object_id) {
            Some(name) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &module_method_error(name, function_name))),
            None => dispatch_object_call(self.py, function_name, object_id, args, self.instances),
        }
    }

    /// Like [`call_object`](Self::call_object) but returns `CallResult::Coroutine`
    /// when the method returns a coroutine.
    pub fn call_object_or_coroutine(&self, function_name: &str, object_id: &MontyUuid, args: &CallArgs) -> CallResult {
        match self.module_stand_in(object_id) {
            Some(name) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(
                self.py,
                &module_method_error(name, function_name),
            ))),
            None => dispatch_object_call_or_coroutine(self.py, function_name, object_id, args, self.instances),
        }
    }

    /// The dict module whose sandbox stand-in `object_id` identifies, if any
    /// (see [`module_uuid`]); a `ClassInstance` module keeps the wrapper's own id.
    fn module_stand_in(&self, object_id: &MontyUuid) -> Option<&'py str> {
        self.stand_ins?.get(object_id).map(String::as_str)
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
        // a module first needed by a call through a binding from an earlier
        // feed, whose factory returned an awaitable: await it, then call
        match self.module_of(function_name) {
            Ok(Some((dot, Resolved::Awaitable(awaitable)))) => {
                return CallResult::ModuleCoroutine {
                    name: function_name[..dot].to_owned(),
                    coro: awaitable.unbind(),
                    then: AfterModule::Call {
                        function_name: function_name.to_owned(),
                        args: args.clone(),
                    },
                };
            }
            Err(err) => return CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err))),
            Ok(_) => {}
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
    /// or, for a dotted name, that attribute of the module the path before the
    /// last dot walks to (a host function bound by an import is named
    /// `<module>.<attr>`). `None` when neither has it.
    fn callable(&self, function_name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        if function_name.contains('.') {
            match self.module_of(function_name)? {
                // a `ClassInstance` module routes its calls by uuid under the wrapper's
                // policy, so a name-based call into it (which only a non-conforming
                // worker sends) finds nothing
                Some((_, Resolved::Module(module))) if is_class_instance_wrapper(&module)? => Ok(None),
                Some((dot, Resolved::Module(module))) => module_attr(&module, &function_name[dot + 1..]),
                Some((dot, Resolved::Awaitable(awaitable))) => {
                    Err(sync_module_coroutine_error(&function_name[..dot], &awaitable))
                }
                None => Ok(None),
            }
        } else {
            match self.lookup {
                Some(lookup) => lookup.get_item(function_name),
                None => Ok(None),
            }
        }
    }

    /// The `external_modules` entry for `name`, if any, with a factory's
    /// result installed; a factory that returned an awaitable is an error here,
    /// since only [`import_module_or_coroutine`](Self::import_module_or_coroutine)
    /// can have it awaited.
    fn module(&self, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        match self.resolve_module(name)? {
            Some(Resolved::Module(module)) => Ok(Some(module)),
            Some(Resolved::Awaitable(awaitable)) => Err(sync_module_coroutine_error(name, &awaitable)),
            None => Ok(None),
        }
    }

    /// The `external_modules` entry for `name`, if any. A factory entry is
    /// called the first time the session needs the module and its result kept
    /// in `resolved_modules`, which is consulted first so every import and
    /// call of the module's functions sees one module; an awaitable it returns
    /// is handed back for the async loop to await and [`install_module`](Self::install_module).
    fn resolve_module(&self, name: &str) -> PyResult<Option<Resolved<'py>>> {
        let (Some(modules), Some(resolved)) = (self.modules, self.resolved_modules) else {
            return Ok(None);
        };
        if let Some(module) = resolved.get_item(name)? {
            return Ok(Some(Resolved::Module(module)));
        }
        let Some(entry) = modules.get_item(name)? else {
            return Ok(None);
        };
        if !entry.is_callable() {
            return Ok(Some(Resolved::Module(entry)));
        }
        let result = callback_context::call(self.py, || entry.call0())?;
        if is_awaitable(self.py, &result) {
            Ok(Some(Resolved::Awaitable(result)))
        } else {
            self.install_module(name, result).map(Resolved::Module).map(Some)
        }
    }

    /// Keeps what module `name`'s factory produced for the rest of the session,
    /// refusing anything that is not a module shape or that cannot carry the
    /// submodules declared for it, the checks a value entry passed at checkout.
    fn install_module(&self, name: &str, module: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        if !is_module_shape(&module)? {
            return Err(PyTypeError::new_err(format!(
                "external_modules['{name}'].module() returned {}, not a dict or a ClassInstance",
                module.get_type().name()?
            )));
        }
        if let Some(children) = self.submodules.and_then(|tree| tree.get(name)) {
            let Ok(dict) = module.cast::<PyDict>() else {
                return Err(PyTypeError::new_err(format!(
                    "external_modules['{name}'].module() returned a ClassInstance, which cannot carry .modules"
                )));
            };
            if let Some((child, _)) = children.iter().find(|(child, _)| dict.contains(child).unwrap_or(false)) {
                return Err(PyValueError::new_err(format!(
                    "external_modules['{name}'].module() returned a dict with both an item and a submodule named '{child}'"
                )));
            }
        }
        self.resolved_modules
            .expect("a module was resolved, so the cache exists")
            .set_item(name, &module)?;
        Ok(module)
    }

    /// The module a dotted `function_name` calls into, with the index of the
    /// dot before its attribute: the path up to that dot is walked one
    /// component at a time, the first through `external_modules` (a factory's
    /// awaitable comes back as such) and the rest through the submodules of
    /// the one before. `None` as soon as a component is missing, so a
    /// worker-sent name costs one lookup per component.
    fn module_of(&self, function_name: &str) -> PyResult<Option<(usize, Resolved<'py>)>> {
        let Some((module_path, _)) = function_name.rsplit_once('.') else {
            return Ok(None);
        };
        let mut parts = module_path.split('.');
        let top = parts.next().unwrap_or_default();
        let mut module = match self.resolve_module(top)? {
            Some(Resolved::Module(module)) => module,
            Some(awaitable @ Resolved::Awaitable(_)) => return Ok(Some((module_path.len(), awaitable))),
            None => return Ok(None),
        };
        let mut path = top.to_owned();
        for part in parts {
            let Some(child) = self.submodule(&path, part) else {
                return Ok(None);
            };
            path.push('.');
            path.push_str(part);
            module = child;
        }
        Ok(Some((module_path.len(), Resolved::Module(module))))
    }

    /// The submodule `name` of the module at dotted `path`, if it has one.
    fn submodule(&self, path: &str, name: &str) -> Option<Bound<'py, PyAny>> {
        self.submodules?
            .get(path)?
            .iter()
            .find(|(child, _)| child == name)
            .map(|(_, value)| value.bind(self.py).clone())
    }

    /// Finishes a request whose module factory returned an awaitable, once the
    /// async loop has awaited it to `result`: the module is installed, then the
    /// import is answered with it or the call it was needed for is made.
    pub(crate) fn finish_after_module(
        &self,
        name: &str,
        result: PyResult<Bound<'py, PyAny>>,
        then: &AfterModule,
    ) -> Staged {
        let module = match result.and_then(|module| self.install_module(name, module)) {
            Ok(module) => module,
            Err(err) => return Staged::Done(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err))),
        };
        match then {
            AfterModule::Import => Staged::Done(match self.module_value(name, &module) {
                Ok(value) => ExtFunctionResult::Return(value),
                Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
            }),
            AfterModule::Call { function_name, args } => match self.call_or_coroutine(function_name, args) {
                CallResult::Sync(result) => Staged::Done(result),
                CallResult::Coroutine(coro) => Staged::Coroutine(coro),
                // the module is installed, so its factory cannot be pending again
                CallResult::ModuleCoroutine { .. } => Staged::Done(ExtFunctionResult::Error(exc_py_to_monty(
                    self.py,
                    &PyRuntimeError::new_err(format!("module factory for {name:?} pending after it was installed")),
                ))),
            },
        }
    }

    /// [`import_module`](Self::import_module) for the async loop: a factory's
    /// awaitable comes back as [`CallResult::ModuleCoroutine`] to await.
    fn import_module_or_coroutine(&self, args: &CallArgs) -> CallResult {
        let Some(name) = args.args().next().and_then(|arg| arg.as_str()) else {
            return CallResult::Sync(ExtFunctionResult::NotFound(IMPORT_FUNCTION.to_owned()));
        };
        match self.resolve_module(name) {
            Ok(Some(Resolved::Awaitable(awaitable))) => CallResult::ModuleCoroutine {
                name: name.to_owned(),
                coro: awaitable.unbind(),
                then: AfterModule::Import,
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

    /// The sandbox value of the module at dotted `path`. A `ClassInstance`
    /// wrapper crosses as itself, its methods routing back by uuid; a dict
    /// becomes a host object named after the path whose public items are sent
    /// eagerly: callables as host functions named `<path>.<attr>` (a
    /// `ClassType` wrapper as the type it wraps), other values converted, and
    /// each submodule as its own module value.
    fn module_value(&self, path: &str, module: &Bound<'py, PyAny>) -> PyResult<MontyObject> {
        if is_class_instance_wrapper(module)? {
            return py_to_monty_value(module, self.instances)
                .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc));
        }
        let mut attrs = Vec::new();
        for (attr, value) in module_attrs(module)? {
            let value = if value.is_callable() && !is_class_type_wrapper(&value)? {
                MontyObject::function(format!("{path}.{attr}"), None)
            } else {
                py_to_monty_value(&value, self.instances)
                    .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc))?
            };
            attrs.push((MontyObject::string(attr), value));
        }
        for (name, child) in self.submodules.and_then(|tree| tree.get(path)).into_iter().flatten() {
            let value = self.module_value(&format!("{path}.{name}"), child.bind(self.py))?;
            attrs.push((MontyObject::string(name), value));
        }
        let class = MontyObject::class_type(path, module_uuid("class", path), true, false, []);
        Ok(MontyObject::class_instance(class, module_uuid("instance", path), attrs))
    }
}

/// An `external_modules` entry after its factory (if any) ran.
enum Resolved<'py> {
    /// A module shape, installed in the session's cache when a factory made it.
    Module(Bound<'py, PyAny>),
    /// A factory's awaitable (a coroutine, a future, ...), still to be awaited
    /// by the async loop.
    Awaitable(Bound<'py, PyAny>),
}

/// The error for a factory awaitable met where nothing can await it: the sync
/// pool. A coroutine is closed so it does not warn that it was never awaited;
/// other awaitables have no close protocol and are left alone.
pub(crate) fn sync_module_coroutine_error(name: &str, awaitable: &Bound<'_, PyAny>) -> PyErr {
    if is_coroutine(awaitable.py(), awaitable) {
        let _ = awaitable.call_method0("close");
    }
    PyRuntimeError::new_err(format!(
        "external_modules['{name}'].module() returned an awaitable; async module factories require AsyncMonty"
    ))
}

/// The error for a method call on a dict module's stand-in, which has no host
/// object to call: `__call__` (calling the module) is `TypeError`, any other
/// name `AttributeError`, both as CPython words them for a value of that kind.
fn module_method_error(name: &str, method: &str) -> PyErr {
    if method == "__call__" {
        PyTypeError::new_err(format!("'{name}' object is not callable"))
    } else {
        PyAttributeError::new_err(format!("'{name}' object has no attribute '{method}'"))
    }
}

/// Checks the `ExternalModule` for the module at dotted `path` (see
/// [`HostModules::capture`]), returning its `module`, the [`ModuleStub`] its
/// `stubs` declare and its `modules` entries. `entry` must be an
/// `ExternalModule` and its `module` a module shape (see [`is_module_shape`])
/// or, at the top level, a callable returning one, else `TypeError`; a class
/// is callable but constructs an instance, never a module shape, so it is
/// refused here with a message naming it rather than at the import. A
/// submodule name that is not an identifier, or that the parent dict also
/// has, is a `ValueError`, as is a stub path the checker refuses.
fn check_external_module<'py>(path: &str, entry: &Bound<'py, PyAny>, top_level: bool) -> PyResult<CheckedModule<'py>> {
    let where_ = |suffix: &str| format!("external_modules['{path}']{suffix}");
    if !entry.is_instance(external_module_class(entry.py())?)? {
        return Err(PyTypeError::new_err(format!(
            "{} must be an ExternalModule, not {}",
            where_(""),
            entry.get_type().name()?
        )));
    }
    let shapes = if top_level {
        "must be a dict, a ClassInstance or a callable returning one"
    } else {
        "must be a dict or a ClassInstance"
    };
    let module = entry.getattr("module")?;
    if let Ok(class) = module.cast::<PyType>() {
        return Err(PyTypeError::new_err(format!(
            "{} {shapes}, not the class {}",
            where_(".module"),
            class.name()?
        )));
    }
    if !(is_module_shape(&module)? || (top_level && module.is_callable())) {
        return Err(PyTypeError::new_err(format!(
            "{} {shapes}, not {}",
            where_(".module"),
            module.get_type().name()?
        )));
    }
    // a dict's keys are checked now rather than at the import
    if module.is_instance_of::<PyDict>() {
        module_attrs(&module)?;
    }
    let stubs = entry.getattr("stubs")?;
    let stub = if stubs.is_none() {
        None
    } else {
        let Ok(source) = stubs.extract::<String>() else {
            return Err(PyTypeError::new_err(format!(
                "{} must be a str or None, not {}",
                where_(".stubs"),
                stubs.get_type().name()?
            )));
        };
        Some(ModuleStub::new(path, source).map_err(|err| PyValueError::new_err(err.to_string()))?)
    };
    let modules = entry.getattr("modules")?;
    let mut submodules = Vec::new();
    if !modules.is_none() {
        let Ok(modules) = modules.cast::<PyDict>() else {
            return Err(PyTypeError::new_err(format!(
                "{} must be a dict or None, not {}",
                where_(".modules"),
                modules.get_type().name()?
            )));
        };
        for (name, child) in modules.iter() {
            let name = module_key(&name, &where_(".modules"))?;
            if !name.call_method0("isidentifier")?.is_truthy()? {
                return Err(PyValueError::new_err(format!(
                    "{} name {name:?} is not a valid identifier",
                    where_(".modules")
                )));
            }
            if module
                .cast::<PyDict>()
                .is_ok_and(|dict| dict.contains(&name).unwrap_or(false))
            {
                return Err(PyValueError::new_err(format!(
                    "{} has both an item and a submodule named {name:?}",
                    where_(".module")
                )));
            }
            submodules.push((name.to_string(), child));
        }
        // a wrapper's attributes are its own, so nothing can be hung on it
        if !submodules.is_empty() && is_class_instance_wrapper(&module)? {
            return Err(PyTypeError::new_err(format!(
                "{} must be None when .module is a ClassInstance, whose attributes are its own",
                where_(".modules")
            )));
        }
    }
    Ok(CheckedModule {
        module,
        stub,
        submodules,
    })
}

/// What [`check_external_module`] reads off one `ExternalModule`.
struct CheckedModule<'py> {
    module: Bound<'py, PyAny>,
    stub: Option<ModuleStub>,
    /// Its `modules`, each still to be checked.
    submodules: Vec<(String, Bound<'py, PyAny>)>,
}

/// A module-mapping key as a Python `str`; `where_` names the mapping in the
/// `TypeError` for any other key.
fn module_key<'py>(key: &Bound<'py, PyAny>, where_: &str) -> PyResult<Bound<'py, PyString>> {
    key.cast::<PyString>()
        .cloned()
        .map_err(|_| PyTypeError::new_err(format!("{where_} keys must be str")))
}

/// Cached import of the `pydantic_monty.ExternalModule` class.
fn external_module_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static EXTERNAL_MODULE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    EXTERNAL_MODULE.import(py, "pydantic_monty", "ExternalModule")
}

/// Whether `value` can stand for a module: a dict or a `ClassInstance` wrapper.
fn is_module_shape(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    Ok(value.is_instance_of::<PyDict>() || is_class_instance_wrapper(value)?)
}

/// The public item `attr` of a dict `external_modules` entry; `None` when
/// absent or private.
fn module_attr<'py>(module: &Bound<'py, PyAny>, attr: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    if attr.starts_with('_') {
        Ok(None)
    } else {
        module.cast::<PyDict>()?.get_item(attr)
    }
}

/// The public items of a dict `external_modules` entry, in order; keys must
/// be identifiers, since a host function is named by its dotted path.
fn module_attrs<'py>(module: &Bound<'py, PyAny>) -> PyResult<Vec<(String, Bound<'py, PyAny>)>> {
    let mut attrs = Vec::new();
    for (key, value) in module.cast::<PyDict>()?.iter() {
        let Ok(key) = key.cast::<PyString>() else {
            return Err(PyTypeError::new_err("external_modules entries must have str keys"));
        };
        if !key.call_method0("isidentifier")?.is_truthy()? {
            return Err(PyValueError::new_err(format!(
                "external_modules entries must have identifier keys, not {}",
                key.repr()?
            )));
        }
        let key = key.to_string();
        if !key.starts_with('_') {
            attrs.push((key, value));
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
    /// A module factory's awaitable, needed before `then` can be answered:
    /// awaited as a value (an import cannot take a future) and finished with
    /// [`ExternalLookup::finish_after_module`].
    ModuleCoroutine {
        name: String,
        coro: Py<PyAny>,
        then: AfterModule,
    },
}

/// What a factory's awaited module is for, once installed.
pub enum AfterModule {
    /// `import <module>`: the module value is the answer.
    Import,
    /// A call of one of the module's functions, through a binding an earlier
    /// feed made; the module is needed to find the function.
    Call { function_name: String, args: CallArgs },
}

/// The second stage of a request that waited for a module factory: an answer,
/// or the called function's own coroutine, still to be awaited.
pub(crate) enum Staged {
    Done(ExtFunctionResult),
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
    inspect_predicate(py, "iscoroutine", obj)
}

/// Checks whether a Python object can be awaited via `inspect.isawaitable()`:
/// a coroutine, a future or anything with `__await__`.
fn is_awaitable(py: Python<'_>, obj: &Bound<'_, PyAny>) -> bool {
    inspect_predicate(py, "isawaitable", obj)
}

/// Calls the `inspect` predicate `name` on `obj`, `false` on any failure.
fn inspect_predicate(py: Python<'_>, name: &str, obj: &Bound<'_, PyAny>) -> bool {
    py.import("inspect")
        .and_then(|inspect| inspect.getattr(name))
        .and_then(|predicate| predicate.call1((obj,)))
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
