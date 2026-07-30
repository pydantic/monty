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
//! Both classes share all pool/dispatch machinery — `monty-pool` is async, so
//! every protocol turn is a future driven on the pyo3-async-runtimes tokio
//! runtime. `AsyncMonty` awaits those futures from the asyncio event loop via
//! `future_into_py`; `Monty` blocks the calling thread on the same futures
//! with `Runtime::block_on` (GIL released). External functions may be
//! coroutines under `AsyncMonty`. Python callbacks — external functions,
//! `os=`, `print_callback` — always execute in the host process.

use std::{
    future::{Future, ready},
    num::NonZeroU32,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use monty_pool::{
    Checkout, MountSpec, OnPrint, Pool, PoolConfig, PoolError, PrintFuture, ReplConfig, ResumeValue, TurnEvent,
};
use monty_proto::python::{DcRegistry, exc_py_to_monty, monty_to_py, py_to_monty_value};
use monty_types::{AssertMessageAnnotations, ExtFunctionResult, MontyException, MontyObject, PrintStream};
use pyo3::{
    Borrowed,
    exceptions::{PyRuntimeError, PyTimeoutError, PyTypeError, PyValueError},
    prelude::*,
    types::{PyBool, PyBytes, PyDict, PyInt, PyList, PyString, PyTuple},
};
use pyo3_async_runtimes::tokio::{future_into_py, get_runtime};
use tokio::{sync::Mutex as AsyncMutex, task::JoinSet};

use crate::{
    async_dispatch::{dispatch_function_call, spawn_coroutine_task, wait_for_futures},
    build::{extract_repl_inputs, extract_source_code, extract_type_check_stubs},
    exceptions::{MontyCrashedError, MontyDisconnectError, MontyError, MontyShutdown, MontyTypingError},
    external::{CallResult, ExternalLookup, dispatch_method_call},
    get_not_handled,
    limits::extract_limits,
    mount::PyMountDir,
    print_target::PrintTarget,
    snapshot::{DriveContext, build_snapshot, feed_start_async, feed_start_sync},
};

/// The pool handle shared between a pool object and its sessions. `None`
/// until the context manager is entered and again after it exits. A std
/// mutex: it is only ever held to clone/take the `Arc`, never across awaits.
pub(crate) type SharedPool = Arc<Mutex<Option<Arc<Pool>>>>;
/// The worker handle of one session. `None` before the session is entered,
/// after it exits, and after the worker is discarded on a crash. A *tokio*
/// mutex: it is held for the whole duration of a protocol turn, across all
/// of that turn's awaits, which serializes turns on the session.
pub(crate) type SharedCheckout = Arc<AsyncMutex<Option<Checkout>>>;

/// The boxed turn future a [`turn_fn`] closure returns: one protocol turn
/// borrowing the locked checkout and the per-turn print callback.
pub(crate) type TurnFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, PoolError>> + Send + 'a>>;

/// Identity helper pinning the HRTB closure shape [`run_turn`] accepts —
/// without it, closure lifetime inference fails at every callsite.
pub(crate) fn turn_fn<T, F>(f: F) -> F
where
    F: for<'a> FnOnce(&'a mut Checkout, OnPrint<'a>) -> TurnFuture<'a, T> + Send,
{
    f
}

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
        let pool = py
            .detach(|| get_runtime().block_on(Pool::new(config)))
            .map_err(|e| pool_err_to_py(py, e))?;
        *lock(&this.pool) = Some(Arc::new(pool));
        Ok(slf)
    }

    /// Shuts the pool down: idle workers exit, capacity is gone. Sessions
    /// still checked out keep their workers until they exit.
    #[pyo3(signature = (*_args))]
    fn __exit__(&self, py: Python<'_>, _args: &Bound<'_, PyTuple>) {
        let pool = lock(&self.pool).take();
        py.detach(|| get_runtime().block_on(close_pool(pool)));
    }

    /// Prepares a REPL session; the worker is checked out by `with`.
    #[pyo3(signature = (
        *,
        script_name = "main.py",
        limits = None,
        type_check = false,
        type_check_stubs = None,
        assert_message_annotations = AssertAnnotationsArg::default(),
        dataclass_registry = None,
    ))]
    #[expect(clippy::too_many_arguments)]
    fn checkout(
        &self,
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
        type_check: bool,
        type_check_stubs: Option<&Bound<'_, PyString>>,
        assert_message_annotations: AssertAnnotationsArg,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyMontySession> {
        Ok(PyMontySession {
            pool: Arc::clone(&self.pool),
            repl_config: parse_repl_config(
                py,
                script_name,
                limits,
                type_check,
                type_check_stubs,
                assert_message_annotations,
            )?,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            checkout: Arc::new(AsyncMutex::new(None)),
            used: AtomicBool::new(false),
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
    /// Set once the session has been fed or restored. `load_session` /
    /// `load_snapshot` are valid only while this is unset (a fresh, undriven
    /// session).
    used: AtomicBool,
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
            get_runtime().block_on(async {
                let checkout = pool.checkout(&repl_config).await?;
                *slot.lock().await = Some(checkout);
                Ok(())
            })
        })
        .map_err(|e: PoolError| pool_err_to_py(py, e))?;
        Ok(slf)
    }

    /// Returns the worker to the pool (best effort — a crashed worker has
    /// already been discarded and replaced). The slot is taken with the GIL
    /// released, like [`__enter__`](Self::__enter__).
    #[pyo3(signature = (*_args))]
    fn __exit__(&self, py: Python<'_>, _args: &Bound<'_, PyTuple>) {
        let slot = Arc::clone(&self.checkout);
        py.detach(move || get_runtime().block_on(finish_checkout(&slot)));
    }

    /// Executes one snippet in the worker, driving external function calls,
    /// OS callbacks, and print callbacks in this process. Session state
    /// (globals, functions) persists across feeds.
    ///
    /// Blocks the calling thread with the GIL released; async external
    /// functions are not supported here — use [`AsyncMonty`].
    #[pyo3(signature = (code, *, inputs=None, external_lookup=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    #[expect(clippy::too_many_arguments)]
    fn feed_run(
        &self,
        py: Python<'_>,
        code: &Bound<'_, PyString>,
        inputs: Option<&Bound<'_, PyDict>>,
        external_lookup: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Py<PyAny>> {
        self.used.store(true, Ordering::Relaxed);
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
        drive_sync(py, args, external_lookup)
    }

    /// Starts a snippet but, instead of driving it to completion, returns a
    /// snapshot at each external call, OS call, name lookup, or future
    /// resolution. The caller answers with `snapshot.resume(...)` and may
    /// `snapshot.dump()` to checkpoint the worker mid-execution.
    ///
    /// Unlike [`feed_run`](Self::feed_run), *every* suspension is surfaced as a
    /// snapshot rather than answered — external calls, name lookups and OS
    /// calls alike, so even a mount-covered file read comes back as a snapshot.
    /// That is the point of `feed_start`. An `external_lookup` / `os=` may
    /// still be supplied: neither is consulted during this drive, but both are
    /// captured on the snapshot so `snapshot.resume_auto()` can answer
    /// subsequent suspensions from them (and from this feed's mounts), letting
    /// a caller iterate to completion without resolving each call by hand.
    #[pyo3(signature = (code, *, inputs=None, external_lookup=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    #[expect(clippy::too_many_arguments)]
    fn feed_start(
        &self,
        py: Python<'_>,
        code: &Bound<'_, PyString>,
        inputs: Option<&Bound<'_, PyDict>>,
        external_lookup: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Py<PyAny>> {
        self.used.store(true, Ordering::Relaxed);
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
        let ext = external_lookup.map(|d| d.clone().unbind());
        feed_start_sync(py, args, ext, self.repl_config.script_name.clone())
    }

    /// Restores a dumped **idle** session — bytes from `session.dump()` taken
    /// between feeds — so you can keep feeding it. Use
    /// [`load_snapshot`](Self::load_snapshot) for a dump taken mid-execution.
    ///
    /// Valid only on a fresh session, before any feed or load; raises
    /// `RuntimeError` otherwise. The dump restores its own `script_name` /
    /// limits / type-check state (the `checkout()` config for those is not
    /// applied); the dataclass registry from `checkout()` is reused. Raises if
    /// the dump is actually a suspended snapshot.
    fn load_session(&self, py: Python<'_>, state: Vec<u8>) -> PyResult<()> {
        // an idle session has no snapshot, so the restored script name is unused
        if self.restore_turn(py, state, Vec::new())?.0.is_some() {
            py.detach(|| get_runtime().block_on(discard_checkout(&self.checkout)));
            return Err(PyRuntimeError::new_err(
                "this dump is a suspended snapshot — use load_snapshot() to resume it",
            ));
        }
        Ok(())
    }

    /// Restores a dumped **suspended** snapshot — bytes from `feed_start` +
    /// `snapshot.dump()` — and returns the re-announced snapshot to resume. Use
    /// [`load_session`](Self::load_session) for a dump taken between feeds.
    ///
    /// Valid only on a fresh session, before any feed or load; raises
    /// `RuntimeError` otherwise. `mount` re-establishes the suspended feed's
    /// mounts, which are never part of the dump — pass the same mounts the
    /// original feed used, or its filesystem calls degrade into unhandled OS
    /// calls. The dump restores its own config; the dataclass registry from
    /// `checkout()` is reused. Raises if the dump is actually an idle session.
    ///
    /// `external_lookup` / `os` are captured on the restored snapshot so it
    /// supports `resume_auto()`, just like `feed_start`. One caveat applies to a
    /// restored snapshot: a restored `FutureSnapshot`'s pending coroutines are
    /// gone (they lived in the previous process), so async `resume_auto()` on it
    /// raises — resolve it manually with `resume({call_id: ...})`.
    #[pyo3(signature = (state, *, mount=None, print_callback=None, external_lookup=None, os=None))]
    fn load_snapshot(
        &self,
        py: Python<'_>,
        state: Vec<u8>,
        mount: Option<&Bound<'_, PyAny>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        external_lookup: Option<&Bound<'_, PyDict>>,
        os: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        // extract args before committing the session, so a bad-args error
        // leaves it loadable (a failed load is not retryable — checkout a fresh
        // session — matching the async path)
        check_os_callable(py, os.as_ref())?;
        let mounts = extract_mount_specs(mount)?;
        let print_target = PrintTarget::from_py(print_callback)?;
        let ext = external_lookup.map(|d| d.clone().unbind());
        let (event, script_name) = self.restore_turn(py, state, mounts)?;
        let Some(event) = event else {
            py.detach(|| get_runtime().block_on(discard_checkout(&self.checkout)));
            return Err(PyRuntimeError::new_err(
                "this dump is an idle session — use load_session() to restore it",
            ));
        };
        let ctx = DriveContext::new(
            Arc::clone(&self.checkout),
            self.dc_registry.clone_ref(py),
            print_target,
            // the dump's own script name, falling back to the session config
            // only if the worker did not report one (e.g. an older child)
            script_name.unwrap_or_else(|| self.repl_config.script_name.clone()),
            ext,
            os,
        );
        build_snapshot(py, ctx, event, false)
    }

    /// Serializes the worker's session state (idle or suspended) into opaque
    /// bytes via monty's existing dump format. The session stays usable.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let state = py
            .detach(|| get_runtime().block_on(dump_checkout(&self.checkout)))
            .map_err(|e| pool_err_to_py(py, e))?;
        Ok(PyBytes::new(py, &state))
    }

    /// Installs third-party Python packages into the session via the worker's
    /// `uv`, making them importable by subsequent `feed_run` calls.
    /// Session-scoped and repeatable; an empty list is a no-op.
    ///
    /// Only the embedded-CPython worker supports this. Against the `monty`
    /// sandbox worker, or on a uv install failure (carrying uv's stderr), this
    /// raises `MontyRuntimeError`; the session stays usable. Blocks the calling
    /// thread with the GIL released, bounded by the pool's `request_timeout`.
    fn install_dependencies(&self, py: Python<'_>, requirements: Vec<String>) -> PyResult<()> {
        self.used.store(true, Ordering::Relaxed);
        py.detach(|| get_runtime().block_on(install_deps_checkout(&self.checkout, requirements)))
            .map_err(|e| pool_err_to_py(py, e))
    }

    /// OS process id of this session's worker, or `None` when no worker is
    /// attached or a turn is in flight (diagnostics/tests).
    ///
    /// Must not block on the checkout lock: this getter runs with the GIL
    /// held, and the task driving a turn holds the lock while needing the
    /// GIL for print callbacks — blocking here can deadlock.
    #[getter]
    fn worker_pid(&self) -> Option<u32> {
        self.checkout.try_lock().ok()?.as_ref().and_then(Checkout::pid)
    }
}

impl PyMontySession {
    /// Claims the fresh session (rejecting a reused one) and runs the low-level
    /// restore off the GIL, returning the re-announced suspension (`Some`) or
    /// `None` for an idle dump, paired with the dump's adopted script name (for
    /// restored snapshots' `script_name`). The restore turn runs no sandbox
    /// code, so it needs no print sink. Shared by
    /// [`load_session`](Self::load_session) and
    /// [`load_snapshot`](Self::load_snapshot).
    fn restore_turn(
        &self,
        py: Python<'_>,
        state: Vec<u8>,
        mounts: Vec<MountSpec>,
    ) -> PyResult<(Option<TurnEvent>, Option<String>)> {
        if self.used.swap(true, Ordering::Relaxed) {
            // non-destructive: an already-running session keeps its worker
            return Err(session_used_err());
        }
        let checkout = Arc::clone(&self.checkout);
        let result = py.detach(|| get_runtime().block_on(restore_turn(&checkout, state, mounts)));
        result.map_err(|e| pool_err_to_py(py, e))
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
            let pool = Pool::new(config)
                .await
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
            close_pool(pool).await;
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
        assert_message_annotations = AssertAnnotationsArg::default(),
        dataclass_registry = None,
    ))]
    #[expect(clippy::too_many_arguments)]
    fn checkout(
        &self,
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
        type_check: bool,
        type_check_stubs: Option<&Bound<'_, PyString>>,
        assert_message_annotations: AssertAnnotationsArg,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyAsyncMontySession> {
        Ok(PyAsyncMontySession {
            pool: Arc::clone(&self.pool),
            repl_config: parse_repl_config(
                py,
                script_name,
                limits,
                type_check,
                type_check_stubs,
                assert_message_annotations,
            )?,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            checkout: Arc::new(AsyncMutex::new(None)),
            used: AtomicBool::new(false),
        })
    }
}

/// Async context manager owning a pool of remote `monty` workers reached over a
/// WebSocket. The intended peer is `monty-server` (one child per connection
/// plus timeout/capacity/drain policy); any server that bridges to a worker fits,
/// e.g. a relay pairing this connection with a child that dialed in from the
/// other end, or the dev relay script.
///
/// Mirrors [`PyAsyncMonty`] but, instead of spawning local subprocesses, each
/// checkout dials the configured URL; `checkout()` yields the same
/// [`PyAsyncMontySession`]. There is no sync counterpart — remote turns are
/// network-bound, so the async API is the only one.
#[pyclass(name = "AsyncMontyWebsocket", module = "pydantic_monty", frozen)]
pub struct PyAsyncMontyWebsocket {
    config: PoolConfig,
    pool: SharedPool,
}

#[pymethods]
impl PyAsyncMontyWebsocket {
    /// Creates the pool configuration; connections are made by `async with` and
    /// each checkout (no workers are pre-warmed).
    ///
    /// `request_timeout` is the per-turn deadline in seconds (default 10.0): a
    /// remote relay that accepts the connection but never produces a worker
    /// would otherwise leave each turn blocked on the socket until the far end
    /// closes it. Pass `None` to wait indefinitely.
    ///
    /// Note that `install_dependencies` is also a turn, so the default 10.0 is
    /// often too low for it — a real `uv pip install` (e.g. `numpy`) can exceed
    /// it and surface as `MontyCrashedError`. Raise `request_timeout` (or pass
    /// `None`) when installing dependencies over the WebSocket transport.
    #[new]
    #[pyo3(signature = (
        url,
        *,
        max_processes = None,
        checkout_timeout = None,
        request_timeout = 10.0,
    ))]
    fn new(
        url: String,
        max_processes: Option<usize>,
        checkout_timeout: Option<f64>,
        request_timeout: Option<f64>,
    ) -> PyResult<Self> {
        Ok(Self {
            config: parse_websocket_config(url, max_processes, checkout_timeout, request_timeout)?,
            pool: Arc::new(Mutex::new(None)),
        })
    }

    /// Initializes the pool (off the event loop) and returns `self`.
    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        let config = slf.get().config.clone();
        let slot = Arc::clone(&slf.get().pool);
        future_into_py(py, async move {
            let pool = Pool::new(config)
                .await
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            *lock(&slot) = Some(Arc::new(pool));
            Ok(slf)
        })
    }

    /// Closes the pool: capacity is gone; checked-out sessions keep their
    /// connections until finished.
    #[pyo3(signature = (*_args))]
    fn __aexit__<'py>(&self, py: Python<'py>, _args: &Bound<'_, PyTuple>) -> PyResult<Bound<'py, PyAny>> {
        let pool = lock(&self.pool).take();
        future_into_py(py, async move {
            close_pool(pool).await;
            Ok(())
        })
    }

    /// Prepares a REPL session; the connection is opened by `async with`.
    #[pyo3(signature = (
        *,
        script_name = "main.py",
        limits = None,
        type_check = false,
        type_check_stubs = None,
        assert_message_annotations = AssertAnnotationsArg::default(),
        dataclass_registry = None,
    ))]
    #[expect(clippy::too_many_arguments)]
    fn checkout(
        &self,
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
        type_check: bool,
        type_check_stubs: Option<&Bound<'_, PyString>>,
        assert_message_annotations: AssertAnnotationsArg,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyAsyncMontySession> {
        Ok(PyAsyncMontySession {
            pool: Arc::clone(&self.pool),
            repl_config: parse_repl_config(
                py,
                script_name,
                limits,
                type_check,
                type_check_stubs,
                assert_message_annotations,
            )?,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            checkout: Arc::new(AsyncMutex::new(None)),
            used: AtomicBool::new(false),
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
    /// Set once the session has been fed or restored; `load_session` /
    /// `load_snapshot` are valid only while unset. See
    /// [`PyMontySession::load_snapshot`].
    used: AtomicBool,
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
            let checkout = pool
                .checkout(&repl_config)
                .await
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            *slot.lock().await = Some(checkout);
            Ok(slf)
        })
    }

    /// Returns the worker to the pool (best effort — a crashed worker has
    /// already been discarded and replaced). A cancelled `feed_run` may have
    /// left its turn future dropped mid-flight; `Checkout::finish` notices and
    /// discards the worker instead of returning it.
    #[pyo3(signature = (*_args))]
    fn __aexit__<'py>(&self, py: Python<'py>, _args: &Bound<'_, PyTuple>) -> PyResult<Bound<'py, PyAny>> {
        let slot = Arc::clone(&self.checkout);
        future_into_py(py, async move {
            finish_checkout(&slot).await;
            Ok(())
        })
    }

    /// Executes one snippet in the worker, driving external function calls
    /// (which may be coroutines, awaited concurrently), OS callbacks, and
    /// print callbacks in this process. Session state persists across feeds.
    ///
    /// Worker I/O runs off the event loop via tokio's blocking pool.
    #[pyo3(signature = (code, *, inputs=None, external_lookup=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    #[expect(clippy::too_many_arguments)]
    fn feed_run<'py>(
        &self,
        py: Python<'py>,
        code: &Bound<'_, PyString>,
        inputs: Option<&Bound<'_, PyDict>>,
        external_lookup: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.used.store(true, Ordering::Relaxed);
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
        let ext = external_lookup.map(|d| d.clone().unbind());
        future_into_py(py, async move { drive_async(args, ext).await })
    }

    /// Async counterpart of [`PyMontySession::feed_start`]: the returned
    /// coroutine resolves to a snapshot (whose `resume(...)` / `resume_auto()`
    /// is awaitable) or a `MontyComplete`. See that method for the
    /// snapshot-driven protocol and the `external_lookup` / `os` capture that
    /// backs `resume_auto()`.
    #[pyo3(signature = (code, *, inputs=None, external_lookup=None, print_callback=None, mount=None, os=None, skip_type_check=false))]
    #[expect(clippy::too_many_arguments)]
    fn feed_start<'py>(
        &self,
        py: Python<'py>,
        code: &Bound<'_, PyString>,
        inputs: Option<&Bound<'_, PyDict>>,
        external_lookup: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
        skip_type_check: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.used.store(true, Ordering::Relaxed);
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
        let ext = external_lookup.map(|d| d.clone().unbind());
        feed_start_async(py, args, ext, self.repl_config.script_name.clone())
    }

    /// Async counterpart of [`PyMontySession::load_session`]: the coroutine
    /// restores a dumped idle session, resolving to `None`. Valid only on a
    /// fresh session; raises if the dump is actually a suspended snapshot.
    fn load_session<'py>(&self, py: Python<'py>, state: Vec<u8>) -> PyResult<Bound<'py, PyAny>> {
        // claim the session in the synchronous prologue (which completes before
        // the future), so a concurrent call is rejected at call time and the
        // off-thread restore can't race onto a fresh session
        if self.used.swap(true, Ordering::Relaxed) {
            return Err(session_used_err());
        }
        let checkout = Arc::clone(&self.checkout);
        future_into_py(py, async move {
            // an idle session has no snapshot, so the restored name is unused
            let restored = restore_turn(&checkout, state, Vec::new())
                .await
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            if restored.0.is_some() {
                discard_checkout(&checkout).await;
                return Err(PyRuntimeError::new_err(
                    "this dump is a suspended snapshot — use load_snapshot() to resume it",
                ));
            }
            Ok(())
        })
    }

    /// Async counterpart of [`PyMontySession::load_snapshot`]: the coroutine
    /// restores a dumped suspended snapshot and resolves to it (whose
    /// `resume(...)` / `resume_auto()` is awaitable). Valid only on a fresh
    /// session; raises if the dump is actually an idle session. `external_lookup`
    /// / `os` are captured for `resume_auto()` with the same caveats as the sync
    /// method (a restored `FutureSnapshot` cannot be `resume_auto`'d).
    #[pyo3(signature = (state, *, mount=None, print_callback=None, external_lookup=None, os=None))]
    fn load_snapshot<'py>(
        &self,
        py: Python<'py>,
        state: Vec<u8>,
        mount: Option<&Bound<'_, PyAny>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        external_lookup: Option<&Bound<'_, PyDict>>,
        os: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // extract args before committing the session (a bad-args error leaves
        // it loadable), then claim it in the synchronous prologue
        check_os_callable(py, os.as_ref())?;
        let mounts = extract_mount_specs(mount)?;
        let print_target = PrintTarget::from_py(print_callback)?;
        let ext = external_lookup.map(|d| d.clone().unbind());
        if self.used.swap(true, Ordering::Relaxed) {
            return Err(session_used_err());
        }
        let checkout = Arc::clone(&self.checkout);
        let dc_registry = self.dc_registry.clone_ref(py);
        let config_script_name = self.repl_config.script_name.clone();
        future_into_py(py, async move {
            let (event, restored_script_name) = restore_turn(&checkout, state, mounts)
                .await
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            let Some(event) = event else {
                discard_checkout(&checkout).await;
                return Err(PyRuntimeError::new_err(
                    "this dump is an idle session — use load_session() to restore it",
                ));
            };
            // the dump's own script name, falling back to the session config
            // only if the worker did not report one (e.g. an older child)
            let script_name = restored_script_name.unwrap_or(config_script_name);
            Python::attach(|py| {
                let ctx = DriveContext::new(checkout, dc_registry, print_target, script_name, ext, os);
                build_snapshot(py, ctx, event, true)
            })
        })
    }

    /// Serializes the worker's session state (idle or suspended) into opaque
    /// bytes via monty's existing dump format. The session stays usable.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let checkout = Arc::clone(&self.checkout);
        future_into_py(py, async move {
            let state = dump_checkout(&checkout)
                .await
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            Ok(Python::attach(|py| PyBytes::new(py, &state).unbind()))
        })
    }

    /// Async counterpart of [`PyMontySession::install_dependencies`]: the
    /// coroutine installs the packages off the event loop, resolving to `None`.
    /// Raises `MontyRuntimeError` against the `monty` sandbox worker or on a uv
    /// install failure; the session stays usable.
    fn install_dependencies<'py>(&self, py: Python<'py>, requirements: Vec<String>) -> PyResult<Bound<'py, PyAny>> {
        self.used.store(true, Ordering::Relaxed);
        let checkout = Arc::clone(&self.checkout);
        future_into_py(py, async move {
            install_deps_checkout(&checkout, requirements)
                .await
                .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))?;
            // resolve to None (whereas `()` would convert to an empty tuple) to
            // match the `-> None` stub and the sync method
            Ok(None::<()>)
        })
    }

    /// OS process id of this session's worker, or `None` when no worker is
    /// attached or a turn is in flight (diagnostics/tests). Non-blocking for
    /// the same reason as the sync getter.
    #[getter]
    fn worker_pid(&self) -> Option<u32> {
        self.checkout.try_lock().ok()?.as_ref().and_then(Checkout::pid)
    }
}

// =============================================================================
// Shared argument parsing
// =============================================================================

/// Builds the subprocess-transport `monty-pool` config from the (shared)
/// `Monty`/`AsyncMonty` constructor arguments, resolving the binary via
/// `pydantic_monty._binary` when not given explicitly.
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
    let mut config = PoolConfig::subprocess(binary_path);
    config.min_processes = min_processes;
    if let Some(max) = max_processes {
        config.max_processes = max;
    }
    config.checkout_timeout = checkout_timeout.map(duration_from_secs).transpose()?;
    config.request_timeout = request_timeout.map(duration_from_secs).transpose()?;
    config.max_checkouts_per_worker = max_checkouts_per_worker;
    Ok(config)
}

/// Rejects a non-callable `os=` handler with the same `TypeError` for every
/// entry point that accepts one (`feed_run` / `feed_start` via
/// [`FeedArgs::extract`], and `load_snapshot`).
fn check_os_callable(py: Python<'_>, os: Option<&Py<PyAny>>) -> PyResult<()> {
    if let Some(os_cb) = os
        && !os_cb.bind(py).is_callable()
    {
        let t = os_cb.bind(py).get_type().name()?;
        return Err(PyTypeError::new_err(format!("'{t}' object is not callable")));
    }
    Ok(())
}

/// The error raised when `load_session` / `load_snapshot` is called on a
/// session that has already been fed or restored (it would otherwise silently
/// discard work).
fn session_used_err() -> PyErr {
    PyRuntimeError::new_err(
        "load_session / load_snapshot is only valid on a fresh session, before any feed_run / feed_start / load_session / load_snapshot",
    )
}

/// Runs the low-level restore turn, returning the re-announced suspension
/// (`Some`) or `None` for an idle dump, paired with the dump's adopted script
/// name. The restore turn runs no sandbox code, so it needs no print sink. A
/// failed restore (bad mount, protocol desync, ...) leaves the worker in an
/// untrusted state: it is discarded so a later feed fails fast rather than
/// running on a half-restored session. Shared by the sync and async
/// `load_session` / `load_snapshot`.
async fn restore_turn(
    checkout: &SharedCheckout,
    state: Vec<u8>,
    mounts: Vec<MountSpec>,
) -> Result<(Option<TurnEvent>, Option<String>), PoolError> {
    let result = {
        let mut guard = checkout.lock().await;
        match guard.as_mut() {
            Some(checkout) => {
                checkout
                    .restore(state, mounts, &mut monty_pool::on_print_sync(|_, _| {}))
                    .await
            }
            None => Err(PoolError::Finished),
        }
    };
    // discard the worker on failure (the lock is released above) so a later
    // feed fails fast — a failed load is not retryable
    if result.is_err() {
        discard_checkout(checkout).await;
    }
    result
}

/// Kills the session's worker and empties its checkout slot after a failed
/// load. Any subsequent feed then fails with [`PoolError::Finished`] — like a
/// crashed session — enforcing that a failed load is not retryable (callers
/// must check out a fresh session). Does no protocol I/O.
pub(crate) async fn discard_checkout(checkout: &SharedCheckout) {
    // take in its own statement so the lock is released before the worker is
    // dropped (its `Drop` kills the process)
    let taken = checkout.lock().await.take();
    drop(taken);
}

/// Gracefully closes a pool taken out of its shared slot (a no-op when the
/// context manager was never entered). Shared by the sync `__exit__` and the
/// async `__aexit__`s.
async fn close_pool(pool: Option<Arc<Pool>>) {
    if let Some(pool) = pool {
        pool.close().await;
        drop(pool);
    }
}

/// Takes the session's checkout out of its slot and finishes it, returning
/// the worker to the pool (best effort — errors are swallowed, matching the
/// previous context-manager behaviour). Shared by the sync `__exit__` and
/// the async `__aexit__`.
async fn finish_checkout(checkout: &SharedCheckout) {
    // take in its own statement so the lock is released before the finish
    // turn runs
    let taken = checkout.lock().await.take();
    if let Some(taken) = taken {
        let _ = taken.finish().await;
    }
}

/// Builds the WebSocket-transport `monty-pool` config from the `AsyncMontyWebsocket`
/// constructor arguments. Each checkout dials `url` verbatim; there is no
/// pre-warming, so `min_processes` stays 0 and `max_processes` caps concurrent
/// connections.
fn parse_websocket_config(
    url: String,
    max_processes: Option<usize>,
    checkout_timeout: Option<f64>,
    request_timeout: Option<f64>,
) -> PyResult<PoolConfig> {
    let mut config = PoolConfig::websocket(url);
    if let Some(max) = max_processes {
        config.max_processes = max;
    }
    config.checkout_timeout = checkout_timeout.map(duration_from_secs).transpose()?;
    config.request_timeout = request_timeout.map(duration_from_secs).transpose()?;
    Ok(config)
}

/// Builds the worker-side REPL session config from the (shared) `checkout`
/// arguments.
pub(crate) fn parse_repl_config(
    py: Python<'_>,
    script_name: &str,
    limits: Option<&Bound<'_, PyDict>>,
    type_check: bool,
    type_check_stubs: Option<&Bound<'_, PyString>>,
    assert_message_annotations: AssertAnnotationsArg,
) -> PyResult<ReplConfig> {
    Ok(ReplConfig {
        script_name: script_name.to_owned(),
        limits: limits.map(extract_limits).transpose()?,
        type_check,
        type_check_stubs: extract_type_check_stubs(py, type_check_stubs)?,
        assert_message_annotations: assert_message_annotations.0,
    })
}

/// The `assert_message_annotations` checkout argument: `True`/`False`, or an
/// int giving a custom operand-repr truncation length in bytes.
#[derive(Clone, Copy, Default)]
pub(crate) struct AssertAnnotationsArg(pub AssertMessageAnnotations);

impl<'a, 'py> FromPyObject<'a, 'py> for AssertAnnotationsArg {
    type Error = PyErr;

    fn extract(ob: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        // Check bool before int because `True` must not become a one-byte cap.
        if let Ok(enabled) = ob.cast_exact::<PyBool>() {
            Ok(Self(enabled.is_true().into()))
        } else if ob.cast::<PyInt>().is_ok() {
            // `NonZeroU32` rejects 0: it encodes `Off` on the wire, so a 0
            // limit must be spelled `False`.
            match ob.extract::<u32>().ok().and_then(NonZeroU32::new) {
                Some(n) => Ok(Self(AssertMessageAnnotations::MaxBytes(n))),
                None => Err(PyValueError::new_err(
                    "assert_message_annotations int value must be between 1 and 2**32 - 1",
                )),
            }
        } else {
            Err(PyTypeError::new_err(
                "assert_message_annotations must be a bool or an int truncation length",
            ))
        }
    }
}

/// Clones the live pool handle out of a shared slot, erroring when the
/// context manager has not been entered (or already exited).
pub(crate) fn active_pool(pool: &SharedPool) -> PyResult<Arc<Pool>> {
    lock(pool).as_ref().map(Arc::clone).ok_or_else(|| {
        PyRuntimeError::new_err("the pool is not active — enter the Monty / AsyncMonty context manager first")
    })
}

/// Dumps the session of a live checkout (shared by the sync and async dump
/// methods; runs without the GIL).
async fn dump_checkout(checkout: &SharedCheckout) -> Result<Vec<u8>, PoolError> {
    let mut guard = checkout.lock().await;
    match guard.as_mut() {
        Some(checkout) => checkout.dump().await,
        None => Err(PoolError::Finished),
    }
}

/// Installs dependencies into a live checkout's session (shared by the sync and
/// async `install_dependencies` methods; runs without the GIL).
async fn install_deps_checkout(checkout: &SharedCheckout, requirements: Vec<String>) -> Result<(), PoolError> {
    let mut guard = checkout.lock().await;
    match guard.as_mut() {
        Some(checkout) => checkout.install_dependencies(requirements).await,
        None => Err(PoolError::Finished),
    }
}

/// Everything a feed needs, extracted from Python arguments up front so the
/// sync and async drive loops share one validation path.
pub(crate) struct FeedArgs {
    pub(crate) code: String,
    pub(crate) inputs: Vec<(String, MontyObject)>,
    pub(crate) mounts: Vec<MountSpec>,
    pub(crate) skip_type_check: bool,
    pub(crate) os: Option<Py<PyAny>>,
    pub(crate) print_target: PrintTarget,
    pub(crate) checkout: SharedCheckout,
    pub(crate) dc_registry: DcRegistry,
}

impl FeedArgs {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn extract(
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
        check_os_callable(py, os.as_ref())?;
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

/// Synchronous drive loop: protocol turns block the calling thread (GIL
/// released) via `block_on`; callbacks run between turns with the GIL held.
fn drive_sync(py: Python<'_>, args: FeedArgs, external_lookup: Option<&Bound<'_, PyDict>>) -> PyResult<Py<PyAny>> {
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
    let lookup = ExternalLookup::new(py, external_lookup, &dc_registry);
    let mut event = run_turn_sync(
        py,
        &checkout,
        &print_target,
        turn_fn(move |c, p| Box::pin(async move { c.feed(&code, inputs, mounts, skip_type_check, p).await })),
    )?;

    loop {
        // `Complete` ends the loop; on any other event a failure to compute the
        // answer discards the checkout (see `sync_turn_answer`).
        let resume_with = match event {
            TurnEvent::Complete(value) => return monty_to_py(py, &value, &dc_registry),
            // This feed's mounts get first refusal on every OS call; only what
            // they don't cover reaches the `os=` callback.
            TurnEvent::OsCall {
                function_name,
                args,
                kwargs,
                ..
            } => {
                let mounted = run_turn_sync(
                    py,
                    &checkout,
                    &print_target,
                    turn_fn(|c, p| Box::pin(c.resume_from_mounts(p))),
                )?;
                match mounted {
                    Some(next) => {
                        event = next;
                        continue;
                    }
                    None => TurnAnswer::Call(dispatch_os_parts(
                        py,
                        &function_name,
                        &args,
                        &kwargs,
                        os.as_ref(),
                        &dc_registry,
                    )),
                }
            }
            event => match sync_turn_answer(py, event, &lookup, &dc_registry) {
                Ok(answer) => answer,
                Err(err) => {
                    py.detach(|| get_runtime().block_on(discard_checkout(&checkout)));
                    return Err(err);
                }
            },
        };
        event = run_turn_sync(
            py,
            &checkout,
            &print_target,
            turn_fn(move |c, p| {
                Box::pin(async move {
                    match resume_with {
                        TurnAnswer::Call(value) => c.resume(value, p).await,
                        TurnAnswer::Name(value) => c.resume_name_lookup(value, p).await,
                    }
                })
            }),
        )?;
    }
}

/// Computes the resume answer for a (non-`Complete`) sync suspension. Split out
/// of [`drive_sync`]'s loop so a failure here — e.g. converting an
/// `external_lookup` value for a `NameLookup` — can discard the suspended worker
/// instead of returning while it waits forever for a resume the aborted feed
/// will never send.
fn sync_turn_answer(
    py: Python<'_>,
    event: TurnEvent,
    lookup: &ExternalLookup<'_, '_>,
    dc_registry: &DcRegistry,
) -> PyResult<TurnAnswer> {
    match event {
        TurnEvent::FunctionCall {
            function_name,
            args,
            kwargs,
            method_call,
            ..
        } => {
            let result = if method_call {
                dispatch_method_call(py, &function_name, &args, &kwargs, dc_registry)
            } else {
                lookup.call(&function_name, &args, &kwargs)
            };
            Ok(TurnAnswer::Call(ext_to_resume(result)?))
        }
        TurnEvent::NameLookup { name } => Ok(TurnAnswer::Name(lookup.resolve_name(&name)?)),
        TurnEvent::ResolveFutures { .. } => Err(PyRuntimeError::new_err("async external functions require AsyncMonty")),
        TurnEvent::Complete(_) | TurnEvent::OsCall { .. } => {
            unreachable!("Complete and OsCall are handled by the drive loop")
        }
    }
}

/// Async drive loop: protocol turns are awaited directly on the runtime;
/// coroutine external functions are spawned as tasks and resolved via
/// `ResolveFutures`.
async fn drive_async(args: FeedArgs, external_lookup: Option<Py<PyDict>>) -> PyResult<Py<PyAny>> {
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

    // A suspension already pending as a new feed starts means the previous
    // `feed_run` future was cancelled mid-suspension (this loop otherwise
    // always answers or discards): its coroutine tasks died with it, so the
    // suspension can never be answered. Discard the worker and fail cleanly
    // instead of wedging the session behind a worker nobody can resume.
    let abandoned = checkout
        .lock()
        .await
        .as_ref()
        .is_some_and(Checkout::has_pending_suspension);
    if abandoned {
        discard_checkout(&checkout).await;
        return Err(PyRuntimeError::new_err(
            "a previous feed_run was cancelled while a suspension awaited an answer; the worker was discarded",
        ));
    }

    let mut event = run_turn_async(
        &checkout,
        &print_target,
        turn_fn(move |c, p| Box::pin(async move { c.feed(&code, inputs, mounts, skip_type_check, p).await })),
    )
    .await?;

    loop {
        // As in `drive_sync`, a failure to answer a suspension (including the
        // futures `ResolveFutures` awaits erroring) discards the checkout.
        // `Complete` and `ResolveFutures` stay inline — the latter must await
        // the pending tasks.
        let answer: TurnAnswer = match event {
            TurnEvent::Complete(value) => {
                return Python::attach(|py| monty_to_py(py, &value, &dc_registry));
            }
            TurnEvent::ResolveFutures { .. } => {
                let resolved = wait_for_futures(&mut join_set).await.and_then(|results| {
                    results
                        .into_iter()
                        .map(|(call_id, result)| Ok((call_id, ext_to_resume(result)?)))
                        .collect::<PyResult<Vec<_>>>()
                });
                let results = match resolved {
                    Ok(results) => results,
                    Err(err) => {
                        discard_checkout(&checkout).await;
                        return Err(err);
                    }
                };
                event = run_turn_async(
                    &checkout,
                    &print_target,
                    turn_fn(move |c, p| Box::pin(c.resume_futures(results, p))),
                )
                .await?;
                continue;
            }
            // Mounts get first refusal, as in `drive_sync`.
            TurnEvent::OsCall {
                function_name,
                args,
                kwargs,
                ..
            } => {
                let mounted = run_turn_async(
                    &checkout,
                    &print_target,
                    turn_fn(|c, p| Box::pin(c.resume_from_mounts(p))),
                )
                .await?;
                if let Some(next) = mounted {
                    event = next;
                    continue;
                }
                let value = Python::attach(|py| {
                    dispatch_os_parts(py, &function_name, &args, &kwargs, os.as_ref(), &dc_registry)
                });
                TurnAnswer::Call(value)
            }
            event => match async_turn_answer(event, external_lookup.as_ref(), &dc_registry, &mut join_set) {
                Ok(answer) => answer,
                Err(err) => {
                    discard_checkout(&checkout).await;
                    return Err(err);
                }
            },
        };
        event = run_turn_async(
            &checkout,
            &print_target,
            turn_fn(move |c, p| {
                Box::pin(async move {
                    match answer {
                        TurnAnswer::Call(value) => c.resume(value, p).await,
                        TurnAnswer::Name(value) => c.resume_name_lookup(value, p).await,
                    }
                })
            }),
        )
        .await?;
    }
}

/// Async counterpart of [`sync_turn_answer`] (minus `ResolveFutures`, which
/// must await in [`drive_async`]'s loop): a failure here lets the loop discard
/// the suspended worker instead of leaving it waiting forever for a resume.
fn async_turn_answer(
    event: TurnEvent,
    external_lookup: Option<&Py<PyDict>>,
    dc_registry: &DcRegistry,
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
) -> PyResult<TurnAnswer> {
    match event {
        TurnEvent::FunctionCall {
            function_name,
            args,
            kwargs,
            call_id,
            method_call,
        } => match dispatch_function_call(
            &function_name,
            method_call,
            &args,
            &kwargs,
            external_lookup,
            dc_registry,
        ) {
            CallResult::Sync(result) => Ok(TurnAnswer::Call(ext_to_resume(result)?)),
            CallResult::Coroutine(coro) => {
                spawn_coroutine_task(join_set, call_id, coro, dc_registry)?;
                Ok(TurnAnswer::Call(ResumeValue::Future))
            }
        },
        TurnEvent::NameLookup { name } => {
            let value = Python::attach(|py| {
                ExternalLookup::new(py, external_lookup.map(|d| d.bind(py)), dc_registry).resolve_name(&name)
            })?;
            Ok(TurnAnswer::Name(value))
        }
        TurnEvent::Complete(_) | TurnEvent::ResolveFutures { .. } | TurnEvent::OsCall { .. } => {
            unreachable!("Complete, ResolveFutures and OsCall are handled by the drive loop")
        }
    }
}

/// The caller's answer to a suspension, paired with which resume call
/// delivers it.
enum TurnAnswer {
    Call(ResumeValue),
    Name(Option<MontyObject>),
}

/// What a turn helper may return, so one implementation serves both an
/// ordinary resume ([`TurnEvent`]) and a mount attempt (`Option<TurnEvent>`,
/// `None` when no mount covered the call).
///
/// Only [`run_turn`]'s print-failure cleanup needs to tell these
/// apart: it must know whether the worker is left suspended.
pub(crate) trait TurnOutcome {
    /// Whether the worker is left waiting for a resume after this outcome.
    fn left_suspended(&self) -> bool;
}

impl TurnOutcome for TurnEvent {
    fn left_suspended(&self) -> bool {
        matches!(
            self,
            Self::FunctionCall { .. } | Self::OsCall { .. } | Self::NameLookup { .. } | Self::ResolveFutures { .. }
        )
    }
}

impl TurnOutcome for Option<TurnEvent> {
    /// `None` means no mount covered the call and no turn ran, so the OS-call
    /// suspension the caller already held is still pending.
    fn left_suspended(&self) -> bool {
        self.as_ref().is_none_or(TurnEvent::left_suspended)
    }
}

/// Runs one protocol turn against the (locked) checkout, streaming prints to
/// `print_target` and capturing the first print-callback failure.
///
/// Print delivery is synchronous inside the turn (the callback attaches the
/// GIL when needed and returns a ready future), preserving print ordering and
/// backpressure against the worker.
pub(crate) async fn run_turn<T: TurnOutcome>(
    checkout: &SharedCheckout,
    print_target: &PrintTarget,
    turn: impl for<'a> FnOnce(&'a mut Checkout, OnPrint<'a>) -> TurnFuture<'a, T> + Send,
) -> (Result<T, PoolError>, Option<MontyException>) {
    let mut guard = checkout.lock().await;
    let Some(checkout) = guard.as_mut() else {
        return (Err(PoolError::Finished), None);
    };
    let mut print_err: Option<MontyException> = None;
    let result = {
        let mut on_print = |stream: PrintStream, text: &str| -> PrintFuture {
            if print_err.is_none()
                && let Err(err) = print_target.write_event(stream, text)
            {
                print_err = Some(err);
            }
            Box::pin(ready(()))
        };
        turn(checkout, &mut on_print).await
    };
    // A print-callback failure aborts the feed. If the turn left the worker
    // suspended awaiting a resume the aborted feed will never send, drop the
    // checkout so the session ends cleanly rather than wedging the next feed
    // with a dangling suspension.
    if print_err.is_some() && result.as_ref().is_ok_and(TurnOutcome::left_suspended) {
        *guard = None;
    }
    (result, print_err)
}

/// Blocks the calling thread on one protocol turn (GIL released) and
/// finalizes it — the sync sessions' counterpart of [`run_turn_async`].
pub(crate) fn run_turn_sync<T: TurnOutcome + Send>(
    py: Python<'_>,
    checkout: &SharedCheckout,
    print_target: &PrintTarget,
    turn: impl for<'a> FnOnce(&'a mut Checkout, OnPrint<'a>) -> TurnFuture<'a, T> + Send,
) -> PyResult<T> {
    let (result, print_err) = py.detach(|| get_runtime().block_on(run_turn(checkout, print_target, turn)));
    finalize_turn(py, result, print_err)
}

/// Awaits one protocol turn and finalizes it for the async drive loops.
pub(crate) async fn run_turn_async<T: TurnOutcome>(
    checkout: &SharedCheckout,
    print_target: &PrintTarget,
    turn: impl for<'a> FnOnce(&'a mut Checkout, OnPrint<'a>) -> TurnFuture<'a, T> + Send,
) -> PyResult<T> {
    let (result, print_err) = run_turn(checkout, print_target, turn).await;
    Python::attach(|py| finalize_turn(py, result, print_err))
}

/// Converts a turn outcome into the next event, surfacing print-callback
/// failures (which take precedence — they are host-side errors).
pub(crate) fn finalize_turn<T>(
    py: Python<'_>,
    result: Result<T, PoolError>,
    print_err: Option<MontyException>,
) -> PyResult<T> {
    if let Some(err) = print_err {
        Err(MontyError::new_err(py, err))
    } else {
        result.map_err(|e| pool_err_to_py(py, e))
    }
}

// =============================================================================
// Dispatch helpers
// =============================================================================

/// Maps an `ExtFunctionResult` from callback dispatch onto the pool's resume
/// payload.
pub(crate) fn ext_to_resume(result: ExtFunctionResult) -> PyResult<ResumeValue> {
    match result {
        ExtFunctionResult::Return(value) => Ok(ResumeValue::Return(value)),
        ExtFunctionResult::Error(exc) => Ok(ResumeValue::Error(exc)),
        ExtFunctionResult::NotFound(_) => Ok(ResumeValue::NotFound),
        // futures are handled explicitly by the async loop before this point
        ExtFunctionResult::Future(_) => Err(PyRuntimeError::new_err("unexpected future result")),
    }
}

/// Calls the Python `os=` fallback for a bubbled OS call. With no callback —
/// or when it returns `NOT_HANDLED` — answers [`ResumeValue::NotHandled`], so
/// the sandbox raises monty's per-call no-handler default itself.
pub(crate) fn dispatch_os_parts(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    os: Option<&Py<PyAny>>,
    dc_registry: &DcRegistry,
) -> ResumeValue {
    let Some(os_callback) = os else {
        return ResumeValue::NotHandled;
    };
    let call = || -> PyResult<ResumeValue> {
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
            return Ok(ResumeValue::NotHandled);
        }
        Ok(match py_to_monty_value(&result, dc_registry) {
            Ok(obj) => ResumeValue::Return(obj),
            Err(exc) => ResumeValue::Error(exc),
        })
    };
    call().unwrap_or_else(|err| ResumeValue::Error(exc_py_to_monty(py, &err)))
}

/// Extracts `MountDir | list[MountDir] | None` into mount specs for the pool,
/// which services mount I/O on the parent side — nothing crosses the process
/// boundary, and overlay writes are discarded when the feed ends.
fn extract_mount_specs(mount: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<MountSpec>> {
    let Some(mount) = mount else {
        return Ok(vec![]);
    };
    if let Ok(single) = mount.extract::<PyRef<'_, PyMountDir>>() {
        return Ok(vec![single.spec()]);
    }
    if let Ok(list) = mount.cast::<PyList>() {
        return list
            .iter()
            .map(|item| {
                let dir = item.extract::<PyRef<'_, PyMountDir>>()?;
                Ok(dir.spec())
            })
            .collect();
    }
    Err(PyTypeError::new_err(
        "mount must be a MountDir, a list of MountDir, or None",
    ))
}

/// Maps a pool failure onto the Python exception hierarchy.
pub(crate) fn pool_err_to_py(py: Python<'_>, err: PoolError) -> PyErr {
    let message = err.to_string();
    match err {
        PoolError::Runtime(exc) => MontyError::new_err(py, exc),
        PoolError::Typing(diagnostics) => MontyTypingError::new_err(py, diagnostics),
        PoolError::Crashed { status, .. } => {
            MontyCrashedError::new_err(py, message, false, status.and_then(|s| s.code()))
        }
        PoolError::Timeout { .. } => MontyCrashedError::new_err(py, message, true, None),
        PoolError::Disconnected { .. } => MontyDisconnectError::new_err(py, message),
        PoolError::Shutdown { dump } => MontyShutdown::new_err(py, message, dump),
        PoolError::Exhausted => PyTimeoutError::new_err(message),
        PoolError::Protocol(_) | PoolError::Spawn(_) | PoolError::Finished => PyRuntimeError::new_err(message),
    }
}

fn duration_from_secs(secs: f64) -> PyResult<Duration> {
    Duration::try_from_secs_f64(secs).map_err(|err| PyValueError::new_err(format!("invalid timeout: {err}")))
}

/// Locks the shared *pool* slot, ignoring poisoning (a panic elsewhere must
/// not wedge the pool). Only ever held to clone/take the `Arc<Pool>` — the
/// per-session checkout slot is a tokio mutex locked with `.lock().await`.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
