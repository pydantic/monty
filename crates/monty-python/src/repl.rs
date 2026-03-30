use std::{
    ffi::CString,
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{
    ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRepl as CoreMontyRepl, NameLookupResult,
    NoLimitTracker, PrintWriter, ReplProgress, ReplStartError, ResourceTracker,
};
use monty::ExcType;
use pyo3::{
    IntoPyObjectExt,
    exceptions::{PyRuntimeError, PyTypeError, PyValueError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyBytes, PyDict, PyList, PyModule, PyTuple, PyType},
};
use pyo3_async_runtimes::tokio::future_into_py;
use send_wrapper::SendWrapper;

use crate::{
    async_dispatch::{ReplCleanupNotifier, await_repl_transition, dispatch_loop_repl, with_print_writer},
    convert::{get_docstring, monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    exceptions::{MontyError, exc_py_to_monty},
    external::{ExternalFunctionRegistry, dispatch_method_call},
    limits::{CancellationFlag, FutureCancellationGuard, PySignalTracker, extract_limits},
    monty_cls::{CallbackStringPrint, EitherProgress},
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
}

#[pymethods]
impl PyMontyRepl {
    /// Creates an empty REPL session ready to receive snippets via `feed_run()`.
    ///
    /// No code is parsed or executed at construction time — all execution
    /// is driven through `feed_run()`.
    #[new]
    #[pyo3(signature = (*, script_name="main.py", limits=None, dataclass_registry=None))]
    fn new(
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
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
        })
    }

    /// Registers a dataclass type for proper isinstance() support on output.
    fn register_dataclass(&self, cls: &Bound<'_, PyType>) -> PyResult<()> {
        self.dc_registry.insert(cls)
    }

    /// Feeds and executes a single incremental REPL snippet.
    ///
    /// The snippet is compiled against existing session state and executed once
    /// without replaying previously fed snippets.
    ///
    /// When `external_functions` is provided, external function calls and name
    /// lookups are dispatched to the provided callables — matching the behavior
    /// of `Monty.run(external_functions=...)`.
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, os=None))]
    fn feed_run<'py>(
        &self,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<Py<PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input_values = extract_repl_inputs(inputs, &self.dc_registry)?;

        let mut print_cb;
        let mut print_writer = match print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb);
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        if external_functions.is_some() || os.is_some() {
            return self.feed_run_with_externals(py, code, input_values, external_functions, os, print_writer);
        }

        let mut guard = self
            .repl
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;
        let repl = guard
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;

        let output = match repl {
            EitherRepl::NoLimit(repl) => repl.feed_run(code, input_values, print_writer.reborrow()),
            EitherRepl::Limited(repl) => repl.feed_run(code, input_values, print_writer.reborrow()),
        }
        .map_err(|e| MontyError::new_err(py, e))?;

        Ok(monty_to_py(py, &output, &self.dc_registry)?.into_bound(py))
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
    #[pyo3(signature = (code, *, inputs=None, print_callback=None))]
    fn feed_start<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        print_callback: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.get();
        let input_values = extract_repl_inputs(inputs, &this.dc_registry)?;

        let mut print_cb;
        let print_writer = match &print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb.clone_ref(py));
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };
        let mut print_output = SendWrapper::new(print_writer);

        let repl = this.take_repl()?;
        let repl_owner: Py<Self> = slf.clone().unbind();

        let code_owned = code.to_owned();
        let inputs_owned = input_values;
        let dc_registry = this.dc_registry.clone_ref(py);
        let script_name = this.script_name.clone();

        match repl {
            EitherRepl::NoLimit(repl) => {
                let progress = py
                    .detach(|| repl.feed_start(&code_owned, inputs_owned, print_output.reborrow()))
                    .map_err(|e| this.restore_repl_from_start_error(py, *e))?;
                let either = EitherProgress::ReplNoLimit(progress, repl_owner);
                either.progress_or_complete(py, script_name, print_callback, dc_registry)
            }
            EitherRepl::Limited(repl) => {
                let progress = py
                    .detach(|| repl.feed_start(&code_owned, inputs_owned, print_output.reborrow()))
                    .map_err(|e| this.restore_repl_from_start_error(py, *e))?;
                let either = EitherProgress::ReplLimited(progress, repl_owner);
                either.progress_or_complete(py, script_name, print_callback, dc_registry)
            }
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
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, os=None))]
    fn feed_run_async<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<Py<PyAny>>,
        os: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Some(ref os_cb) = os
            && !os_cb.bind(py).is_callable()
        {
            let t = os_cb.bind(py).get_type().name()?;
            let msg = format!("TypeError: '{t}' object is not callable");
            return Err(PyTypeError::new_err(msg));
        }

        let this = slf.get();
        let input_values = extract_repl_inputs(inputs, &this.dc_registry)?;
        let dc_registry = this.dc_registry.clone_ref(py);
        let ext_fns = external_functions.map(|d| d.clone().unbind());
        let repl_owner: Py<Self> = slf.clone().unbind();
        let code_owned = code.to_owned();

        PyReplAsyncAwaitable::new_py_any(
            py,
            ReplAsyncStart {
                repl_owner,
                code: code_owned,
                input_values,
                external_functions: ext_fns,
                os,
                dc_registry,
                print_callback,
            },
        )
    }

    /// Serializes this REPL session to bytes.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        #[derive(serde::Serialize)]
        struct SerializedRepl<'a> {
            repl: &'a EitherRepl,
            script_name: &'a str,
        }

        let guard = self.repl.lock().unwrap_or_else(PoisonError::into_inner);
        let repl = guard
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;

        let serialized = SerializedRepl {
            repl,
            script_name: &self.script_name,
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Restores a REPL session from `dump()` bytes.
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
        }

        let serialized: SerializedReplOwned =
            postcard::from_bytes(data.as_bytes()).map_err(|e| PyValueError::new_err(e.to_string()))?;

        Ok(Self {
            repl: Mutex::new(Some(serialized.repl)),
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            script_name: serialized.script_name,
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
    print_callback: Option<Py<PyAny>>,
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
            print_callback,
        } = self;

        let (event_loop, cleanup_waiter) = create_cleanup_waiter(py)?;
        let cleanup_notifier = ReplCleanupNotifier::new(event_loop, cleanup_waiter.clone_ref(py));
        let start_guard = CleanupStartGuard::new(cleanup_notifier.clone());
        let start_print_callback = print_callback.as_ref().map(|cb| cb.clone_ref(py));
        let future = future_into_py(py, async move {
            let mut start_guard = start_guard;
            let cancellation_flag = Arc::new(AtomicBool::new(false));
            let mut cancellation_guard = FutureCancellationGuard::new(cancellation_flag.clone());
            let mut repl = Python::attach(|py| repl_owner.bind(py).get().take_repl())?;
            start_guard.disarm();
            repl.set_cancellation_flag(Some(cancellation_flag));

            let result = match repl {
                EitherRepl::NoLimit(repl) => {
                    let progress = await_repl_transition(
                        &repl_owner,
                        cleanup_notifier.clone(),
                        start_print_callback,
                        move |print_callback| {
                            with_print_writer(print_callback, |writer| repl.feed_start(&code, input_values, writer))
                        },
                    )
                    .await?;
                    dispatch_loop_repl(
                        progress,
                        repl_owner,
                        cleanup_notifier,
                        external_functions,
                        os,
                        dc_registry,
                        print_callback,
                    )
                    .await
                }
                EitherRepl::Limited(repl) => {
                    let progress = await_repl_transition(
                        &repl_owner,
                        cleanup_notifier.clone(),
                        start_print_callback,
                        move |print_callback| {
                            with_print_writer(print_callback, |writer| repl.feed_start(&code, input_values, writer))
                        },
                    )
                    .await?;
                    dispatch_loop_repl(
                        progress,
                        repl_owner,
                        cleanup_notifier,
                        external_functions,
                        os,
                        dc_registry,
                        print_callback,
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
        os: Option<&Bound<'_, PyAny>>,
        mut print_writer: PrintWriter<'_>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut print_output = SendWrapper::new(&mut print_writer);

        let repl = self.take_repl()?;

        let result = match repl {
            EitherRepl::NoLimit(repl) => {
                self.feed_start_loop(py, repl, code, input_values, external_functions, os, &mut print_output)
            }
            EitherRepl::Limited(repl) => {
                self.feed_start_loop(py, repl, code, input_values, external_functions, os, &mut print_output)
            }
        };

        // On error, the REPL is already restored inside `restore_repl_from_start_error`.
        match result {
            Ok((output, restored_repl)) => {
                self.put_repl(restored_repl);
                Ok(monty_to_py(py, &output, &self.dc_registry)?.into_bound(py))
            }
            Err(err) => Err(err),
        }
    }

    /// Runs the feed_start / resume loop for a specific resource tracker type.
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
        os: Option<&Bound<'_, PyAny>>,
        print_output: &mut SendWrapper<&mut PrintWriter<'_>>,
    ) -> PyResult<(MontyObject, EitherRepl)>
    where
        EitherRepl: FromCoreRepl<T>,
    {
        let code_owned = code.to_owned();
        let mut progress = py
            .detach(|| repl.feed_start(&code_owned, input_values, print_output.reborrow()))
            .map_err(|e| self.restore_repl_from_start_error(py, *e))?;

        loop {
            match progress {
                ReplProgress::Complete { repl, value } => {
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
                        return Err(PyRuntimeError::new_err(msg));
                    };

                    progress = py
                        .detach(|| call.resume(return_value, print_output.reborrow()))
                        .map_err(|e| self.restore_repl_from_start_error(py, *e))?;
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

                    progress = py
                        .detach(|| lookup.resume(result, print_output.reborrow()))
                        .map_err(|e| self.restore_repl_from_start_error(py, *e))?;
                }
                ReplProgress::OsCall(call) => {
                    let result: ExtFunctionResult = if let Some(os_callback) = os {
                        let py_args: Vec<Py<PyAny>> = call
                            .args
                            .iter()
                            .map(|arg| monty_to_py(py, arg, &self.dc_registry))
                            .collect::<PyResult<_>>()?;
                        let py_args_tuple = PyTuple::new(py, py_args)?;

                        let py_kwargs = PyDict::new(py);
                        for (k, v) in &call.kwargs {
                            py_kwargs.set_item(
                                monty_to_py(py, k, &self.dc_registry)?,
                                monty_to_py(py, v, &self.dc_registry)?,
                            )?;
                        }

                        match os_callback.call1((call.function.to_string(), py_args_tuple, py_kwargs)) {
                            Ok(result) => py_to_monty(&result, &self.dc_registry)?.into(),
                            Err(err) => exc_py_to_monty(py, &err).into(),
                        }
                    } else {
                        MontyException::new(
                            ExcType::NotImplementedError,
                            Some(format!("OS function '{}' not implemented", call.function)),
                        )
                        .into()
                    };

                    progress = py
                        .detach(|| call.resume(result, print_output.reborrow()))
                        .map_err(|e| self.restore_repl_from_start_error(py, *e))?;
                }
                ReplProgress::ResolveFutures(state) => {
                    self.put_repl(EitherRepl::from_core(state.into_repl()));
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
    pub(crate) fn empty_owner(script_name: String, dc_registry: DcRegistry) -> Self {
        Self {
            repl: Mutex::new(None),
            dc_registry,
            script_name,
        }
    }

    /// Restores a REPL into the mutex after `feed_start` completes successfully.
    pub(crate) fn put_repl(&self, repl: EitherRepl) {
        let mut repl = repl;
        repl.set_cancellation_flag(None);
        let mut guard = self.repl.lock().unwrap_or_else(PoisonError::into_inner);
        *guard = Some(repl);
    }

    /// Extracts the REPL from a `ReplStartError`, restores it into `self.repl`,
    /// and returns the Python exception.
    fn restore_repl_from_start_error<T: ResourceTracker>(&self, py: Python<'_>, err: ReplStartError<T>) -> PyErr
    where
        EitherRepl: FromCoreRepl<T>,
    {
        self.put_repl(EitherRepl::from_core(err.repl));
        MontyError::new_err(py, err.error)
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
