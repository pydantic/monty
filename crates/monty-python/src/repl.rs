use std::{
    ffi::CString,
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{
    ExtFunctionResult, LimitedTracker, MontyObject, MontyRepl as CoreMontyRepl, NameLookupResult, NoLimitTracker,
    ReplProgress, ReplStartError, ResourceTracker,
};
use monty::fs::MountTable;
use pyo3::{
    IntoPyObjectExt,
    exceptions::{PyRuntimeError, PyTypeError, PyValueError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyBytes, PyDict, PyList, PyModule, PyType},
};
use pyo3_async_runtimes::tokio::future_into_py;

use crate::{
    async_dispatch::{ReplCleanupNotifier, await_repl_transition, dispatch_loop_repl},
    convert::{get_docstring, monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    exceptions::MontyError,
    external::{ExternalFunctionRegistry, dispatch_method_call},
    limits::{CancellationFlag, FutureCancellationGuard, PySignalTracker, extract_limits},
    monty_cls::{EitherProgress, call_os_callback_parts, py_type_check},
    mount::OsHandler,
    print_target::PrintTarget,
};

/// Runtime REPL session holder for pyclass interoperability.
///
/// PyO3 classes cannot be generic, so this enum stores REPL sessions for both
/// resource tracker variants.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum EitherRepl {
    NoLimit(CoreMontyRepl<PySignalTracker<NoLimitTracker>>),
    Limited(CoreMontyRepl<PySignalTracker<LimitedTracker>>),
}

/// Tracks the REPL source context used for incremental type checking.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeCheckState {
    /// `committed_stubs` contains user-provided stub declarations plus snippets that
    /// have successfully committed to the REPL state.
    committed_stubs: String,
    /// `pending_snippet` is reserved
    /// for a `feed_start()` snippet that is paused behind a snapshot and must only
    /// become visible to the type checker if that snapshot chain completes.
    pending_snippet: Option<String>,
}

impl EitherRepl {
    /// Installs or clears the async cancellation flag on the underlying tracker.
    fn set_cancellation_flag(&mut self, cancel_flag: Option<CancellationFlag>) {
        match self {
            Self::NoLimit(repl) => repl.tracker_mut().set_cancellation_flag(cancel_flag),
            Self::Limited(repl) => repl.tracker_mut().set_cancellation_flag(cancel_flag),
        }
    }
}

/// Stateful no-replay REPL session.
///
/// Create with `MontyRepl()` then call `feed_run()` to execute snippets
/// incrementally against persistent heap and namespace state.
///
/// Uses `Mutex` for the inner REPL because `CoreMontyRepl` contains a `Heap`
/// with `Cell<usize>` (not `Sync`), and PyO3 requires `Send + Sync` for all
/// pyclass types. The mutex also prevents concurrent `feed_run()` calls.
#[pyclass(name = "MontyRepl", module = "pydantic_monty", frozen)]
#[derive(Debug)]
pub struct PyMontyRepl {
    repl: Mutex<Option<EitherRepl>>,
    dc_registry: DcRegistry,

    /// Name of the script being executed.
    #[pyo3(get)]
    pub script_name: String,

    /// Type-check context for this REPL session.
    ///
    /// None if type checking is disabled.
    type_check_state: Option<Mutex<TypeCheckState>>,
}

#[pymethods]
impl PyMontyRepl {
    /// Creates an empty REPL session ready to receive snippets via `feed_run()`.
    ///
    /// No code is parsed or executed at construction time — all execution
    /// is driven through `feed_run()`.
    ///
    /// When `type_check` is `True`, each snippet fed via `feed_run()`, `feed_run_async()`,
    /// or `feed_start()` is statically type-checked before execution. The accumulated code
    /// from previous snippets is used as stub context so the type checker knows about
    /// previously defined names.
    #[new]
    #[pyo3(signature = (*, script_name="main.py", limits=None, type_check=false, type_check_stubs=None, dataclass_registry=None))]
    fn new(
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
        type_check: bool,
        type_check_stubs: Option<&str>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        let dc_registry = DcRegistry::from_list(py, dataclass_registry)?;
        let script_name = script_name.to_string();

        let repl = if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            EitherRepl::Limited(CoreMontyRepl::new(&script_name, tracker))
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            EitherRepl::NoLimit(CoreMontyRepl::new(&script_name, tracker))
        };

        Ok(Self {
            repl: Mutex::new(Some(repl)),
            dc_registry,
            script_name,
            type_check_state: if type_check {
                Some(Mutex::new(TypeCheckState {
                    committed_stubs: type_check_stubs.map(Into::into).unwrap_or_default(),
                    pending_snippet: None,
                }))
            } else {
                None
            },
        })
    }

    /// Registers a dataclass type for proper isinstance() support on output.
    fn register_dataclass(&self, cls: &Bound<'_, PyType>) -> PyResult<()> {
        self.dc_registry.insert(cls)
    }

    /// Performs static type checking on the given code snippet.
    ///
    /// Checks the snippet in isolation using `prefix_code` as stub context.
    /// This does not use the accumulated code from previous `feed_run` calls —
    /// use `prefix_code` to provide any needed declarations.
    #[pyo3(signature = (code, prefix_code=None))]
    fn type_check(&self, py: Python<'_>, code: &str, prefix_code: Option<&str>) -> PyResult<()> {
        py_type_check(py, code, &self.script_name, prefix_code, "type_stubs.pyi")
    }

    /// Feeds and executes a single incremental REPL snippet.
    ///
    /// The snippet is compiled against existing session state and executed once
    /// without replaying previously fed snippets.
    ///
    /// When `external_functions` is provided, external function calls and name
    /// lookups are dispatched to the provided callables — matching the behavior
    /// of `Monty.run(external_functions=...)`.
    #[expect(clippy::too_many_arguments)]
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    fn feed_run<'py>(
        &self,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.run_type_check_if_enabled(py, code, skip_type_check)?;
        let input_values = extract_repl_inputs(inputs, &self.dc_registry)?;

        let print_target = PrintTarget::from_py(print_callback)?;

        let os_handler = OsHandler::from_run_args(py, mount, os)?;

        if external_functions.is_some() || os_handler.is_some() {
            let result = self.feed_run_with_externals(
                py,
                code,
                input_values,
                external_functions,
                os_handler.as_ref(),
                &print_target,
            );
            if result.is_ok() && !skip_type_check {
                self.append_to_committed_stubs(code);
            }
            return result;
        }

        let mut guard = self
            .repl
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;
        let repl = guard
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;

        // `with_writer` only holds any collector lock for the duration of the
        // VM call.
        let result = match repl {
            EitherRepl::NoLimit(repl) => print_target.with_writer(|w| repl.feed_run(code, input_values, w)),
            EitherRepl::Limited(repl) => print_target.with_writer(|w| repl.feed_run(code, input_values, w)),
        };

        let output = match result {
            Ok(v) => v,
            Err(e) => {
                return Err(MontyError::new_err(py, e));
            }
        };

        if !skip_type_check {
            self.append_to_committed_stubs(code);
        }
        monty_to_py(py, &output, &self.dc_registry).map(|obj| obj.into_bound(py))
    }

    /// Starts executing an incremental snippet, yielding snapshots for external calls.
    ///
    /// Unlike `feed_run()`, which handles external function dispatch internally via a loop,
    /// `feed_start()` returns a snapshot object whenever the code needs an external function
    /// call, OS call, name lookup, or future resolution. The caller then provides the result
    /// via `snapshot.resume(...)`, which returns the next snapshot or `MontyComplete`.
    ///
    /// This enables the same iterative start/resume pattern used by `Monty.start()`,
    /// including support for async external functions via `FutureSnapshot`.
    ///
    /// When `mount` or `os` is provided, OS calls are resolved automatically using
    /// the same logic as [`Self::feed_run`] and the method only returns a snapshot
    /// when a non-OS event is reached. The auto-dispatch does **not** persist
    /// across subsequent `snapshot.resume()` calls.
    #[expect(clippy::too_many_arguments)]
    #[pyo3(signature = (code, *, inputs=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    fn feed_start<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.get();
        this.run_type_check_if_enabled(py, code, skip_type_check)?;
        let input_values = extract_repl_inputs(inputs, &this.dc_registry)?;

        let print_target = PrintTarget::from_py(print_callback)?;

        // Validate mount + os BEFORE touching the REPL so validation errors
        // leave the REPL untouched.
        let os_handler = OsHandler::from_run_args(py, mount, os)?;

        let repl = this.take_repl()?;
        if !skip_type_check {
            this.set_pending_type_check(code);
        }
        let repl_owner: Py<Self> = slf.clone().unbind();

        let code_owned = code.to_owned();
        let inputs_owned = input_values;
        let dc_registry = this.dc_registry.clone_ref(py);
        let script_name = this.script_name.clone();

        // Each transition builds its own writer via `with_writer` so any
        // collector lock is only held during the VM call.
        macro_rules! feed_start_impl {
            ($repl:expr, $variant:ident) => {{
                let result = py
                    .detach(|| print_target.with_writer(|writer| $repl.feed_start(&code_owned, inputs_owned, writer)));
                let progress = match result {
                    Ok(p) => p,
                    Err(e) => {
                        let err = *e;
                        this.put_repl_after_rollback(EitherRepl::from_core(err.repl));
                        return Err(MontyError::new_err(py, err.error));
                    }
                };
                // When mount/os is configured, consume OS-call events internally
                // until we reach the first non-OS event. Mounts are taken inside
                // the helper and put back on every exit path; the REPL is
                // rolled back via `put_repl_after_rollback` on resume errors.
                let progress = if let Some(handler) = &os_handler {
                    match drive_repl_progress_through_os_calls(
                        py,
                        progress,
                        handler,
                        &print_target,
                        &this.dc_registry,
                        this,
                    ) {
                        Ok(p) => p,
                        Err(e) => return Err(e),
                    }
                } else {
                    progress
                };
                let either = EitherProgress::$variant(progress, repl_owner);
                either.progress_or_complete(py, script_name, print_target, dc_registry)
            }};
        }

        match repl {
            EitherRepl::NoLimit(repl) => feed_start_impl!(repl, ReplNoLimit),
            EitherRepl::Limited(repl) => feed_start_impl!(repl, ReplLimited),
        }
    }

    /// Feeds and executes a snippet asynchronously, supporting async external functions.
    ///
    /// Returns a Python awaitable that drives the async dispatch loop.
    /// Unlike `feed_run()`, this handles external functions that return coroutines
    /// by awaiting them on the Python event loop. VM resume calls are offloaded
    /// to a thread pool via `spawn_blocking` to avoid blocking the event loop.
    ///
    /// The REPL is taken lazily when the returned awaitable first starts running,
    /// not when the awaitable is created. This prevents abandoned awaitables from
    /// stealing REPL state before any async work begins.
    ///
    /// # Returns
    /// A Python coroutine that resolves to the result of the snippet.
    ///
    /// # Raises
    /// Various Python exceptions matching what the code would raise.
    #[expect(clippy::too_many_arguments)]
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, os=None, skip_type_check=false))]
    fn feed_run_async<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Some(ref os_cb) = os
            && !os_cb.bind(py).is_callable()
        {
            let t = os_cb.bind(py).get_type().name()?;
            let msg = format!("TypeError: '{t}' object is not callable");
            return Err(PyTypeError::new_err(msg));
        }

        let this = slf.get();
        this.run_type_check_if_enabled(py, code, skip_type_check)?;
        if !skip_type_check {
            this.set_pending_type_check(code);
        }
        let input_values = extract_repl_inputs(inputs, &this.dc_registry)?;
        let dc_registry = this.dc_registry.clone_ref(py);
        let ext_fns = external_functions.map(|d| d.clone().unbind());
        let repl_owner: Py<Self> = slf.clone().unbind();
        let code_owned = code.to_owned();
        let print_target = PrintTarget::from_py(print_callback)?;

        PyReplAsyncAwaitable::new_py_any(
            py,
            ReplAsyncStart {
                repl_owner,
                code: code_owned,
                input_values,
                external_functions: ext_fns,
                os,
                dc_registry,
                print_target,
            },
        )
    }

    /// Serializes this REPL session to bytes.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        #[derive(serde::Serialize)]
        struct SerializedRepl<'a> {
            repl: &'a EitherRepl,
            script_name: &'a str,
            type_check_state: Option<&'a TypeCheckState>,
        }

        let guard = self.repl.lock().unwrap_or_else(PoisonError::into_inner);
        let repl = guard
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;
        let type_check_state_guard = self
            .type_check_state
            .as_ref()
            .map(|m| m.lock().unwrap_or_else(PoisonError::into_inner));

        let serialized = SerializedRepl {
            repl,
            script_name: &self.script_name,
            type_check_state: type_check_state_guard.as_deref(),
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Restores a REPL session from `dump()` bytes.
    ///
    /// Type checking state is restored from the serialized data, so type checking
    /// continues to work correctly for subsequent `feed_run` calls.
    #[staticmethod]
    #[pyo3(signature = (data, *, dataclass_registry=None))]
    fn load(
        py: Python<'_>,
        data: &Bound<'_, PyBytes>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        #[derive(serde::Deserialize)]
        struct SerializedReplOwned {
            repl: EitherRepl,
            script_name: String,
            type_check_state: Option<TypeCheckState>,
        }

        #[derive(serde::Deserialize)]
        struct SerializedReplOwnedLegacy {
            repl: EitherRepl,
            script_name: String,
            type_check_stubs: Option<String>,
        }

        let bytes = data.as_bytes();
        let (repl, script_name, type_check_state) = if let Ok(s) = postcard::from_bytes::<SerializedReplOwned>(bytes) {
            (s.repl, s.script_name, s.type_check_state)
        } else {
            let s: SerializedReplOwnedLegacy =
                postcard::from_bytes(bytes).map_err(|e| PyValueError::new_err(e.to_string()))?;
            let state = s.type_check_stubs.map(|committed_stubs| TypeCheckState {
                committed_stubs,
                pending_snippet: None,
            });
            (s.repl, s.script_name, state)
        };

        Ok(Self {
            repl: Mutex::new(Some(repl)),
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            script_name,
            type_check_state: type_check_state.map(Mutex::new),
        })
    }

    fn __repr__(&self) -> String {
        format!("MontyRepl(script_name='{}')", self.script_name)
    }
}

/// Internal awaitable wrapper for `MontyRepl.feed_run_async()`.
///
/// `future_into_py()` eagerly schedules the Rust future it wraps. For REPL
/// execution that is too early because simply creating the awaitable would take
/// ownership of the REPL. This wrapper defers future creation until Python
/// actually awaits the object, preventing discarded awaitables from stealing
/// REPL state.
#[pyclass(name = "MontyReplAsyncAwaitable", module = "pydantic_monty")]
struct PyReplAsyncAwaitable {
    start: Mutex<Option<ReplAsyncStart>>,
    future: Mutex<Option<Py<PyAny>>>,
    cleanup_waiter: Mutex<Option<Py<PyAny>>>,
}

/// Captures everything needed to lazily start an async REPL snippet.
struct ReplAsyncStart {
    repl_owner: Py<PyMontyRepl>,
    code: String,
    input_values: Vec<(String, MontyObject)>,
    external_functions: Option<Py<PyDict>>,
    os: Option<Py<PyAny>>,
    dc_registry: DcRegistry,
    print_target: PrintTarget,
}

/// Signals the per-await cleanup future unless normal REPL restoration takes over.
///
/// If the Python task is cancelled before the async snippet successfully takes
/// REPL ownership, no restore path runs and the cancellation wrapper would hang
/// forever waiting for cleanup. This guard resolves that wait future on drop
/// for those early-exit paths only.
struct CleanupStartGuard {
    cleanup_notifier: ReplCleanupNotifier,
    armed: bool,
}

impl CleanupStartGuard {
    /// Creates a new armed cleanup guard.
    fn new(cleanup_notifier: ReplCleanupNotifier) -> Self {
        Self {
            cleanup_notifier,
            armed: true,
        }
    }

    /// Disables drop-time signalling once the REPL has been taken.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CleanupStartGuard {
    fn drop(&mut self) {
        if self.armed {
            self.cleanup_notifier.finish();
        }
    }
}

impl ReplAsyncStart {
    /// Builds the real Python future for this REPL snippet the first time it is awaited.
    fn into_future(self, py: Python<'_>) -> PyResult<(Py<PyAny>, Py<PyAny>)> {
        let Self {
            repl_owner,
            code,
            input_values,
            external_functions,
            os,
            dc_registry,
            print_target,
        } = self;

        let (event_loop, cleanup_waiter) = create_cleanup_waiter(py)?;
        let cleanup_notifier = ReplCleanupNotifier::new(event_loop, cleanup_waiter.clone_ref(py));
        let start_guard = CleanupStartGuard::new(cleanup_notifier.clone());
        let start_target = print_target.clone_handle(py);
        let future = future_into_py(py, async move {
            let mut start_guard = start_guard;
            let cancellation_flag = Arc::new(AtomicBool::new(false));
            let mut cancellation_guard = FutureCancellationGuard::new(cancellation_flag.clone());
            let mut repl = Python::attach(|py| repl_owner.bind(py).get().take_repl())?;
            start_guard.disarm();
            repl.set_cancellation_flag(Some(cancellation_flag));

            let result = match repl {
                EitherRepl::NoLimit(repl) => {
                    let progress =
                        await_repl_transition(&repl_owner, cleanup_notifier.clone(), start_target, move |target| {
                            target.with_writer(|writer| repl.feed_start(&code, input_values, writer))
                        })
                        .await?;
                    dispatch_loop_repl(
                        progress,
                        repl_owner,
                        cleanup_notifier,
                        external_functions,
                        os,
                        dc_registry,
                        print_target,
                    )
                    .await
                }
                EitherRepl::Limited(repl) => {
                    let progress =
                        await_repl_transition(&repl_owner, cleanup_notifier.clone(), start_target, move |target| {
                            target.with_writer(|writer| repl.feed_start(&code, input_values, writer))
                        })
                        .await?;
                    dispatch_loop_repl(
                        progress,
                        repl_owner,
                        cleanup_notifier,
                        external_functions,
                        os,
                        dc_registry,
                        print_target,
                    )
                    .await
                }
            };
            cancellation_guard.disarm();
            result
        })?;
        Ok((future.unbind(), cleanup_waiter))
    }
}

impl PyReplAsyncAwaitable {
    /// Creates a lazy awaitable for a pending REPL async snippet.
    fn new_py_any(py: Python<'_>, start: ReplAsyncStart) -> PyResult<Bound<'_, PyAny>> {
        let slf = Self {
            start: Mutex::new(Some(start)),
            future: Mutex::new(None),
            cleanup_waiter: Mutex::new(None),
        };
        slf.into_bound_py_any(py)
    }

    /// Returns the inner Python future and its cleanup waiter, creating them on first use.
    fn get_or_start_future(&self, py: Python<'_>) -> PyResult<(Py<PyAny>, Py<PyAny>)> {
        if let Some(future) = self
            .future
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|future| future.clone_ref(py))
        {
            let cleanup_waiter = self
                .cleanup_waiter
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(|cleanup_waiter| cleanup_waiter.clone_ref(py))
                .ok_or_else(|| PyRuntimeError::new_err("Awaitable cleanup waiter is missing"))?;
            return Ok((future, cleanup_waiter));
        }

        let start = {
            let mut start_guard = self.start.lock().unwrap_or_else(PoisonError::into_inner);
            start_guard.take()
        };

        let Some(start) = start else {
            return self
                .future
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(|future| future.clone_ref(py))
                .zip(
                    self.cleanup_waiter
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_ref()
                        .map(|cleanup_waiter| cleanup_waiter.clone_ref(py)),
                )
                .ok_or_else(|| PyRuntimeError::new_err("Awaitable is currently starting"));
        };

        let (future, cleanup_waiter) = start.into_future(py)?;
        let mut future_guard = self.future.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = future_guard.as_ref() {
            let cleanup_waiter = self
                .cleanup_waiter
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(|cleanup_waiter| cleanup_waiter.clone_ref(py))
                .ok_or_else(|| PyRuntimeError::new_err("Awaitable cleanup waiter is missing"))?;
            Ok((existing.clone_ref(py), cleanup_waiter))
        } else {
            *future_guard = Some(future.clone_ref(py));
            let mut cleanup_guard = self.cleanup_waiter.lock().unwrap_or_else(PoisonError::into_inner);
            *cleanup_guard = Some(cleanup_waiter.clone_ref(py));
            Ok((future, cleanup_waiter))
        }
    }
}

#[pymethods]
impl PyReplAsyncAwaitable {
    /// Returns the iterator used by Python's await protocol.
    fn __await__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (future, cleanup_waiter) = self.get_or_start_future(py)?;
        let wrapped = wrap_future_with_cleanup(py, future, cleanup_waiter)?;
        wrapped.bind(py).call_method0("__await__")
    }
}

/// Creates an event-loop future that becomes ready once REPL cleanup finishes.
fn create_cleanup_waiter(py: Python<'_>) -> PyResult<(Py<PyAny>, Py<PyAny>)> {
    let event_loop = py.import("asyncio")?.call_method0("get_running_loop")?;
    let cleanup_waiter = event_loop.call_method0("create_future")?.unbind();
    Ok((event_loop.unbind(), cleanup_waiter))
}

/// Wraps the inner Rust future so Python cancellation waits for REPL restoration.
fn wrap_future_with_cleanup(py: Python<'_>, future: Py<PyAny>, cleanup_waiter: Py<PyAny>) -> PyResult<Py<PyAny>> {
    get_repl_cancel_wrapper(py)?
        .call1((future, cleanup_waiter))
        .map(Bound::unbind)
}

/// Returns the cached Python helper used to await REPL cleanup on cancellation.
fn get_repl_cancel_wrapper(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static REPL_CANCEL_WRAPPER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    REPL_CANCEL_WRAPPER
        .get_or_try_init(py, || {
            let code = CString::new(
                r"import asyncio

async def await_repl_with_cleanup(future, cleanup_waiter):
    try:
        return await future
    except asyncio.CancelledError:
        future.cancel()
        await asyncio.shield(cleanup_waiter)
        raise
",
            )
            .expect("helper module source must not contain NUL bytes");
            let module = PyModule::from_code(py, code.as_c_str(), c"monty_repl_async.py", c"monty_repl_async")?;
            Ok(module.getattr("await_repl_with_cleanup")?.unbind())
        })
        .map(|wrapper| wrapper.bind(py))
}

impl PyMontyRepl {
    /// Runs type checking on the new snippet if enabled and not skipped.
    ///
    /// Uses `type_check_stubs + accumulated_code` as stubs so that error line numbers
    /// in the diagnostics refer to lines in the new snippet, not the prefix context.
    fn run_type_check_if_enabled(&self, py: Python<'_>, code: &str, skip: bool) -> PyResult<()> {
        if skip {
            return Ok(());
        }
        let Some(state_mutex) = &self.type_check_state else {
            return Ok(());
        };
        let state = state_mutex.lock().unwrap_or_else(PoisonError::into_inner);
        let stubs_ref = if state.committed_stubs.is_empty() {
            None
        } else {
            Some(state.committed_stubs.as_str())
        };
        py_type_check(py, code, &self.script_name, stubs_ref, "repl_type_stubs.pyi")
    }

    /// Appends a snippet directly to committed type-check stubs.
    ///
    /// Used by immediate-execution paths where the snippet is already known to
    /// have committed before control returns to Python.
    fn append_to_committed_stubs(&self, code: &str) {
        if let Some(state_mutex) = &self.type_check_state {
            let mut state = state_mutex.lock().unwrap_or_else(PoisonError::into_inner);
            state.committed_stubs.push('\n');
            state.committed_stubs.push_str(code);
        }
    }

    /// Records a `feed_start()` snippet until it either commits or rolls back.
    fn set_pending_type_check(&self, code: &str) {
        if let Some(state_mutex) = &self.type_check_state {
            let mut state = state_mutex.lock().unwrap_or_else(PoisonError::into_inner);
            debug_assert!(
                state.pending_snippet.is_none(),
                "pending REPL type-check snippet must be cleared before starting a new one"
            );
            state.pending_snippet = Some(code.to_owned());
        }
    }

    /// Moves the pending `feed_start()` snippet into committed type-check state.
    pub(crate) fn commit_pending_type_check(&self) {
        if let Some(state_mutex) = &self.type_check_state {
            let mut state = state_mutex.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(code) = state.pending_snippet.take() {
                state.committed_stubs.push('\n');
                state.committed_stubs.push_str(&code);
            }
        }
    }

    /// Discards any in-flight `feed_start()` snippet during rollback paths.
    pub(crate) fn discard_pending_type_check(&self) {
        if let Some(state_mutex) = &self.type_check_state {
            let mut state = state_mutex.lock().unwrap_or_else(PoisonError::into_inner);
            state.pending_snippet = None;
        }
    }

    /// Returns a clone of the current type-check state for snapshot serialization.
    pub(crate) fn type_check_state_clone(&self) -> Option<TypeCheckState> {
        self.type_check_state
            .as_ref()
            .map(|state_mutex| state_mutex.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }

    /// Executes a REPL snippet with external function and OS call support.
    ///
    /// Uses the iterative `feed_start` / resume loop to handle external function
    /// calls and name lookups, matching the same dispatch logic as `Monty.run()`.
    ///
    /// `feed_start` consumes the REPL, so we temporarily take it out of the mutex
    /// (leaving `None`) and restore it on both success and error paths.
    fn feed_run_with_externals<'py>(
        &self,
        py: Python<'py>,
        code: &str,
        input_values: Vec<(String, MontyObject)>,
        external_functions: Option<&Bound<'_, PyDict>>,
        os_handler: Option<&OsHandler>,
        print_target: &PrintTarget,
    ) -> PyResult<Bound<'py, PyAny>> {
        let repl = self.take_repl()?;

        let result = match repl {
            EitherRepl::NoLimit(repl) => self.feed_start_loop(
                py,
                repl,
                code,
                input_values,
                external_functions,
                os_handler,
                print_target,
            ),
            EitherRepl::Limited(repl) => self.feed_start_loop(
                py,
                repl,
                code,
                input_values,
                external_functions,
                os_handler,
                print_target,
            ),
        };

        // On error, the REPL is already restored inside `feed_start_loop`.
        match result {
            Ok((output, restored_repl)) => {
                self.put_repl(restored_repl);
                monty_to_py(py, &output, &self.dc_registry).map(|obj| obj.into_bound(py))
            }
            Err(err) => Err(err),
        }
    }

    /// Runs the feed_start / resume loop for a specific resource tracker type.
    ///
    /// Handles filesystem mounts via the [`OsHandler`] take/put_back lifecycle:
    /// mounts are taken at the start and put back on all exit paths.
    ///
    /// Returns the output value and the restored REPL enum variant, or a Python error.
    #[expect(clippy::too_many_arguments)]
    fn feed_start_loop<T: ResourceTracker + Send>(
        &self,
        py: Python<'_>,
        repl: CoreMontyRepl<T>,
        code: &str,
        input_values: Vec<(String, MontyObject)>,
        external_functions: Option<&Bound<'_, PyDict>>,
        os_handler: Option<&OsHandler>,
        print_target: &PrintTarget,
    ) -> PyResult<(MontyObject, EitherRepl)>
    where
        EitherRepl: FromCoreRepl<T>,
    {
        // Take mounts out of shared slots for zero-overhead execution.
        let mut mount_table: Option<MountTable> = os_handler.map(OsHandler::take).transpose()?;
        let fallback = os_handler.and_then(|h| h.fallback.as_ref());

        // Helper: put mounts back into shared slots.
        let put_back = |table: Option<MountTable>| {
            if let (Some(h), Some(table)) = (os_handler, table) {
                h.put_back(table);
            }
        };

        macro_rules! restore_err {
            ($e:expr) => {{
                put_back(mount_table);
                let err: ReplStartError<T> = *$e;
                self.put_repl_after_rollback(EitherRepl::from_core(err.repl));
                return Err(MontyError::new_err(py, err.error));
            }};
        }

        let code_owned = code.to_owned();
        let mut progress =
            match py.detach(|| print_target.with_writer(|w| repl.feed_start(&code_owned, input_values, w))) {
                Ok(p) => p,
                Err(e) => restore_err!(e),
            };

        loop {
            match progress {
                ReplProgress::Complete { repl, value } => {
                    put_back(mount_table);
                    return Ok((value, EitherRepl::from_core(repl)));
                }
                ReplProgress::FunctionCall(call) => {
                    let return_value = if call.method_call {
                        dispatch_method_call(py, &call.function_name, &call.args, &call.kwargs, &self.dc_registry)
                    } else if let Some(ext_fns) = external_functions {
                        let registry = ExternalFunctionRegistry::new(py, ext_fns, &self.dc_registry);
                        registry.call(&call.function_name, &call.args, &call.kwargs)
                    } else {
                        let msg = format!(
                            "External function '{}' called but no external_functions provided",
                            call.function_name
                        );
                        self.put_repl(EitherRepl::from_core(call.into_repl()));
                        put_back(mount_table);
                        return Err(PyRuntimeError::new_err(msg));
                    };

                    progress = match py.detach(|| print_target.with_writer(|w| call.resume(return_value, w))) {
                        Ok(p) => p,
                        Err(e) => restore_err!(e),
                    };
                }
                ReplProgress::NameLookup(lookup) => {
                    let result = if let Some(ext_fns) = external_functions
                        && let Some(value) = ext_fns.get_item(&lookup.name)?
                    {
                        NameLookupResult::Value(MontyObject::Function {
                            name: lookup.name.clone(),
                            docstring: get_docstring(&value),
                        })
                    } else {
                        NameLookupResult::Undefined
                    };

                    progress = match py.detach(|| print_target.with_writer(|w| lookup.resume(result, w))) {
                        Ok(p) => p,
                        Err(e) => restore_err!(e),
                    };
                }
                ReplProgress::OsCall(call) => {
                    // `handle_repl_os_call` can fail during Python⇄Monty conversion of
                    // args/results. The OS call still owns the REPL handle — extract
                    // it via `into_repl` and put mounts back so neither leaks.
                    let result: ExtFunctionResult =
                        match handle_repl_os_call(py, &call, mount_table.as_mut(), fallback, &self.dc_registry) {
                            Ok(r) => r,
                            Err(e) => {
                                put_back(mount_table);
                                self.put_repl_after_rollback(EitherRepl::from_core(call.into_repl()));
                                return Err(e);
                            }
                        };

                    progress = match py.detach(|| print_target.with_writer(|w| call.resume(result, w))) {
                        Ok(p) => p,
                        Err(e) => restore_err!(e),
                    };
                }
                ReplProgress::ResolveFutures(state) => {
                    self.put_repl(EitherRepl::from_core(state.into_repl()));
                    put_back(mount_table);
                    return Err(PyRuntimeError::new_err(
                        "async futures not supported with `MontyRepl.feed_run`",
                    ));
                }
            }
        }
    }

    /// Takes the REPL out of the mutex for `feed_start` (which consumes self),
    /// leaving `None` until the REPL is restored via `put_repl`.
    pub(crate) fn take_repl(&self) -> PyResult<EitherRepl> {
        let mut guard = self
            .repl
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;
        guard
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))
    }

    /// Creates an empty REPL owner for snapshot deserialization.
    ///
    /// The REPL mutex starts as `None` — the real REPL state lives inside the
    /// deserialized snapshot and will be restored via `put_repl` when the
    /// snapshot is resumed to completion.
    pub(crate) fn empty_owner(
        script_name: String,
        dc_registry: DcRegistry,
        type_check_state: Option<TypeCheckState>,
    ) -> Self {
        Self {
            repl: Mutex::new(None),
            dc_registry,
            script_name,
            type_check_state: type_check_state.map(Mutex::new),
        }
    }

    /// Restores a REPL into the mutex after `feed_start` completes successfully.
    fn put_repl(&self, repl: EitherRepl) {
        let mut repl = repl;
        repl.set_cancellation_flag(None);
        let mut guard = self.repl.lock().unwrap_or_else(PoisonError::into_inner);
        *guard = Some(repl);
    }

    /// Restores the REPL after a successful `feed_start()` completion.
    pub(crate) fn put_repl_after_commit(&self, repl: EitherRepl) {
        self.commit_pending_type_check();
        self.put_repl(repl);
    }

    /// Restores the REPL after a rollback path.
    pub(crate) fn put_repl_after_rollback(&self, repl: EitherRepl) {
        self.discard_pending_type_check();
        self.put_repl(repl);
    }
}

/// Converts a Python dict of `{name: value}` pairs into the `Vec<(String, MontyObject)>`
/// format expected by the core REPL's `feed_run` and `feed_start`.
fn extract_repl_inputs(
    inputs: Option<&Bound<'_, PyDict>>,
    dc_registry: &DcRegistry,
) -> PyResult<Vec<(String, MontyObject)>> {
    let Some(inputs) = inputs else {
        return Ok(vec![]);
    };
    inputs
        .iter()
        .map(|(key, value)| {
            let name = key.extract::<String>()?;
            let obj = py_to_monty(&value, dc_registry)?;
            Ok((name, obj))
        })
        .collect::<PyResult<_>>()
}

/// Auto-dispatches [`ReplProgress::OsCall`] events until a non-OS progress is reached.
///
/// Used by [`PyMontyRepl::feed_start`] and the snapshot `resume()` methods
/// when the caller supplies a `mount` or `os` argument. Mirrors
/// `drive_run_progress_through_os_calls` but also takes care of REPL rollback
/// when a resume call fails: on error the REPL is restored via
/// [`PyMontyRepl::put_repl_after_rollback`] before returning.
///
/// Mounts are taken lazily on the first OS call and put back on every exit
/// path. This avoids spurious mount-contention failures for progress that
/// never reaches an OS call, while still restoring the REPL on any failure
/// after the OS-dispatch path is entered.
pub(crate) fn drive_repl_progress_through_os_calls<T: ResourceTracker + Send>(
    py: Python<'_>,
    mut progress: ReplProgress<T>,
    handler: &OsHandler,
    print_target: &PrintTarget,
    dc_registry: &DcRegistry,
    repl_this: &PyMontyRepl,
) -> PyResult<ReplProgress<T>>
where
    EitherRepl: FromCoreRepl<T>,
{
    let mut mount_table: Option<MountTable> = None;
    let fallback = handler.fallback.as_ref();
    let put_back = |mount_table: &mut Option<MountTable>| {
        if let Some(table) = mount_table.take() {
            handler.put_back(table);
        }
    };
    loop {
        match progress {
            ReplProgress::OsCall(call) => {
                let table = if let Some(table) = mount_table.as_mut() {
                    Some(table)
                } else {
                    let table = match handler.take() {
                        Ok(table) => table,
                        Err(e) => {
                            repl_this.put_repl_after_rollback(EitherRepl::from_core(call.into_repl()));
                            return Err(e);
                        }
                    };
                    Some(mount_table.insert(table))
                };
                let result = match handle_repl_os_call(py, &call, table, fallback, dc_registry) {
                    Ok(r) => r,
                    Err(e) => {
                        put_back(&mut mount_table);
                        // handle_repl_os_call can fail during Python⇄Monty
                        // conversion of args/results. The OS call still owns
                        // the REPL handle — extract it via `into_repl` and
                        // roll back so the caller's REPL remains usable.
                        repl_this.put_repl_after_rollback(EitherRepl::from_core(call.into_repl()));
                        return Err(e);
                    }
                };
                progress = match py.detach(|| print_target.with_writer(|w| call.resume(result, w))) {
                    Ok(p) => p,
                    Err(e) => {
                        put_back(&mut mount_table);
                        let err = *e;
                        repl_this.put_repl_after_rollback(EitherRepl::from_core(err.repl));
                        return Err(MontyError::new_err(py, err.error));
                    }
                };
            }
            other => {
                put_back(&mut mount_table);
                return Ok(other);
            }
        }
    }
}

/// Handles an OS call from the REPL, dispatching to the mount table if available,
/// then to the fallback callback, and finally to [`OsFunction::on_no_handler`].
///
/// `mount_table` is `Option<&mut MountTable>` so callers can pass `None` when no
/// mount is configured. This matches [`handle_mount_os_call`] (which always has
/// a mount table) while remaining ergonomic from the `Option<MountTable>`-holding
/// loops in `feed_start_loop` and `drive_repl_progress_through_os_calls`.
fn handle_repl_os_call<T: ResourceTracker>(
    py: Python<'_>,
    call: &monty::ReplOsCall<T>,
    mount_table: Option<&mut MountTable>,
    fallback: Option<&Py<PyAny>>,
    dc_registry: &DcRegistry,
) -> PyResult<ExtFunctionResult> {
    if let Some(table) = mount_table {
        match table.handle_os_call(call.function, &call.args, &call.kwargs) {
            Some(Ok(obj)) => return Ok(obj.into()),
            Some(Err(mount_err)) => return Ok(mount_err.into_exception().into()),
            None => {} // Intentional: unmounted paths fall through to `os=`.
        }
    }

    if let Some(fb) = fallback {
        return call_os_callback_parts(
            py,
            &call.function.to_string(),
            &call.args,
            &call.kwargs,
            fb.bind(py),
            dc_registry,
            || call.function.on_no_handler(&call.args).into(),
        );
    }

    Ok(call.function.on_no_handler(&call.args).into())
}

/// Helper trait to convert a typed `CoreMontyRepl<T>` back into the
/// type-erased `EitherRepl` enum.
pub(crate) trait FromCoreRepl<T: ResourceTracker> {
    /// Wraps a core REPL into the appropriate `EitherRepl` variant.
    fn from_core(repl: CoreMontyRepl<T>) -> Self;
}

impl FromCoreRepl<PySignalTracker<NoLimitTracker>> for EitherRepl {
    fn from_core(repl: CoreMontyRepl<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimit(repl)
    }
}

impl FromCoreRepl<PySignalTracker<LimitedTracker>> for EitherRepl {
    fn from_core(repl: CoreMontyRepl<PySignalTracker<LimitedTracker>>) -> Self {
        Self::Limited(repl)
    }
}
