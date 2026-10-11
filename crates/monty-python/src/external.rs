//! Resolves bare names and calls through `external_lookup`, and imports, stubs and
//! dotted calls through `external_modules`. [`ExternalLookup`] applies the same
//! callable/value rules to name lookups and subsequent calls. Object methods and
//! lazy attributes resolve through the session's [`InstanceStore`].

use monty_proto::python::{
    DecodedArena, InstanceStore, PyExternalModule, exc_py_to_monty, get_docstring, py_to_monty, py_to_monty_value,
};
use monty_types::{
    CallArgs, ExtFunctionResult, IMPORT_CALL, IMPORT_STUBS_CALL, MontyObject, MontyUuid, NameLookupResult,
    unstable::{self, MontyNode},
    validate_module_name,
};
use pyo3::{
    exceptions::{PyAttributeError, PyKeyError, PyRuntimeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    types::{PyDict, PyMapping, PyTuple, PyType},
};

use crate::{callback_context, exceptions::MontyConversionError};

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

    /// Dispatches a method call through the instance store.
    pub fn call_object(&self, function_name: &str, object_id: &MontyUuid, args: &CallArgs) -> ExtFunctionResult {
        dispatch_object_call(self.py, function_name, object_id, args, self.instances)
    }

    /// Like [`call_object`](Self::call_object) but returns `CallResult::Coroutine`
    /// when the method returns a coroutine.
    pub fn call_object_or_coroutine(&self, function_name: &str, object_id: &MontyUuid, args: &CallArgs) -> CallResult {
        dispatch_object_call_or_coroutine(self.py, function_name, object_id, args, self.instances)
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
                Some((dot, Resolved::Module(module))) => module.get().attr(self.py, &function_name[dot + 1..]),
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
    /// in this session. Returns awaitables for the async loop to await and install.
    fn resolve_module(&self, name: &str) -> PyResult<Option<Resolved<'py>>> {
        let Some(modules) = self.modules else {
            return Ok(None);
        };
        if let Some(module) = modules.resolved.bind(self.py).get_item(name)? {
            return Ok(Some(Resolved::Module(module.cast_into()?)));
        }
        let Some(entry) = mapping_entry(modules.entries.bind(self.py), name)? else {
            return Ok(None);
        };
        check_entry(name, &entry)?;
        if entry.is_instance_of::<PyExternalModule>() {
            return Ok(Some(Resolved::Module(entry.cast_into()?)));
        }
        let result = callback_context::call(self.py, || entry.call0())?;
        if is_awaitable(self.py, &result) {
            Ok(Some(Resolved::Awaitable(result)))
        } else {
            self.install_module(name, result).map(Resolved::Module).map(Some)
        }
    }

    /// Checks and caches a factory result for the rest of the session.
    fn install_module(&self, name: &str, module: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyExternalModule>> {
        let modules = self.modules.expect("a module was resolved, so the entries exist");
        if !module.is_instance_of::<PyExternalModule>() {
            return Err(PyTypeError::new_err(format!(
                "external_modules['{name}']() returned {}, not an ExternalModule",
                module.get_type().name()?
            )));
        }
        modules.resolved.bind(self.py).set_item(name, &module)?;
        Ok(module.cast_into()?)
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
            let Some(child) = submodule(self.py, &entry, part)? else {
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
            Ok(Some(Resolved::Module(module))) => CallResult::Sync(match self.module_value(name, module.get()) {
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
            Ok(Some(Resolved::Module(module))) => match self.module_value(name, module.get()) {
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
                Ok(Some(Resolved::Module(module))) => collect_stubs(self.py, name, module.get(), &mut stubs),
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

    /// The module value an import binds: the module's public attrs, with callables
    /// as host functions named by their dotted path (which a call resolves back
    /// through the module) and nested modules as submodules.
    fn module_value(&self, path: &str, module: &PyExternalModule) -> PyResult<MontyObject> {
        let mut attrs = Vec::new();
        for (name, value) in module.public_attrs(self.py) {
            let value = if let Ok(submodule) = value.cast::<PyExternalModule>() {
                self.module_value(&format!("{path}.{name}"), submodule.get())?
            } else if value.is_callable() {
                MontyObject::function(format!("{path}.{name}"), get_docstring(&value))
            } else {
                py_to_monty_value(&value, self.instances)
                    .map_err(|exc| MontyConversionError::value_conversion_err(self.py, exc))?
            };
            attrs.push((MontyObject::string(name), value));
        }
        Ok(MontyObject::module(path, module.uuid(), attrs))
    }
}

/// The `external_lookup=` and `external_modules=` dicts one feed captured:
/// the host side of the names and imports a snippet leaves to it. Held as
/// owned references so a snapshot can keep them for `resume_auto`.
#[derive(Default)]
pub(crate) struct HostNames {
    /// `external_lookup=`: host values by the bare name the snippet reads.
    pub(crate) lookup: Option<Py<PyDict>>,
    /// The session's `external_modules=`.
    pub(crate) modules: Option<HostModules>,
}

impl HostNames {
    /// Pairs the lookup a feed was called with and the session's modules.
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

/// A session's modules and cached factory results, shared by its feeds and their
/// `resume_auto` snapshots. The checkout's mapping is read live and an entry is
/// validated when a snippet first reaches it, so a host may add modules during the
/// session. A factory runs on first use and its validated result serves the rest of
/// the session; a failure is raised there and the next use runs it again.
pub(crate) struct HostModules {
    /// The checkout's mapping, read live, by the name the snippet imports: an
    /// `ExternalModule`, or the factory returning one.
    entries: Py<PyMapping>,
    /// What each factory returned, by module name: a factory runs once per
    /// session, at the first import, call or stubs ask that needs it.
    resolved: Py<PyDict>,
}

impl HostModules {
    /// Keeps the mapping of modules or zero-argument factories returning them; only
    /// its type is checked here, each entry when a snippet reaches it (see
    /// [`check_entry`]).
    pub(crate) fn capture(py: Python<'_>, entries: &Bound<'_, PyAny>) -> PyResult<Self> {
        match entries.cast::<PyMapping>() {
            Ok(mapping) => Ok(Self {
                entries: mapping.clone().unbind(),
                resolved: PyDict::new(py).unbind(),
            }),
            Err(_) => Err(PyTypeError::new_err(format!(
                "external_modules must be a mapping of module name to ExternalModule, not {}",
                entries.get_type().name()?
            ))),
        }
    }

    /// A second owner of the same dicts, for a snapshot's drive context.
    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            entries: self.entries.clone_ref(py),
            resolved: self.resolved.clone_ref(py),
        }
    }
}

/// Identifies module OS calls, which take precedence over mounts and `os=`.
/// Call only for OS events: an ordinary function named `import_stubs` is unrelated.
pub(crate) fn is_module_call(function_name: &str) -> bool {
    function_name == IMPORT_CALL || function_name == IMPORT_STUBS_CALL
}

/// An `external_modules` entry after its factory (if any) ran.
enum Resolved<'py> {
    /// An `ExternalModule`, installed in the session's cache when a factory made it.
    Module(Bound<'py, PyExternalModule>),
    /// A factory's awaitable (a coroutine, a future, ...), still to be awaited
    /// by the async loop.
    Awaitable(Bound<'py, PyAny>),
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

/// Validates a top-level entry the first time a snippet reaches it: a module name
/// (never a sandbox module's) mapped to an `ExternalModule` or a factory. Classes are
/// callable but construct instances, so they are refused as factories.
fn check_entry(name: &str, entry: &Bound<'_, PyAny>) -> PyResult<()> {
    validate_module_name(name).map_err(|err| PyValueError::new_err(err.to_string()))?;
    if entry.is_instance_of::<PyExternalModule>() {
        Ok(())
    } else if let Ok(class) = entry.cast::<PyType>() {
        Err(PyTypeError::new_err(format!(
            "external_modules['{name}'] must be an ExternalModule or a callable returning one, not the class {}",
            class.name()?
        )))
    } else if !entry.is_callable() {
        Err(PyTypeError::new_err(format!(
            "external_modules['{name}'] must be an ExternalModule or a callable returning one, not {}",
            entry.get_type().name()?
        )))
    } else {
        Ok(())
    }
}

/// The entry under `name`, `None` when the mapping lacks it.
fn mapping_entry<'py>(mapping: &Bound<'py, PyMapping>, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    match mapping.get_item(name) {
        Ok(entry) => Ok(Some(entry)),
        Err(err) if err.is_instance_of::<PyKeyError>(mapping.py()) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Collects `(dotted_path, source or None)` pairs for a module and its submodules.
fn collect_stubs(py: Python<'_>, path: &str, module: &PyExternalModule, stubs: &mut Vec<(MontyObject, MontyObject)>) {
    let source = module
        .stubs_source()
        .map_or_else(MontyObject::none, MontyObject::string);
    stubs.push((MontyObject::string(path), source));
    for (name, value) in module.public_attrs(py) {
        if let Ok(submodule) = value.cast::<PyExternalModule>() {
            collect_stubs(py, &format!("{path}.{name}"), submodule.get(), stubs);
        }
    }
}

/// The submodule `name` of `module`: the `ExternalModule` under that public key, if any.
fn submodule<'py>(
    py: Python<'py>,
    module: &Bound<'py, PyExternalModule>,
    name: &str,
) -> PyResult<Option<Bound<'py, PyExternalModule>>> {
    Ok(module
        .get()
        .attr(py, name)?
        .and_then(|value| value.cast_into::<PyExternalModule>().ok()))
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

/// Rejects a factory awaitable in the sync pool, closing it (see [`close_coroutine`]).
pub(crate) fn sync_module_coroutine_error(name: &str, awaitable: &Bound<'_, PyAny>) -> PyErr {
    close_coroutine(awaitable);
    PyRuntimeError::new_err(format!(
        "external_modules['{name}']() returned an awaitable; async module factories require AsyncMonty"
    ))
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

/// Closes a factory coroutine that will never be awaited, avoiding the
/// "never awaited" warning; other awaitables have no close protocol.
fn close_coroutine(awaitable: &Bound<'_, PyAny>) {
    if is_coroutine(awaitable.py(), awaitable) {
        let _ = awaitable.call_method0(intern!(awaitable.py(), "close"));
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
