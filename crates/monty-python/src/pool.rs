//! `MontyPool` — crash-isolated execution in a pool of `monty` subprocesses.
//!
//! A monty process can never be made fully crash-proof against memory errors
//! (stack overflow, allocator aborts), so `MontyPool` runs the interpreter in
//! worker subprocesses via the `monty-pool` crate: a crashed worker raises
//! [`MontyCrashedError`] and is replaced, and the host Python process is
//! never at risk.
//!
//! ```python
//! async with MontyPool() as pool:
//!     async with pool.checkout() as session:
//!         result = await session.feed_run_async('1 + 1')
//! ```
//!
//! All pool I/O runs off the GIL (and off the event loop via
//! `spawn_blocking` for the async methods); Python callbacks — external
//! functions, `os=`, `print_callback` — execute in this process exactly as
//! they do for in-process `MontyRepl`.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use ::monty::{ExcType, ExtFunctionResult, MontyException, MontyObject};
use monty_pool::{Checkout, MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, ReplConfig, ResumeValue, TurnEvent};
use pyo3::{
    exceptions::{PyRuntimeError, PyTimeoutError, PyTypeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyString, PyTuple},
};
use pyo3_async_runtimes::tokio::future_into_py;
use tokio::task::{JoinSet, spawn_blocking};

use crate::{
    async_dispatch::{dispatch_function_call, join_error_to_py, spawn_coroutine_task, wait_for_futures},
    build::extract_type_check_stubs,
    convert::{get_docstring, monty_to_py, py_to_monty_value},
    dataclass::DcRegistry,
    exceptions::{MontyCrashedError, MontyError, MontyTypingError, exc_py_to_monty},
    external::{CallResult, ExternalFunctionRegistry, dispatch_method_call},
    get_not_handled,
    limits::extract_limits,
    mount::PyMountDir,
    print_target::PrintTarget,
    repl::extract_repl_inputs,
};

/// The pool handle shared between `MontyPool` and its sessions. `None` until
/// `__aenter__` creates the pool and again after `__aexit__` shuts it down.
type SharedPool = Arc<Mutex<Option<Arc<Pool>>>>;
/// The worker handle of one session. `None` before `__aenter__`, after
/// `__aexit__`, and after the worker is discarded on a crash.
type SharedCheckout = Arc<Mutex<Option<Checkout>>>;

/// Async context manager owning a pool of `monty` subprocess workers.
#[pyclass(name = "MontyPool", module = "pydantic_monty", frozen)]
pub struct PyMontyPool {
    config: PoolConfig,
    pool: SharedPool,
}

#[pymethods]
impl PyMontyPool {
    /// Creates the pool configuration; workers are spawned by `async with`.
    #[new]
    #[pyo3(signature = (
        *,
        binary_path = None,
        min_processes = 1,
        max_processes = None,
        checkout_timeout = None,
        request_timeout = None,
        max_checkouts_per_worker = None,
    ))]
    fn new(
        py: Python<'_>,
        binary_path: Option<PathBuf>,
        min_processes: usize,
        max_processes: Option<usize>,
        checkout_timeout: Option<f64>,
        request_timeout: Option<f64>,
        max_checkouts_per_worker: Option<u32>,
    ) -> PyResult<Self> {
        let binary_path = match binary_path {
            Some(path) => path,
            // resolution lives in Python (env var, bundled binary, PATH)
            None => py
                .import("pydantic_monty._binary")?
                .call_method0("find_monty_binary")?
                .extract()?,
        };
        let mut config = PoolConfig::new(binary_path);
        config.min_processes = min_processes;
        if let Some(max) = max_processes {
            config.max_processes = max;
        }
        config.checkout_timeout = checkout_timeout.map(duration_from_secs).transpose()?;
        config.request_timeout = request_timeout.map(duration_from_secs).transpose()?;
        config.max_checkouts_per_worker = max_checkouts_per_worker;
        Ok(Self {
            config,
            pool: Arc::new(Mutex::new(None)),
        })
    }

    /// Spawns the pool's workers (off the event loop) and returns `self`.
    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        let config = slf.get().config.clone();
        let slot = Arc::clone(&slf.get().pool);
        future_into_py(py, async move {
            let pool = spawn_blocking(move || Pool::new(config))
                .await
                .map_err(join_error_to_py)?
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            *lock(&slot) = Some(Arc::new(pool));
            Ok(slf)
        })
    }

    /// Shuts the pool down: idle workers exit, capacity is gone. Sessions
    /// still checked out keep their workers until they exit.
    #[pyo3(signature = (*_args))]
    fn __aexit__<'py>(&self, py: Python<'py>, _args: &Bound<'_, PyTuple>) -> PyResult<Bound<'py, PyAny>> {
        let pool = lock(&self.pool).take();
        future_into_py(py, async move {
            spawn_blocking(move || drop(pool)).await.map_err(join_error_to_py)?;
            Ok(())
        })
    }

    /// Prepares a REPL session; the worker is checked out by `async with`.
    /// Arguments mirror the `MontyRepl` constructor.
    #[pyo3(signature = (
        *,
        script_name = "main.py",
        limits = None,
        type_check = false,
        type_check_stubs = None,
        dataclass_registry = None,
    ))]
    fn checkout(
        &self,
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
        type_check: bool,
        type_check_stubs: Option<&Bound<'_, PyString>>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyMontyPoolSession> {
        Ok(PyMontyPoolSession {
            pool: Arc::clone(&self.pool),
            repl_config: ReplConfig {
                script_name: script_name.to_owned(),
                limits: limits.map(extract_limits).transpose()?,
                type_check,
                type_check_stubs: extract_type_check_stubs(py, type_check_stubs)?,
            },
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            checkout: Arc::new(Mutex::new(None)),
        })
    }
}

/// One worker process dedicated to one REPL session; created by
/// [`PyMontyPool::checkout`] and driven with `feed_run` / `feed_run_async`.
#[pyclass(name = "MontyPoolSession", module = "pydantic_monty", frozen)]
pub struct PyMontyPoolSession {
    pool: SharedPool,
    repl_config: ReplConfig,
    dc_registry: DcRegistry,
    checkout: SharedCheckout,
}

#[pymethods]
impl PyMontyPoolSession {
    /// Checks a worker out of the pool (spawning one if needed) and creates
    /// the REPL session in it.
    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        let this = slf.get();
        let pool = Arc::clone(&this.pool);
        let repl_config = this.repl_config.clone();
        let slot = Arc::clone(&this.checkout);
        future_into_py(py, async move {
            let pool = lock(&pool)
                .as_ref()
                .map(Arc::clone)
                .ok_or_else(|| PyRuntimeError::new_err("MontyPool is not active — use `async with MontyPool(...)`"))?;
            let checkout = spawn_blocking(move || pool.checkout(&repl_config))
                .await
                .map_err(join_error_to_py)?
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            *lock(&slot) = Some(checkout);
            Ok(slf)
        })
    }

    /// Returns the worker to the pool (best effort — a crashed worker has
    /// already been discarded and replaced).
    #[pyo3(signature = (*_args))]
    fn __aexit__<'py>(&self, py: Python<'py>, _args: &Bound<'_, PyTuple>) -> PyResult<Bound<'py, PyAny>> {
        let checkout = lock(&self.checkout).take();
        future_into_py(py, async move {
            spawn_blocking(move || checkout.map(Checkout::finish))
                .await
                .map_err(join_error_to_py)?;
            Ok(())
        })
    }

    /// Executes one snippet in the worker, driving external function calls,
    /// OS callbacks, and print callbacks in this process. Blocks the calling
    /// thread (with the GIL released); use `feed_run_async` on an event loop.
    ///
    /// Async external functions are not supported here — use
    /// `feed_run_async`.
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    #[expect(clippy::too_many_arguments)]
    fn feed_run(
        &self,
        py: Python<'_>,
        code: &Bound<'_, PyString>,
        inputs: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Py<PyAny>> {
        let args = FeedArgs::extract(py, self, code, inputs, print_callback, mount, os, skip_type_check)?;
        drive_sync(py, args, external_functions)
    }

    /// Async variant of `feed_run`: pool I/O runs off the event loop and
    /// external functions may be coroutines (awaited concurrently, exactly
    /// like `MontyRepl.feed_run_async`).
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    #[expect(clippy::too_many_arguments)]
    fn feed_run_async<'py>(
        &self,
        py: Python<'py>,
        code: &Bound<'_, PyString>,
        inputs: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let args = FeedArgs::extract(py, self, code, inputs, print_callback, mount, os, skip_type_check)?;
        let ext_fns = external_functions.map(|d| d.clone().unbind());
        future_into_py(py, async move { drive_async(args, ext_fns).await })
    }

    /// Serializes the worker's session state (idle or suspended) into opaque
    /// bytes via monty's existing dump format. The session stays usable.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let checkout = Arc::clone(&self.checkout);
        let state = py
            .detach(|| {
                let mut guard = lock(&checkout);
                guard.as_mut().ok_or(PoolError::Finished).and_then(Checkout::dump)
            })
            .map_err(|e| pool_err_to_py(py, e))?;
        Ok(PyBytes::new(py, &state))
    }

    /// OS process id of this session's worker, or `None` when no worker is
    /// attached (diagnostics/tests).
    #[getter]
    fn worker_pid(&self) -> Option<u32> {
        lock(&self.checkout).as_ref().and_then(Checkout::pid)
    }
}

/// Everything a feed needs, extracted from Python arguments up front so the
/// sync and async drive loops share one validation path.
struct FeedArgs {
    code: String,
    inputs: Vec<(String, MontyObject)>,
    mounts: Vec<MountSpec>,
    skip_type_check: bool,
    os: Option<Py<PyAny>>,
    print_target: PrintTarget,
    checkout: SharedCheckout,
    dc_registry: DcRegistry,
}

impl FeedArgs {
    #[expect(clippy::too_many_arguments)]
    fn extract(
        py: Python<'_>,
        session: &PyMontyPoolSession,
        code: &Bound<'_, PyString>,
        inputs: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Self> {
        if let Some(ref os_cb) = os
            && !os_cb.bind(py).is_callable()
        {
            let t = os_cb.bind(py).get_type().name()?;
            return Err(PyTypeError::new_err(format!("TypeError: '{t}' object is not callable")));
        }
        Ok(Self {
            code: code.to_str()?.to_owned(),
            inputs: extract_repl_inputs(inputs, &session.dc_registry)?,
            mounts: extract_mount_specs(mount)?,
            skip_type_check,
            os,
            print_target: PrintTarget::from_py(print_callback)?,
            checkout: Arc::clone(&session.checkout),
            dc_registry: session.dc_registry.clone_ref(py),
        })
    }
}

// =============================================================================
// Drive loops
// =============================================================================

/// Synchronous drive loop: protocol turns run with the GIL released;
/// callbacks run between turns with the GIL held.
fn drive_sync(py: Python<'_>, args: FeedArgs, external_functions: Option<&Bound<'_, PyDict>>) -> PyResult<Py<PyAny>> {
    let FeedArgs {
        code,
        inputs,
        mounts,
        skip_type_check,
        os,
        print_target,
        checkout,
        dc_registry,
    } = args;
    let mut event = {
        let (result, print_err) = py.detach(|| {
            run_turn_blocking(&checkout, &print_target, |c, p| {
                c.feed(&code, inputs, mounts, skip_type_check, p)
            })
        });
        finalize_turn(py, result, print_err)?
    };

    loop {
        let resume_with: TurnAnswer = match event {
            TurnEvent::Complete(value) => return monty_to_py(py, &value, &dc_registry),
            TurnEvent::FunctionCall {
                function_name,
                args,
                kwargs,
                method_call,
                ..
            } => {
                let result = if method_call {
                    dispatch_method_call(py, &function_name, &args, &kwargs, &dc_registry)
                } else if let Some(fns) = external_functions {
                    ExternalFunctionRegistry::new(py, fns, &dc_registry).call(&function_name, &args, &kwargs)
                } else {
                    ExtFunctionResult::NotFound(function_name)
                };
                TurnAnswer::Call(ext_to_resume(result)?)
            }
            TurnEvent::OsCall {
                function_name,
                args,
                kwargs,
                not_handled_error,
                ..
            } => {
                let result = dispatch_os_parts(
                    py,
                    &function_name,
                    &args,
                    &kwargs,
                    not_handled_error.as_ref(),
                    os.as_ref(),
                    &dc_registry,
                );
                TurnAnswer::Call(ext_to_resume(result)?)
            }
            TurnEvent::NameLookup { name } => TurnAnswer::Name(resolve_pool_name_lookup(&name, external_functions)),
            TurnEvent::ResolveFutures { .. } => {
                return Err(PyRuntimeError::new_err(
                    "async external functions require feed_run_async",
                ));
            }
        };
        let (result, print_err) = py.detach(|| {
            run_turn_blocking(&checkout, &print_target, move |c, p| match resume_with {
                TurnAnswer::Call(value) => c.resume(value, p),
                TurnAnswer::Name(value) => c.resume_name_lookup(value, p),
            })
        });
        event = finalize_turn(py, result, print_err)?;
    }
}

/// Async drive loop: protocol turns run in `spawn_blocking`; coroutine
/// external functions are spawned as tasks and resolved via
/// `ResolveFutures`, mirroring `MontyRepl.feed_run_async`.
async fn drive_async(args: FeedArgs, external_functions: Option<Py<PyDict>>) -> PyResult<Py<PyAny>> {
    let FeedArgs {
        code,
        inputs,
        mounts,
        skip_type_check,
        os,
        print_target,
        checkout,
        dc_registry,
    } = args;
    let mut join_set: JoinSet<(u32, ExtFunctionResult)> = JoinSet::new();

    let mut event = run_turn_async(&checkout, &print_target, move |c, p| {
        c.feed(&code, inputs, mounts, skip_type_check, p)
    })
    .await?;

    loop {
        let answer: TurnAnswer = match event {
            TurnEvent::Complete(value) => {
                return Python::attach(|py| monty_to_py(py, &value, &dc_registry));
            }
            TurnEvent::FunctionCall {
                function_name,
                args,
                kwargs,
                call_id,
                method_call,
            } => {
                match dispatch_function_call(
                    &function_name,
                    method_call,
                    &args,
                    &kwargs,
                    external_functions.as_ref(),
                    &dc_registry,
                ) {
                    CallResult::Sync(result) => TurnAnswer::Call(ext_to_resume(result)?),
                    CallResult::Coroutine(coro) => {
                        spawn_coroutine_task(&mut join_set, call_id, coro, &dc_registry)?;
                        TurnAnswer::Call(ResumeValue::Future)
                    }
                }
            }
            TurnEvent::OsCall {
                function_name,
                args,
                kwargs,
                not_handled_error,
                ..
            } => {
                let result = Python::attach(|py| {
                    dispatch_os_parts(
                        py,
                        &function_name,
                        &args,
                        &kwargs,
                        not_handled_error.as_ref(),
                        os.as_ref(),
                        &dc_registry,
                    )
                });
                TurnAnswer::Call(ext_to_resume(result)?)
            }
            TurnEvent::NameLookup { name } => TurnAnswer::Name(Python::attach(|py| {
                resolve_pool_name_lookup(&name, external_functions.as_ref().map(|d| d.bind(py)))
            })),
            TurnEvent::ResolveFutures { pending_call_ids } => {
                let results = wait_for_futures(&mut join_set, &pending_call_ids).await?;
                let results = results
                    .into_iter()
                    .map(|(call_id, result)| Ok((call_id, ext_to_resume(result)?)))
                    .collect::<PyResult<Vec<_>>>()?;
                event = run_turn_async(&checkout, &print_target, move |c, p| c.resume_futures(results, p)).await?;
                continue;
            }
        };
        event = run_turn_async(&checkout, &print_target, move |c, p| match answer {
            TurnAnswer::Call(value) => c.resume(value, p),
            TurnAnswer::Name(value) => c.resume_name_lookup(value, p),
        })
        .await?;
    }
}

/// The caller's answer to a suspension, paired with which resume call
/// delivers it.
enum TurnAnswer {
    Call(ResumeValue),
    Name(Option<MontyObject>),
}

/// Runs one protocol turn against the (locked) checkout, streaming prints to
/// `print_target` and capturing the first print-callback failure.
fn run_turn_blocking(
    checkout: &SharedCheckout,
    print_target: &PrintTarget,
    turn: impl FnOnce(&mut Checkout, monty_pool::OnPrint<'_>) -> Result<TurnEvent, PoolError>,
) -> (Result<TurnEvent, PoolError>, Option<MontyException>) {
    let mut guard = lock(checkout);
    let Some(checkout) = guard.as_mut() else {
        return (Err(PoolError::Finished), None);
    };
    let mut print_err: Option<MontyException> = None;
    let result = {
        let mut on_print = |stream, text: &str| {
            if print_err.is_none()
                && let Err(err) = print_target.write_event(stream, text)
            {
                print_err = Some(err);
            }
        };
        turn(checkout, &mut on_print)
    };
    (result, print_err)
}

/// `spawn_blocking` wrapper around [`run_turn_blocking`] for the async loop.
async fn run_turn_async(
    checkout: &SharedCheckout,
    print_target: &PrintTarget,
    turn: impl FnOnce(&mut Checkout, monty_pool::OnPrint<'_>) -> Result<TurnEvent, PoolError> + Send + 'static,
) -> PyResult<TurnEvent> {
    let checkout = Arc::clone(checkout);
    let print_target = print_target.clone_handle_detached();
    let (result, print_err) = spawn_blocking(move || run_turn_blocking(&checkout, &print_target, turn))
        .await
        .map_err(join_error_to_py)?;
    Python::attach(|py| finalize_turn(py, result, print_err))
}

/// Converts a turn outcome into the next event, surfacing print-callback
/// failures (which take precedence — they are host-side errors).
fn finalize_turn(
    py: Python<'_>,
    result: Result<TurnEvent, PoolError>,
    print_err: Option<MontyException>,
) -> PyResult<TurnEvent> {
    if let Some(err) = print_err {
        return Err(MontyError::new_err(py, err));
    }
    result.map_err(|e| pool_err_to_py(py, e))
}

// =============================================================================
// Dispatch helpers
// =============================================================================

/// Maps an `ExtFunctionResult` from callback dispatch onto the pool's resume
/// payload.
fn ext_to_resume(result: ExtFunctionResult) -> PyResult<ResumeValue> {
    match result {
        ExtFunctionResult::Return(value) => Ok(ResumeValue::Return(value)),
        ExtFunctionResult::Error(exc) => Ok(ResumeValue::Error(exc)),
        ExtFunctionResult::NotFound(_) => Ok(ResumeValue::NotFound),
        // futures are handled explicitly by the async loop before this point
        ExtFunctionResult::Future(_) => Err(PyRuntimeError::new_err("unexpected future result")),
    }
}

/// Calls the Python `os=` fallback for a bubbled OS call. With no callback —
/// or when it returns `NOT_HANDLED` — answers with the child-provided
/// `not_handled_error`, preserving monty's per-call no-handler semantics.
fn dispatch_os_parts(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    not_handled_error: Option<&MontyException>,
    os: Option<&Py<PyAny>>,
    dc_registry: &DcRegistry,
) -> ExtFunctionResult {
    let on_no_handler = || {
        not_handled_error.cloned().unwrap_or_else(|| {
            MontyException::new(
                ExcType::RuntimeError,
                Some(format!("'{function_name}' is not supported in this environment")),
            )
        })
    };
    let Some(os_callback) = os else {
        return on_no_handler().into();
    };
    let call = || -> PyResult<ExtFunctionResult> {
        let py_args: Vec<Py<PyAny>> = args
            .iter()
            .map(|arg| monty_to_py(py, arg, dc_registry))
            .collect::<PyResult<_>>()?;
        let py_args = PyTuple::new(py, py_args)?;
        let py_kwargs = PyDict::new(py);
        for (k, v) in kwargs {
            py_kwargs.set_item(monty_to_py(py, k, dc_registry)?, monty_to_py(py, v, dc_registry)?)?;
        }
        let result = os_callback.bind(py).call1((function_name, py_args, py_kwargs))?;
        if result.is(get_not_handled(py)?.bind(py)) {
            return Ok(on_no_handler().into());
        }
        Ok(match py_to_monty_value(&result, dc_registry) {
            Ok(obj) => ExtFunctionResult::Return(obj),
            Err(exc) => ExtFunctionResult::Error(exc),
        })
    };
    call().unwrap_or_else(|err| ExtFunctionResult::Error(exc_py_to_monty(py, &err)))
}

/// Resolves a bare-name lookup against the external functions dict.
fn resolve_pool_name_lookup(name: &str, external_functions: Option<&Bound<'_, PyDict>>) -> Option<MontyObject> {
    let value = external_functions?.get_item(name).ok().flatten()?;
    Some(MontyObject::Function {
        name: name.to_owned(),
        docstring: get_docstring(&value),
    })
}

/// Extracts `MountDir | list[MountDir] | None` into child-local mount specs.
/// Only the mount *configuration* crosses the process boundary — overlay
/// writes live in the worker and are discarded when the feed ends.
fn extract_mount_specs(mount: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<MountSpec>> {
    let Some(mount) = mount else {
        return Ok(vec![]);
    };
    if let Ok(single) = mount.extract::<PyRef<'_, PyMountDir>>() {
        return Ok(vec![mount_spec(&single)?]);
    }
    if let Ok(list) = mount.cast::<PyList>() {
        return list
            .iter()
            .map(|item| {
                let dir = item.extract::<PyRef<'_, PyMountDir>>()?;
                mount_spec(&dir)
            })
            .collect();
    }
    Err(PyTypeError::new_err(
        "mount must be a MountDir, a list of MountDir, or None",
    ))
}

fn mount_spec(dir: &PyRef<'_, PyMountDir>) -> PyResult<MountSpec> {
    let (virtual_path, host_path, mode, write_bytes_limit) = dir.spec_parts()?;
    let mode = match mode {
        "read-only" => MountSpecMode::ReadOnly,
        "read-write" => MountSpecMode::ReadWrite,
        "overlay" => MountSpecMode::Overlay,
        other => return Err(PyValueError::new_err(format!("unknown mount mode {other:?}"))),
    };
    Ok(MountSpec {
        virtual_path,
        host_path,
        mode,
        write_bytes_limit,
    })
}

/// Maps a pool failure onto the Python exception hierarchy.
fn pool_err_to_py(py: Python<'_>, err: PoolError) -> PyErr {
    let message = err.to_string();
    match err {
        PoolError::Runtime(exc) => MontyError::new_err(py, exc),
        PoolError::Typing(diagnostics) => MontyTypingError::new_err_rendered(py, diagnostics),
        PoolError::Crashed { status, .. } => {
            MontyCrashedError::new_err(py, message, false, status.and_then(|s| s.code()))
        }
        PoolError::Timeout { .. } => MontyCrashedError::new_err(py, message, true, None),
        PoolError::Exhausted => PyTimeoutError::new_err(message),
        PoolError::Protocol(_) | PoolError::Spawn(_) | PoolError::Finished => PyRuntimeError::new_err(message),
    }
}

fn duration_from_secs(secs: f64) -> PyResult<Duration> {
    Duration::try_from_secs_f64(secs).map_err(|err| PyValueError::new_err(format!("invalid timeout: {err}")))
}

/// Locks a shared slot, ignoring poisoning (a panic elsewhere must not wedge
/// the pool).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
