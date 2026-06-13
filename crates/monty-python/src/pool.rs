//! `Monty` and `AsyncMonty` — crash-isolated execution in pools of `monty`
//! subprocess workers.
//!
//! A monty process can never be made fully crash-proof against memory errors
//! (stack overflow, allocator aborts), so this package *only* runs the
//! interpreter in worker subprocesses via the `monty-pool` crate: a crashed
//! worker raises [`MontyCrashedError`] and is replaced, and the host Python
//! process is never at risk.
//!
//! ```python
//! with Monty() as pool:
//!     with pool.checkout() as session:
//!         result = session.feed_run('1 + 1')
//!
//! async with AsyncMonty() as pool:
//!     async with pool.checkout() as session:
//!         result = await session.feed_run('1 + 1')
//! ```
//!
//! Both classes share all pool/dispatch machinery; they differ only in how
//! the blocking protocol turns are driven. `Monty` blocks the calling thread
//! with the GIL released; `AsyncMonty` hands turns to tokio's blocking pool
//! via `spawn_blocking` so the event loop stays free, and its external
//! functions may be coroutines. Python callbacks — external functions, `os=`,
//! `print_callback` — always execute in the host process.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError},
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
    build::{extract_repl_inputs, extract_source_code, extract_type_check_stubs},
    convert::{get_docstring, monty_to_py, py_to_monty_value},
    dataclass::DcRegistry,
    exceptions::{MontyCrashedError, MontyError, MontyTypingError, exc_py_to_monty},
    external::{CallResult, ExternalFunctionRegistry, dispatch_method_call},
    get_not_handled,
    limits::extract_limits,
    mount::PyMountDir,
    print_target::PrintTarget,
};

/// The pool handle shared between a pool object and its sessions. `None`
/// until the context manager is entered and again after it exits.
type SharedPool = Arc<Mutex<Option<Arc<Pool>>>>;
/// The worker handle of one session. `None` before the session is entered,
/// after it exits, and after the worker is discarded on a crash.
type SharedCheckout = Arc<Mutex<Option<Checkout>>>;

// =============================================================================
// Sync API: Monty / MontySession
// =============================================================================

/// Sync context manager owning a pool of `monty` subprocess workers.
#[pyclass(name = "Monty", module = "pydantic_monty", frozen)]
pub struct PyMonty {
    config: PoolConfig,
    pool: SharedPool,
}

#[pymethods]
impl PyMonty {
    /// Creates the pool configuration; workers are spawned by `with`.
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
        Ok(Self {
            config: parse_pool_config(
                py,
                binary_path,
                min_processes,
                max_processes,
                checkout_timeout,
                request_timeout,
                max_checkouts_per_worker,
            )?,
            pool: Arc::new(Mutex::new(None)),
        })
    }

    /// Spawns the pool's workers (with the GIL released) and returns `self`.
    fn __enter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        let this = slf.get();
        let config = this.config.clone();
        let pool = py.detach(|| Pool::new(config)).map_err(|e| pool_err_to_py(py, e))?;
        *lock(&this.pool) = Some(Arc::new(pool));
        Ok(slf)
    }

    /// Shuts the pool down: idle workers exit, capacity is gone. Sessions
    /// still checked out keep their workers until they exit.
    #[pyo3(signature = (*_args))]
    fn __exit__(&self, py: Python<'_>, _args: &Bound<'_, PyTuple>) {
        let pool = lock(&self.pool).take();
        py.detach(|| drop(pool));
    }

    /// Prepares a REPL session; the worker is checked out by `with`.
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
    ) -> PyResult<PyMontySession> {
        Ok(PyMontySession {
            pool: Arc::clone(&self.pool),
            repl_config: parse_repl_config(py, script_name, limits, type_check, type_check_stubs)?,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            checkout: Arc::new(Mutex::new(None)),
        })
    }
}

/// One worker process dedicated to one REPL session; created by
/// [`PyMonty::checkout`] and driven with `feed_run`.
#[pyclass(name = "MontySession", module = "pydantic_monty", frozen)]
pub struct PyMontySession {
    pool: SharedPool,
    repl_config: ReplConfig,
    dc_registry: DcRegistry,
    checkout: SharedCheckout,
}

#[pymethods]
impl PyMontySession {
    /// Checks a worker out of the pool (spawning one if needed) and creates
    /// the REPL session in it.
    ///
    /// The checkout slot is locked with the GIL released: a turn in flight on
    /// another thread holds that lock and may block on the GIL for print
    /// callbacks, so locking it while attached can deadlock.
    fn __enter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        let this = slf.get();
        let pool = active_pool(&this.pool)?;
        let repl_config = this.repl_config.clone();
        let slot = Arc::clone(&this.checkout);
        py.detach(|| {
            pool.checkout(&repl_config)
                .map(|checkout| *lock(&slot) = Some(checkout))
        })
        .map_err(|e| pool_err_to_py(py, e))?;
        Ok(slf)
    }

    /// Returns the worker to the pool (best effort — a crashed worker has
    /// already been discarded and replaced). The slot is taken with the GIL
    /// released, like [`__enter__`](Self::__enter__).
    #[pyo3(signature = (*_args))]
    fn __exit__(&self, py: Python<'_>, _args: &Bound<'_, PyTuple>) {
        let slot = Arc::clone(&self.checkout);
        py.detach(move || {
            let checkout = lock(&slot).take();
            if let Some(checkout) = checkout {
                let _ = checkout.finish();
            }
        });
    }

    /// Executes one snippet in the worker, driving external function calls,
    /// OS callbacks, and print callbacks in this process. Session state
    /// (globals, functions) persists across feeds.
    ///
    /// Blocks the calling thread with the GIL released; async external
    /// functions are not supported here — use [`AsyncMonty`].
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
        let args = FeedArgs::extract(
            py,
            &self.checkout,
            &self.dc_registry,
            code,
            inputs,
            print_callback,
            mount,
            os,
            skip_type_check,
        )?;
        drive_sync(py, args, external_functions)
    }

    /// Serializes the worker's session state (idle or suspended) into opaque
    /// bytes via monty's existing dump format. The session stays usable.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let state = py
            .detach(|| dump_checkout(&self.checkout))
            .map_err(|e| pool_err_to_py(py, e))?;
        Ok(PyBytes::new(py, &state))
    }

    /// OS process id of this session's worker, or `None` when no worker is
    /// attached or a turn is in flight (diagnostics/tests).
    ///
    /// Must not block on the checkout lock: this getter runs with the GIL
    /// held, and the thread driving a turn holds the lock while needing the
    /// GIL for print callbacks — blocking here can deadlock both threads.
    #[getter]
    fn worker_pid(&self) -> Option<u32> {
        try_lock(&self.checkout)?.as_ref().and_then(Checkout::pid)
    }
}

// =============================================================================
// Async API: AsyncMonty / AsyncMontySession
// =============================================================================

/// Async context manager owning a pool of `monty` subprocess workers.
#[pyclass(name = "AsyncMonty", module = "pydantic_monty", frozen)]
pub struct PyAsyncMonty {
    config: PoolConfig,
    pool: SharedPool,
}

#[pymethods]
impl PyAsyncMonty {
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
        Ok(Self {
            config: parse_pool_config(
                py,
                binary_path,
                min_processes,
                max_processes,
                checkout_timeout,
                request_timeout,
                max_checkouts_per_worker,
            )?,
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
    ) -> PyResult<PyAsyncMontySession> {
        Ok(PyAsyncMontySession {
            pool: Arc::clone(&self.pool),
            repl_config: parse_repl_config(py, script_name, limits, type_check, type_check_stubs)?,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            checkout: Arc::new(Mutex::new(None)),
        })
    }
}

/// One worker process dedicated to one REPL session; created by
/// [`PyAsyncMonty::checkout`] and driven with the async `feed_run`.
#[pyclass(name = "AsyncMontySession", module = "pydantic_monty", frozen)]
pub struct PyAsyncMontySession {
    pool: SharedPool,
    repl_config: ReplConfig,
    dc_registry: DcRegistry,
    checkout: SharedCheckout,
}

#[pymethods]
impl PyAsyncMontySession {
    /// Checks a worker out of the pool (spawning one if needed) and creates
    /// the REPL session in it.
    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        let this = slf.get();
        let pool = Arc::clone(&this.pool);
        let repl_config = this.repl_config.clone();
        let slot = Arc::clone(&this.checkout);
        future_into_py(py, async move {
            let pool = active_pool(&pool)?;
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
    ///
    /// The checkout slot is taken inside `spawn_blocking`, never on the event
    /// loop with the GIL held: a cancelled `feed_run` leaves its blocking
    /// turn running with the lock until the worker answers (or the request
    /// timeout fires), and that turn may itself block on the GIL for print
    /// callbacks — taking the lock here synchronously would deadlock.
    #[pyo3(signature = (*_args))]
    fn __aexit__<'py>(&self, py: Python<'py>, _args: &Bound<'_, PyTuple>) -> PyResult<Bound<'py, PyAny>> {
        let slot = Arc::clone(&self.checkout);
        future_into_py(py, async move {
            spawn_blocking(move || {
                // take in its own statement so the lock is released before
                // the (blocking) finish turn runs
                let checkout = lock(&slot).take();
                checkout.map(Checkout::finish)
            })
            .await
            .map_err(join_error_to_py)?;
            Ok(())
        })
    }

    /// Executes one snippet in the worker, driving external function calls
    /// (which may be coroutines, awaited concurrently), OS callbacks, and
    /// print callbacks in this process. Session state persists across feeds.
    ///
    /// Worker I/O runs off the event loop via tokio's blocking pool.
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    #[expect(clippy::too_many_arguments)]
    fn feed_run<'py>(
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
        let args = FeedArgs::extract(
            py,
            &self.checkout,
            &self.dc_registry,
            code,
            inputs,
            print_callback,
            mount,
            os,
            skip_type_check,
        )?;
        let ext_fns = external_functions.map(|d| d.clone().unbind());
        future_into_py(py, async move { drive_async(args, ext_fns).await })
    }

    /// Serializes the worker's session state (idle or suspended) into opaque
    /// bytes via monty's existing dump format. The session stays usable.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let checkout = Arc::clone(&self.checkout);
        future_into_py(py, async move {
            let state = spawn_blocking(move || dump_checkout(&checkout))
                .await
                .map_err(join_error_to_py)?
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            Ok(Python::attach(|py| PyBytes::new(py, &state).unbind()))
        })
    }

    /// OS process id of this session's worker, or `None` when no worker is
    /// attached or a turn is in flight (diagnostics/tests). Non-blocking for
    /// the same reason as the sync getter.
    #[getter]
    fn worker_pid(&self) -> Option<u32> {
        try_lock(&self.checkout)?.as_ref().and_then(Checkout::pid)
    }
}

// =============================================================================
// Shared argument parsing
// =============================================================================

/// Builds the `monty-pool` config from the (shared) pool constructor
/// arguments, resolving the binary via `pydantic_monty._binary` when not
/// given explicitly.
fn parse_pool_config(
    py: Python<'_>,
    binary_path: Option<PathBuf>,
    min_processes: usize,
    max_processes: Option<usize>,
    checkout_timeout: Option<f64>,
    request_timeout: Option<f64>,
    max_checkouts_per_worker: Option<u32>,
) -> PyResult<PoolConfig> {
    let binary_path = match binary_path {
        Some(path) => path,
        // resolution lives in Python (env var, installed cli wheel, PATH)
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
    Ok(config)
}

/// Builds the worker-side REPL session config from the (shared) `checkout`
/// arguments.
fn parse_repl_config(
    py: Python<'_>,
    script_name: &str,
    limits: Option<&Bound<'_, PyDict>>,
    type_check: bool,
    type_check_stubs: Option<&Bound<'_, PyString>>,
) -> PyResult<ReplConfig> {
    Ok(ReplConfig {
        script_name: script_name.to_owned(),
        limits: limits.map(extract_limits).transpose()?,
        type_check,
        type_check_stubs: extract_type_check_stubs(py, type_check_stubs)?,
    })
}

/// Clones the live pool handle out of a shared slot, erroring when the
/// context manager has not been entered (or already exited).
fn active_pool(pool: &SharedPool) -> PyResult<Arc<Pool>> {
    lock(pool).as_ref().map(Arc::clone).ok_or_else(|| {
        PyRuntimeError::new_err("the pool is not active — enter the Monty / AsyncMonty context manager first")
    })
}

/// Dumps the session of a live checkout (shared by the sync and async dump
/// methods; runs without the GIL).
fn dump_checkout(checkout: &SharedCheckout) -> Result<Vec<u8>, PoolError> {
    let mut guard = lock(checkout);
    guard.as_mut().ok_or(PoolError::Finished).and_then(Checkout::dump)
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
        checkout: &SharedCheckout,
        dc_registry: &DcRegistry,
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
            return Err(PyTypeError::new_err(format!("'{t}' object is not callable")));
        }
        Ok(Self {
            code: extract_source_code(py, code)?,
            inputs: extract_repl_inputs(inputs, dc_registry)?,
            mounts: extract_mount_specs(mount)?,
            skip_type_check,
            os,
            print_target: PrintTarget::from_py(print_callback)?,
            checkout: Arc::clone(checkout),
            dc_registry: dc_registry.clone_ref(py),
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
                return Err(PyRuntimeError::new_err("async external functions require AsyncMonty"));
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
/// `ResolveFutures`.
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
    // A print-callback failure aborts the feed. If the turn left the worker
    // suspended awaiting a resume the aborted feed will never send, drop the
    // checkout so the session ends cleanly rather than wedging the next feed
    // with a dangling suspension.
    if print_err.is_some()
        && matches!(
            result,
            Ok(TurnEvent::FunctionCall { .. }
                | TurnEvent::OsCall { .. }
                | TurnEvent::NameLookup { .. }
                | TurnEvent::ResolveFutures { .. })
        )
    {
        *guard = None;
    }
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
        PoolError::Typing(diagnostics) => MontyTypingError::new_err(py, diagnostics),
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
/// the pool). Never call while attached to the GIL: a protocol turn holds the
/// checkout lock for its whole duration and attaches for print callbacks, so
/// a GIL-holding waiter deadlocks both threads — detach first, or use
/// [`try_lock`].
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Non-blocking [`lock`]: `None` when the lock is held (e.g. by a turn in
/// flight on another thread). Safe to call with the GIL held.
fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(err)) => Some(err.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}
