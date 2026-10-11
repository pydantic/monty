//! Resolves bare names and calls through `external_lookup`, and imports, stubs and
//! dotted calls through `external_modules`. [`ExternalLookup`] applies the same
//! callable/value rules to name lookups and subsequent calls. Object methods and
//! lazy attributes resolve through the session's [`InstanceStore`].

use std::{
    collections::HashMap,
    iter::once,
    sync::{Arc, Mutex},
};

use monty_proto::python::{
    DecodedArena, InstanceStore, exc_py_to_monty, is_class_instance_wrapper, is_class_type_wrapper, py_to_monty,
    py_to_monty_value,
};
use monty_types::{
    CallArgs, ExtFunctionResult, IMPORT_CALL, IMPORT_STUBS_CALL, MontyObject, MontyUuid, NameLookupResult,
    unstable::{self, MontyNode},
    validate_module_name,
};
use pyo3::{
    exceptions::{PyAttributeError, PyRuntimeError, PyTypeError, PyValueError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyMapping, PyString, PyTuple, PyType},
};

use crate::{callback_context, exceptions::MontyConversionError, pool::lock};

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

/// A feed's validated modules and cached factory results, retained for `resume_auto`.
/// Factories run and are validated on first use, at most once per feed.
pub(crate) struct HostModules {
    /// Each entry by the name the snippet imports: an `ExternalModule`, or
    /// the factory returning one.
    entries: Py<PyDict>,
    /// What each factory returned, by module name: a factory runs once per
    /// feed, at the first import, call or stubs ask that needs it.
    resolved: Py<PyDict>,
    /// Each dict module's dotted path by the uuid of its stand-in (see
    /// [`module_uuid`]), so a method call on one is answered without a scan.
    /// Grows as factories resolve, hence the lock.
    stand_ins: Arc<Mutex<HashMap<MontyUuid, String>>>,
}

impl HostModules {
    /// Captures modules or zero-argument factories returning them from any mapping
    /// (a dict is kept, so its entries are re-read on each import). Dicts explicitly
    /// select what crosses; module objects could expose imports. Identifier keys keep
    /// dotted host-function names unambiguous.
    pub(crate) fn capture(py: Python<'_>, entries: &Bound<'_, PyAny>) -> PyResult<Self> {
        let entries = if let Ok(dict) = entries.cast::<PyDict>() {
            dict.clone()
        } else {
            let Ok(mapping) = entries.cast::<PyMapping>() else {
                return Err(PyTypeError::new_err(format!(
                    "external_modules must be a mapping of module name to ExternalModule, not {}",
                    entries.get_type().name()?
                )));
            };
            let dict = PyDict::new(py);
            dict.update(mapping)?;
            dict
        };
        let mut stand_ins = HashMap::new();
        for (name, entry) in entries.iter() {
            let name = module_key(&name, "external_modules")?;
            let name = name.to_cow()?;
            validate_module_name(&name).map_err(|err| PyValueError::new_err(err.to_string()))?;
            if entry.is_instance(external_module_class(py)?)? {
                check_module_tree(&name, &entry, &mut stand_ins, &mut Vec::new())?;
            } else if let Ok(class) = entry.cast::<PyType>() {
                // callable, but constructs an instance, never a module
                return Err(PyTypeError::new_err(format!(
                    "external_modules['{name}'] must be an ExternalModule or a callable returning one, not the class {}",
                    class.name()?
                )));
            } else if !entry.is_callable() {
                return Err(PyTypeError::new_err(format!(
                    "external_modules['{name}'] must be an ExternalModule or a callable returning one, not {}",
                    entry.get_type().name()?
                )));
            }
        }
        Ok(Self {
            entries: entries.unbind(),
            resolved: PyDict::new(py).unbind(),
            stand_ins: Arc::new(Mutex::new(stand_ins)),
        })
    }

    /// A second owner of the same dicts, for a snapshot's drive context.
    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            entries: self.entries.clone_ref(py),
            resolved: self.resolved.clone_ref(py),
            stand_ins: Arc::clone(&self.stand_ins),
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
    /// `external_modules=`: the modules the snippet may import.
    pub(crate) modules: Option<HostModules>,
}

impl HostNames {
    /// Captures the dicts a feed was called with, validating the modules.
    pub(crate) fn capture(
        py: Python<'_>,
        lookup: Option<&Bound<'_, PyDict>>,
        modules: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Ok(Self {
            lookup: lookup.map(|d| d.clone().unbind()),
            modules: modules.map(|m| HostModules::capture(py, m)).transpose()?,
        })
    }

    /// A second owner of the same dicts, for a snapshot's drive context.
    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            lookup: self.lookup.as_ref().map(|d| d.clone_ref(py)),
            modules: self.modules.as_ref().map(|m| m.clone_ref(py)),
        }
    }
}

/// Identifies module OS calls, which take precedence over mounts and `os=`.
/// Call only for OS events: an ordinary function named `import_stubs` is unrelated.
pub(crate) fn is_module_call(function_name: &str) -> bool {
    function_name == IMPORT_CALL || function_name == IMPORT_STUBS_CALL
}

/// Resolves a feed's bare names, dotted module calls and module OS calls.
/// Calls re-read the dict entry, so replacing a callable with a value raises `TypeError`.
/// Returned `ClassInstance` wrappers register in the instance store.
pub struct ExternalLookup<'a, 'py> {
    py: Python<'py>,
    lookup: Option<&'py Bound<'py, PyDict>>,
    modules: Option<&'py HostModules>,
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
            modules: names.modules.as_ref(),
            instances,
        }
    }

    /// Dispatches through the instance store. Dict modules have no callable host object:
    /// `__call__` raises `TypeError`; other methods raise `AttributeError`.
    pub fn call_object(&self, function_name: &str, object_id: &MontyUuid, args: &CallArgs) -> ExtFunctionResult {
        match self.module_stand_in(object_id) {
            Some(name) => {
                ExtFunctionResult::Error(exc_py_to_monty(self.py, &module_method_error(&name, function_name)))
            }
            None => dispatch_object_call(self.py, function_name, object_id, args, self.instances),
        }
    }

    /// Like [`call_object`](Self::call_object) but returns `CallResult::Coroutine`
    /// when the method returns a coroutine.
    pub fn call_object_or_coroutine(&self, function_name: &str, object_id: &MontyUuid, args: &CallArgs) -> CallResult {
        match self.module_stand_in(object_id) {
            Some(name) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(
                self.py,
                &module_method_error(&name, function_name),
            ))),
            None => dispatch_object_call_or_coroutine(self.py, function_name, object_id, args, self.instances),
        }
    }

    /// The dict module whose sandbox stand-in `object_id` identifies, if any
    /// (see [`module_uuid`]); a `ClassInstance` module keeps the wrapper's own id.
    fn module_stand_in(&self, object_id: &MontyUuid) -> Option<String> {
        lock(&self.modules?.stand_ins).get(object_id).cloned()
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
    /// that will be re-raised inside Monty execution.
    pub fn call(&self, function_name: &str, args: &CallArgs) -> ExtFunctionResult {
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
        // a module first needed by a call through a binding from an earlier
        // feed, whose factory returned an awaitable: await it, then call
        match self.module_of(function_name) {
            Ok(Some((dot, Resolved::Awaitable(awaitable)))) => {
                return CallResult::ModuleCoroutine {
                    names: vec![function_name[..dot].to_owned()],
                    coros: vec![awaitable.unbind()],
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

    /// Resolves a bare `external_lookup` key or a dotted module attribute; `None` if absent.
    fn callable(&self, function_name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        if function_name.contains('.') {
            match self.module_of(function_name)? {
                Some((dot, Resolved::Module(entry))) => {
                    let module = module_value_of(&entry)?;
                    // a `ClassInstance` module routes its calls by uuid under the wrapper's
                    // policy, so a name-based call into it (which only a non-conforming
                    // worker sends) finds nothing
                    if is_class_instance_wrapper(&module)? {
                        Ok(None)
                    } else {
                        module_attr(&module, &function_name[dot + 1..])
                    }
                }
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

    /// Answers an `import` or `import_stubs` OS call (see [`is_module_call`])
    /// from `external_modules`; `None` for any other call. A factory's
    /// awaitable is an error here, as the sync pool cannot await it.
    pub fn module_call(&self, function_name: &str, args: &CallArgs) -> Option<ExtFunctionResult> {
        match function_name {
            IMPORT_CALL => Some(self.import_module(args)),
            IMPORT_STUBS_CALL => Some(self.import_stubs(args)),
            _ => None,
        }
    }

    /// Like [`module_call`](Self::module_call) but hands a factory's awaitable
    /// back as [`CallResult::ModuleCoroutine`] for the async loop to await.
    pub fn module_call_or_coroutine(&self, function_name: &str, args: &CallArgs) -> Option<CallResult> {
        match function_name {
            IMPORT_CALL => Some(self.import_module_or_coroutine(args)),
            IMPORT_STUBS_CALL => Some(self.import_stubs_or_coroutine(args)),
            _ => None,
        }
    }

    /// Resolves a module, caching its factory result for all imports, calls and stub requests
    /// in this feed. Returns awaitables for the async loop to await and install.
    fn resolve_module(&self, name: &str) -> PyResult<Option<Resolved<'py>>> {
        let Some(modules) = self.modules else {
            return Ok(None);
        };
        if let Some(entry) = modules.resolved.bind(self.py).get_item(name)? {
            return Ok(Some(Resolved::Module(entry)));
        }
        let Some(entry) = modules.entries.bind(self.py).get_item(name)? else {
            return Ok(None);
        };
        if entry.is_instance(external_module_class(self.py)?)? {
            return Ok(Some(Resolved::Module(entry)));
        }
        let result = callback_context::call(self.py, || entry.call0())?;
        if is_awaitable(self.py, &result) {
            Ok(Some(Resolved::Awaitable(result)))
        } else {
            self.install_module(name, result).map(Resolved::Module).map(Some)
        }
    }

    /// Validates and caches a factory result for the rest of the feed.
    fn install_module(&self, name: &str, module: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let modules = self.modules.expect("a module was resolved, so the entries exist");
        if !module.is_instance(external_module_class(self.py)?)? {
            return Err(PyTypeError::new_err(format!(
                "external_modules['{name}']() returned {}, not an ExternalModule",
                module.get_type().name()?
            )));
        }
        check_module_tree(name, &module, &mut lock(&modules.stand_ins), &mut Vec::new())?;
        modules.resolved.bind(self.py).set_item(name, &module)?;
        Ok(module)
    }

    /// Resolves a dotted call's module and returns its prefix length.
    /// A pending top-level factory returns its awaitable and top-level prefix length.
    /// Stops at the first missing component, bounding work for worker-supplied names.
    fn module_of(&self, function_name: &str) -> PyResult<Option<(usize, Resolved<'py>)>> {
        let Some((module_path, _)) = function_name.rsplit_once('.') else {
            return Ok(None);
        };
        let mut parts = module_path.split('.');
        let top = parts.next().unwrap_or_default();
        let mut entry = match self.resolve_module(top)? {
            Some(Resolved::Module(entry)) => entry,
            Some(awaitable @ Resolved::Awaitable(_)) => return Ok(Some((top.len(), awaitable))),
            None => return Ok(None),
        };
        for part in parts {
            let Some(child) = submodule(&module_value_of(&entry)?, part)? else {
                return Ok(None);
            };
            entry = child;
        }
        Ok(Some((module_path.len(), Resolved::Module(entry))))
    }

    /// Installs awaited factory results, then answers the request that needed them.
    pub(crate) fn finish_after_module(
        &self,
        names: &[String],
        modules: PyResult<Vec<Bound<'py, PyAny>>>,
        then: &AfterModule,
    ) -> Staged {
        let installed = modules.and_then(|modules| {
            for (name, module) in names.iter().zip(modules) {
                self.install_module(name, module)?;
            }
            Ok(())
        });
        if let Err(err) = installed {
            return Staged::Done(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)));
        }
        match then {
            AfterModule::Import => Staged::Done(self.import_module(&single_arg(MontyObject::string(&names[0])))),
            AfterModule::Stubs { asked } => {
                Staged::Done(self.import_stubs(&single_arg(MontyObject::list(asked.iter().map(MontyObject::string)))))
            }
            AfterModule::Call { function_name, args } => match self.call_or_coroutine(function_name, args) {
                CallResult::Sync(result) => Staged::Done(result),
                CallResult::Coroutine(coro) => Staged::Coroutine(coro),
                // the module is installed, so its factory cannot be pending again
                CallResult::ModuleCoroutine { names, .. } => Staged::Done(ExtFunctionResult::Error(exc_py_to_monty(
                    self.py,
                    &PyRuntimeError::new_err(format!(
                        "module factory for {:?} pending after it was installed",
                        names.join(", ")
                    )),
                ))),
            },
        }
    }

    /// [`import_module`](Self::import_module) for the async loop: a factory's
    /// awaitable comes back as [`CallResult::ModuleCoroutine`] to await.
    fn import_module_or_coroutine(&self, args: &CallArgs) -> CallResult {
        let Some(name) = import_name(args) else {
            return CallResult::Sync(ExtFunctionResult::NotFound(IMPORT_CALL.to_owned()));
        };
        match self.resolve_module(name) {
            Ok(Some(Resolved::Awaitable(awaitable))) => CallResult::ModuleCoroutine {
                names: vec![name.to_owned()],
                coros: vec![awaitable.unbind()],
                then: AfterModule::Import,
            },
            Ok(Some(Resolved::Module(entry))) => CallResult::Sync(match self.module_value(name, &entry) {
                Ok(value) => ExtFunctionResult::Return(value),
                Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
            }),
            Ok(None) => CallResult::Sync(ExtFunctionResult::NotFound(IMPORT_CALL.to_owned())),
            Err(err) => CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err))),
        }
    }

    /// Answers `import <name>` with the module's value, `NotFound` (a
    /// `ModuleNotFoundError`) when `external_modules` lacks it.
    fn import_module(&self, args: &CallArgs) -> ExtFunctionResult {
        let Some(name) = import_name(args) else {
            return ExtFunctionResult::NotFound(IMPORT_CALL.to_owned());
        };
        match self.resolve_module(name) {
            Ok(Some(Resolved::Module(entry))) => match self.module_value(name, &entry) {
                Ok(value) => ExtFunctionResult::Return(value),
                Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
            },
            Ok(Some(Resolved::Awaitable(awaitable))) => {
                ExtFunctionResult::Error(exc_py_to_monty(self.py, &sync_module_coroutine_error(name, &awaitable)))
            }
            Ok(None) => ExtFunctionResult::NotFound(IMPORT_CALL.to_owned()),
            Err(err) => ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
        }
    }

    /// [`import_stubs`](Self::import_stubs) for the async loop: the factories
    /// that returned awaitables are awaited together, as one
    /// [`CallResult::ModuleCoroutine`], before the answer is made.
    fn import_stubs_or_coroutine(&self, args: &CallArgs) -> CallResult {
        let Some(asked) = asked_modules(args) else {
            return CallResult::Sync(ExtFunctionResult::NotFound(IMPORT_STUBS_CALL.to_owned()));
        };
        let mut pending: Vec<(String, Bound<'py, PyAny>)> = Vec::new();
        for name in &asked {
            match self.resolve_module(name) {
                Ok(Some(Resolved::Awaitable(awaitable))) => pending.push((name.clone(), awaitable)),
                Ok(_) => {}
                Err(err) => {
                    // the error ends the ask, so the coroutines already made are never awaited
                    for (_, awaitable) in &pending {
                        close_coroutine(awaitable);
                    }
                    return CallResult::Sync(ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)));
                }
            }
        }
        if pending.is_empty() {
            return CallResult::Sync(self.import_stubs(args));
        }
        let (names, coros) = pending
            .into_iter()
            .map(|(name, awaitable)| (name, awaitable.unbind()))
            .unzip();
        CallResult::ModuleCoroutine {
            names,
            coros,
            then: AfterModule::Stubs { asked },
        }
    }

    /// Answers `import_stubs` with a dict of dotted module path to `.pyi`
    /// source (`None` for a module without one) for every asked module
    /// `external_modules` has, submodules included; absent names are omitted.
    fn import_stubs(&self, args: &CallArgs) -> ExtFunctionResult {
        let Some(asked) = asked_modules(args) else {
            return ExtFunctionResult::NotFound(IMPORT_STUBS_CALL.to_owned());
        };
        let mut stubs = Vec::new();
        for name in &asked {
            match self.resolve_module(name) {
                Ok(Some(Resolved::Module(entry))) => {
                    if let Err(err) = collect_stubs(name, &entry, &mut stubs) {
                        return ExtFunctionResult::Error(exc_py_to_monty(self.py, &err));
                    }
                }
                Ok(Some(Resolved::Awaitable(awaitable))) => {
                    return ExtFunctionResult::Error(exc_py_to_monty(
                        self.py,
                        &sync_module_coroutine_error(name, &awaitable),
                    ));
                }
                Ok(None) => {}
                Err(err) => return ExtFunctionResult::Error(exc_py_to_monty(self.py, &err)),
            }
        }
        ExtFunctionResult::Return(MontyObject::dict(stubs))
    }

    /// Converts a `ClassInstance` directly or a dict to a module-named host instance.
    /// Public dict items cross eagerly: callables as `path.attr`, nested modules as submodules.
    /// Path-derived UUIDs preserve identity across imports and dumps.
    fn module_value(&self, path: &str, entry: &Bound<'py, PyAny>) -> PyResult<MontyObject> {
        let module = module_value_of(entry)?;
        if is_class_instance_wrapper(&module)? {
            return py_to_monty_value(&module, self.instances)
                .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc));
        }
        let mut attrs = Vec::new();
        for (attr, value) in module_attrs(&module)? {
            let value = if value.is_instance(external_module_class(self.py)?)? {
                self.module_value(&format!("{path}.{attr}"), &value)?
            } else if value.is_callable() && !is_class_type_wrapper(&value)? {
                MontyObject::function(format!("{path}.{attr}"), None)
            } else {
                py_to_monty_value(&value, self.instances)
                    .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc))?
            };
            attrs.push((MontyObject::string(attr), value));
        }
        let class = MontyObject::class_type(path, module_uuid("class", path), true, false, []);
        Ok(MontyObject::class_instance(class, module_uuid("instance", path), attrs))
    }
}

/// An `external_modules` entry after its factory (if any) ran.
enum Resolved<'py> {
    /// An `ExternalModule`, installed in the feed's cache when a factory made it.
    Module(Bound<'py, PyAny>),
    /// A factory's awaitable (a coroutine, a future, ...), still to be awaited
    /// by the async loop.
    Awaitable(Bound<'py, PyAny>),
}

/// Rejects a factory awaitable in the sync pool, closing it (see [`close_coroutine`]).
pub(crate) fn sync_module_coroutine_error(name: &str, awaitable: &Bound<'_, PyAny>) -> PyErr {
    close_coroutine(awaitable);
    PyRuntimeError::new_err(format!(
        "external_modules['{name}']() returned an awaitable; async module factories require AsyncMonty"
    ))
}

/// Closes a factory coroutine that will never be awaited, avoiding the
/// "never awaited" warning; other awaitables have no close protocol.
fn close_coroutine(awaitable: &Bound<'_, PyAny>) {
    if is_coroutine(awaitable.py(), awaitable) {
        let _ = awaitable.call_method0("close");
    }
}

/// [`sync_module_coroutine_error`] for every awaitable of a
/// [`CallResult::ModuleCoroutine`]: all are closed, the error names the first.
pub(crate) fn sync_module_coroutines_error(py: Python<'_>, names: &[String], coros: &[Py<PyAny>]) -> PyErr {
    let errors: Vec<PyErr> = names
        .iter()
        .zip(coros)
        .map(|(name, coro)| sync_module_coroutine_error(name, coro.bind(py)))
        .collect();
    errors
        .into_iter()
        .next()
        .expect("a module coroutine names at least one module")
}

/// Dict modules have no callable host object: `__call__` raises `TypeError`, other methods `AttributeError`.
fn module_method_error(name: &str, method: &str) -> PyErr {
    if method == "__call__" {
        PyTypeError::new_err(format!("'{name}' object is not callable"))
    } else {
        PyAttributeError::new_err(format!("'{name}' object has no attribute '{method}'"))
    }
}

/// Validates module values, stubs and submodules, and registers dict stand-ins.
/// Dict keys must be identifiers for dotted host-function names. Classes are rejected
/// as module values; `ancestors` detects cycles before they can overflow the stack.
fn check_module_tree(
    path: &str,
    entry: &Bound<'_, PyAny>,
    stand_ins: &mut HashMap<MontyUuid, String>,
    ancestors: &mut Vec<(usize, String)>,
) -> PyResult<()> {
    let identity = entry.as_ptr() as usize;
    if let Some((_, ancestor)) = ancestors.iter().find(|(ptr, _)| *ptr == identity) {
        return Err(PyValueError::new_err(format!(
            "external_modules['{path}'] is external_modules['{ancestor}'] again: modules cannot nest cyclically"
        )));
    }
    let module = entry.getattr("module")?;
    if let Ok(class) = module.cast::<PyType>() {
        return Err(PyTypeError::new_err(format!(
            "external_modules['{path}'].module must be a dict or a ClassInstance, not the class {}",
            class.name()?
        )));
    }
    if !is_module_shape(&module)? {
        return Err(PyTypeError::new_err(format!(
            "external_modules['{path}'].module must be a dict or a ClassInstance, not {}",
            module.get_type().name()?
        )));
    }
    let stubs = entry.getattr("stubs")?;
    if !stubs.is_none() && !stubs.is_instance_of::<PyString>() {
        return Err(PyTypeError::new_err(format!(
            "external_modules['{path}'].stubs must be a str or None, not {}",
            stubs.get_type().name()?
        )));
    }
    if module.is_instance_of::<PyDict>() {
        stand_ins.insert(module_uuid("instance", path), path.to_owned());
        ancestors.push((identity, path.to_owned()));
        for (name, value) in module_attrs(&module)? {
            if value.is_instance(external_module_class(entry.py())?)? {
                // a submodule is imported by name, so it needs a module name too
                validate_module_name(&name)
                    .map_err(|err| PyValueError::new_err(format!("external_modules['{path}.{name}']: {err}")))?;
                check_module_tree(&format!("{path}.{name}"), &value, stand_ins, ancestors)?;
            }
        }
        ancestors.pop();
    }
    Ok(())
}

/// Collects `(dotted_path, source or None)` pairs for a module and its submodules.
fn collect_stubs(path: &str, entry: &Bound<'_, PyAny>, stubs: &mut Vec<(MontyObject, MontyObject)>) -> PyResult<()> {
    let source = entry.getattr("stubs")?;
    let source = if source.is_none() {
        MontyObject::none()
    } else {
        MontyObject::string(source.extract::<String>()?)
    };
    stubs.push((MontyObject::string(path), source));
    let module = module_value_of(entry)?;
    if module.is_instance_of::<PyDict>() {
        for (name, value) in module_attrs(&module)? {
            if value.is_instance(external_module_class(entry.py())?)? {
                collect_stubs(&format!("{path}.{name}"), &value, stubs)?;
            }
        }
    }
    Ok(())
}

fn module_value_of<'py>(entry: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    entry.getattr("module")
}

/// The submodule `name` of a module value: the `ExternalModule` a dict holds
/// under that public key, if any; a `ClassInstance` module has none.
fn submodule<'py>(module: &Bound<'py, PyAny>, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    match module_attr(module, name)? {
        Some(value) if value.is_instance(external_module_class(module.py())?)? => Ok(Some(value)),
        _ => Ok(None),
    }
}

/// The module name an `import` call carries, if it is well formed.
fn import_name(args: &CallArgs) -> Option<&str> {
    let mut args = args.args();
    let name = args.next()?;
    args.next().is_none().then(|| name.as_str()).flatten()
}

/// The module names an `import_stubs` call carries, if it is well formed.
fn asked_modules(args: &CallArgs) -> Option<Vec<String>> {
    let mut args = args.args();
    let names = args.next()?;
    if args.next().is_some() {
        return None;
    }
    names
        .items()?
        .iter()
        .map(|name| name.as_str().map(str::to_owned))
        .collect()
}

/// Reconstructs module-call arguments after a factory resolves.
fn single_arg(value: MontyObject) -> CallArgs {
    let mut args = CallArgs::new();
    args.push_arg(value);
    args
}

/// A module-mapping key as a Python `str`; `where_` names the mapping in the
/// `TypeError` for any other key.
fn module_key<'py>(key: &Bound<'py, PyAny>, where_: &str) -> PyResult<Bound<'py, PyString>> {
    key.cast::<PyString>()
        .cloned()
        .map_err(|_| PyTypeError::new_err(format!("{where_} keys must be str")))
}

fn external_module_class(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static EXTERNAL_MODULE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    EXTERNAL_MODULE.import(py, "pydantic_monty", "ExternalModule")
}

/// Whether `value` can stand for a module: a dict or a `ClassInstance` wrapper.
fn is_module_shape(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    Ok(value.is_instance_of::<PyDict>() || is_class_instance_wrapper(value)?)
}

/// The public item `attr` of a dict module; `None` when absent or private.
fn module_attr<'py>(module: &Bound<'py, PyAny>, attr: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    if attr.starts_with('_') {
        Ok(None)
    } else {
        module.cast::<PyDict>()?.get_item(attr)
    }
}

/// The public items of a dict module, in order; keys must be identifiers,
/// since a host function is named by its dotted path.
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

/// Derives stable class/instance UUIDs from `kind:name` using two FNV-1a hashes,
/// so imports keep their identity across dumps and processes.
fn module_uuid(kind: &str, name: &str) -> MontyUuid {
    let fnv = |seed: u64| {
        let mut hash = seed;
        for byte in kind.bytes().chain(once(b':')).chain(name.bytes()) {
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
    /// Factory awaitables finished by [`ExternalLookup::finish_after_module`].
    /// Imports require values, so await these together before answering: one for an
    /// import or call, all pending factories for a stub request.
    ModuleCoroutine {
        names: Vec<String>,
        coros: Vec<Py<PyAny>>,
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
    /// The `import_stubs` ask for `asked`, answered once every pending
    /// factory among them has resolved.
    Stubs { asked: Vec<String> },
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
