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
    num::NonZeroU32,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, TryLockError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ::monty::{AssertMessageAnnotations, ExcType, ExtFunctionResult, MontyException, MontyObject};
use monty_pool::{Checkout, MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, ReplConfig, ResumeValue, TurnEvent};
use monty_proto::python::{DcRegistry, exc_py_to_monty, monty_to_py, py_to_monty_value};
use pyo3::{
    Borrowed,
    exceptions::{PyRuntimeError, PyTimeoutError, PyTypeError, PyValueError},
    prelude::*,
    types::{PyBool, PyBytes, PyDict, PyInt, PyList, PyString, PyTuple},
};
use pyo3_async_runtimes::tokio::future_into_py;
use tokio::task::{JoinSet, spawn_blocking};

use crate::{
    async_dispatch::{dispatch_function_call, join_error_to_py, spawn_coroutine_task, wait_for_futures},
    build::{extract_repl_inputs, extract_source_code, extract_type_check_stubs},
    exceptions::{MontyCrashedError, MontyError, MontyTypingError},
    external::{CallResult, ExternalLookup, dispatch_method_call},
    get_not_handled,
    limits::extract_limits,
    mount::PyMountDir,
    print_target::PrintTarget,
    snapshot::{DriveContext, build_snapshot, feed_start_async, feed_start_sync},
};

/// The pool handle shared between a pool object and its sessions. `None`
/// until the context manager is entered and again after it exits.
pub(crate) type SharedPool = Arc<Mutex<Option<Arc<Pool>>>>;
/// The worker handle of one session. `None` before the session is entered,
/// after it exits, and after the worker is discarded on a crash.
pub(crate) type SharedCheckout = Arc<Mutex<Option<Checkout>>>;

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
            checkout: Arc::new(Mutex::new(None)),
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
    /// Set once the session has been fed or restored. `load_snapshot` is valid
    /// only while this is unset (a fresh, undriven session).
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
    /// Unlike [`feed_run`](Self::feed_run), external calls and name lookups are
    /// surfaced as snapshots rather than auto-dispatched — that is the point of
    /// `feed_start`. An `external_lookup` (and `os=`) may still be supplied: it
    /// is *not* consulted during this drive but is captured on the snapshot so
    /// `snapshot.resume_auto()` can answer subsequent suspensions from it,
    /// letting a caller iterate to completion without resolving each call by
    /// hand. An `os=` handler additionally auto-dispatches uncovered OS calls
    /// until the next non-OS event, exactly as before.
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
    fn load(&self, py: Python<'_>, state: Vec<u8>) -> PyResult<()> {
        // an idle session has no snapshot, so the restored script name is unused
        if self.restore_turn(py, state, Vec::new())?.0.is_some() {
            py.detach(|| discard_checkout(&self.checkout));
            return Err(PyRuntimeError::new_err(
                "this dump is a suspended snapshot — use load_snapshot() to resume it",
            ));
        }
        Ok(())
    }

    /// Restores a dumped **suspended** snapshot — bytes from `feed_start` +
    /// `snapshot.dump()` — and returns the re-announced snapshot to resume. Use
    /// [`load`](Self::load) for a dump taken between feeds.
    ///
    /// Valid only on a fresh session, before any feed or load; raises
    /// `RuntimeError` otherwise. `mount` re-establishes the suspended feed's
    /// mounts (whose host paths are not in the dump), validated against the
    /// dump's recorded requirements. The dump restores its own config; the
    /// dataclass registry from `checkout()` is reused. Raises if the dump is
    /// actually an idle session.
    ///
    /// `external_lookup` / `os` are captured on the restored snapshot so it
    /// supports `resume_auto()`, just like `feed_start`. Two caveats apply to a
    /// restored snapshot: a restored `FutureSnapshot`'s pending coroutines are
    /// gone (they lived in the previous process), so async `resume_auto()` on it
    /// raises — resolve it manually with `resume({call_id: ...})`; and a
    /// restored OS-call snapshot carries no args/kwargs, so prefer manual
    /// `resume` / `resume_not_handled` there rather than `resume_auto`.
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
            py.detach(|| discard_checkout(&self.checkout));
            return Err(PyRuntimeError::new_err(
                "this dump is an idle session — use load() to restore it",
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
            .detach(|| dump_checkout(&self.checkout))
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
        py.detach(|| install_deps_checkout(&self.checkout, requirements))
            .map_err(|e| pool_err_to_py(py, e))
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

impl PyMontySession {
    /// Claims the fresh session (rejecting a reused one) and runs the low-level
    /// restore off the GIL, returning the re-announced suspension (`Some`) or
    /// `None` for an idle dump, paired with the dump's adopted script name (for
    /// restored snapshots' `script_name`). The restore turn runs no sandbox
    /// code, so it needs no print sink. Shared by [`load`](Self::load) and
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
        let result = py.detach(|| {
            let mut guard = lock(&checkout);
            guard
                .as_mut()
                .ok_or(PoolError::Finished)?
                .restore(state, mounts, &mut |_, _| {})
        });
        // a failed restore (bad mount, protocol desync, ...) leaves the worker
        // in an untrusted state: discard it so a later feed fails fast rather
        // than running on a half-restored session
        if result.is_err() {
            py.detach(|| discard_checkout(&checkout));
        }
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
            checkout: Arc::new(Mutex::new(None)),
            used: AtomicBool::new(false),
        })
    }
}

/// Async context manager owning a pool of remote `monty` workers reached over a
/// WebSocket. The dialed peer is the server side: a relay that pairs this
/// connection with a child (e.g. `monty-cpython websocket`, which dials in from
/// the other end), or any server that bridges to a worker.
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
            let pool = spawn_blocking(move || Pool::new(config))
                .await
                .map_err(join_error_to_py)?
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
            spawn_blocking(move || drop(pool)).await.map_err(join_error_to_py)?;
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
            checkout: Arc::new(Mutex::new(None)),
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
    /// Set once the session has been fed or restored; `load_snapshot` is valid
    /// only while unset. See [`PyMontySession::load_snapshot`].
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

    /// Async counterpart of [`PyMontySession::load`]: the coroutine restores a
    /// dumped idle session, resolving to `None`. Valid only on a fresh session;
    /// raises if the dump is actually a suspended snapshot.
    fn load<'py>(&self, py: Python<'py>, state: Vec<u8>) -> PyResult<Bound<'py, PyAny>> {
        // claim the session in the synchronous prologue (which completes before
        // the future), so a concurrent call is rejected at call time and the
        // off-thread restore can't race onto a fresh session
        if self.used.swap(true, Ordering::Relaxed) {
            return Err(session_used_err());
        }
        let checkout = Arc::clone(&self.checkout);
        future_into_py(py, async move {
            // an idle session has no snapshot, so the restored name is unused
            if restore_turn_async(Arc::clone(&checkout), state, Vec::new())
                .await?
                .0
                .is_some()
            {
                spawn_blocking(move || discard_checkout(&checkout))
                    .await
                    .map_err(join_error_to_py)?;
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
            let (event, restored_script_name) = restore_turn_async(Arc::clone(&checkout), state, mounts).await?;
            let Some(event) = event else {
                spawn_blocking(move || discard_checkout(&checkout))
                    .await
                    .map_err(join_error_to_py)?;
                return Err(PyRuntimeError::new_err(
                    "this dump is an idle session — use load() to restore it",
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
            let state = spawn_blocking(move || dump_checkout(&checkout))
                .await
                .map_err(join_error_to_py)?
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
            spawn_blocking(move || install_deps_checkout(&checkout, requirements))
                .await
                .map_err(join_error_to_py)?
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
        try_lock(&self.checkout)?.as_ref().and_then(Checkout::pid)
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

/// The error raised when `load` / `load_snapshot` is called on a session that
/// has already been fed or restored (it would otherwise silently discard work).
fn session_used_err() -> PyErr {
    PyRuntimeError::new_err(
        "load / load_snapshot is only valid on a fresh session, before any feed_run / feed_start / load / load_snapshot",
    )
}

/// Runs the low-level restore off the event loop (via `spawn_blocking`),
/// returning the re-announced suspension (`Some`) or `None` for an idle dump,
/// paired with the dump's adopted script name. The restore turn runs no sandbox
/// code, so it needs no print sink. Shared by the async
/// [`PyAsyncMontySession::load`] / `load_snapshot`.
async fn restore_turn_async(
    checkout: SharedCheckout,
    state: Vec<u8>,
    mounts: Vec<MountSpec>,
) -> PyResult<(Option<TurnEvent>, Option<String>)> {
    spawn_blocking(move || {
        let result = {
            let mut guard = lock(&checkout);
            guard
                .as_mut()
                .ok_or(PoolError::Finished)
                .and_then(|checkout| checkout.restore(state, mounts, &mut |_, _| {}))
        };
        // discard the worker on failure (the lock is released above) so a later
        // feed fails fast — a failed load is not retryable
        if result.is_err() {
            discard_checkout(&checkout);
        }
        result
    })
    .await
    .map_err(join_error_to_py)?
    .map_err(|e| Python::attach(|py| pool_err_to_py(py, e)))
}

/// Kills the session's worker and empties its checkout slot after a failed
/// load. Any subsequent feed then fails with [`PoolError::Finished`] — like a
/// crashed session — enforcing that a failed load is not retryable (callers
/// must check out a fresh session). Does no protocol I/O, so it never blocks.
pub(crate) fn discard_checkout(checkout: &SharedCheckout) {
    // take in its own statement so the lock is released before the worker is
    // dropped (its `Drop` kills the process)
    let taken = lock(checkout).take();
    drop(taken);
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
fn dump_checkout(checkout: &SharedCheckout) -> Result<Vec<u8>, PoolError> {
    let mut guard = lock(checkout);
    guard.as_mut().ok_or(PoolError::Finished).and_then(Checkout::dump)
}

/// Installs dependencies into a live checkout's session (shared by the sync and
/// async `install_dependencies` methods; runs without the GIL).
fn install_deps_checkout(checkout: &SharedCheckout, requirements: Vec<String>) -> Result<(), PoolError> {
    let mut guard = lock(checkout);
    guard
        .as_mut()
        .ok_or(PoolError::Finished)?
        .install_dependencies(requirements)
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

/// Synchronous drive loop: protocol turns run with the GIL released;
/// callbacks run between turns with the GIL held.
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
    let mut event = {
        let (result, print_err) = py.detach(|| {
            run_turn_blocking(&checkout, &print_target, |c, p| {
                c.feed(&code, inputs, mounts, skip_type_check, p)
            })
        });
        finalize_turn(py, result, print_err)?
    };

    loop {
        // `Complete` ends the loop; on any other event a failure to compute the
        // answer discards the checkout (see `sync_turn_answer`).
        let resume_with = match event {
            TurnEvent::Complete(value) => return monty_to_py(py, &value, &dc_registry),
            event => match sync_turn_answer(py, event, &lookup, os.as_ref(), &dc_registry) {
                Ok(answer) => answer,
                Err(err) => {
                    py.detach(|| discard_checkout(&checkout));
                    return Err(err);
                }
            },
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

/// Computes the resume answer for a (non-`Complete`) sync suspension. Split out
/// of [`drive_sync`]'s loop so a failure here — e.g. converting an
/// `external_lookup` value for a `NameLookup` — can discard the suspended worker
/// instead of returning while it waits forever for a resume the aborted feed
/// will never send.
fn sync_turn_answer(
    py: Python<'_>,
    event: TurnEvent,
    lookup: &ExternalLookup<'_, '_>,
    os: Option<&Py<PyAny>>,
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
                os,
                dc_registry,
            );
            Ok(TurnAnswer::Call(ext_to_resume(result)?))
        }
        TurnEvent::NameLookup { name } => Ok(TurnAnswer::Name(lookup.resolve_name(&name)?)),
        TurnEvent::ResolveFutures { .. } => Err(PyRuntimeError::new_err("async external functions require AsyncMonty")),
        TurnEvent::Complete(_) => unreachable!("Complete is handled by the drive loop"),
    }
}

/// Async drive loop: protocol turns run in `spawn_blocking`; coroutine
/// external functions are spawned as tasks and resolved via
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

    let mut event = run_turn_async(&checkout, &print_target, move |c, p| {
        c.feed(&code, inputs, mounts, skip_type_check, p)
    })
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
                        discard_checkout_async(&checkout).await;
                        return Err(err);
                    }
                };
                event = run_turn_async(&checkout, &print_target, move |c, p| c.resume_futures(results, p)).await?;
                continue;
            }
            event => match async_turn_answer(
                event,
                external_lookup.as_ref(),
                os.as_ref(),
                &dc_registry,
                &mut join_set,
            ) {
                Ok(answer) => answer,
                Err(err) => {
                    discard_checkout_async(&checkout).await;
                    return Err(err);
                }
            },
        };
        event = run_turn_async(&checkout, &print_target, move |c, p| match answer {
            TurnAnswer::Call(value) => c.resume(value, p),
            TurnAnswer::Name(value) => c.resume_name_lookup(value, p),
        })
        .await?;
    }
}

/// Async counterpart of [`sync_turn_answer`] (minus `ResolveFutures`, which
/// must await in [`drive_async`]'s loop): a failure here lets the loop discard
/// the suspended worker instead of leaving it waiting forever for a resume.
fn async_turn_answer(
    event: TurnEvent,
    external_lookup: Option<&Py<PyDict>>,
    os: Option<&Py<PyAny>>,
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
                    os,
                    dc_registry,
                )
            });
            Ok(TurnAnswer::Call(ext_to_resume(result)?))
        }
        TurnEvent::NameLookup { name } => {
            let value = Python::attach(|py| {
                ExternalLookup::new(py, external_lookup.map(|d| d.bind(py)), dc_registry).resolve_name(&name)
            })?;
            Ok(TurnAnswer::Name(value))
        }
        TurnEvent::Complete(_) | TurnEvent::ResolveFutures { .. } => {
            unreachable!("Complete and ResolveFutures are handled by the drive loop")
        }
    }
}

/// Best-effort discard of a suspended checkout from an async drive-loop error
/// path. The caller returns the original error, so a `spawn_blocking` join
/// failure here is deliberately ignored.
pub(crate) async fn discard_checkout_async(checkout: &SharedCheckout) {
    let checkout = Arc::clone(checkout);
    let _ = spawn_blocking(move || discard_checkout(&checkout)).await;
}

/// The caller's answer to a suspension, paired with which resume call
/// delivers it.
enum TurnAnswer {
    Call(ResumeValue),
    Name(Option<MontyObject>),
}

/// Runs one protocol turn against the (locked) checkout, streaming prints to
/// `print_target` and capturing the first print-callback failure.
pub(crate) fn run_turn_blocking(
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
pub(crate) async fn run_turn_async(
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
pub(crate) fn finalize_turn(
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
/// or when it returns `NOT_HANDLED` — answers with the child-provided
/// `not_handled_error`, preserving monty's per-call no-handler semantics.
pub(crate) fn dispatch_os_parts(
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
pub(crate) fn pool_err_to_py(py: Python<'_>, err: PoolError) -> PyErr {
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
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
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
