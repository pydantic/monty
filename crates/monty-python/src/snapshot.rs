//! Suspendable `feed_start` execution surface for the subprocess pool.
//!
//! `feed_run` drives a snippet to completion, answering every external call
//! from host callbacks before it returns. `feed_start` instead hands control
//! back to the caller at each suspension as a *snapshot* object — a
//! [`PyFunctionSnapshot`] for an external/OS call, a [`PyNameLookupSnapshot`]
//! for an undefined name, or a [`PyFutureSnapshot`] when every sandbox task is
//! blocked on external futures — so the caller can inspect the call, snapshot
//! the worker with [`PyFunctionSnapshot::dump`] (etc.), and resume when ready.
//! Completion yields a [`MontyComplete`].
//!
//! This reinstates the pre-subprocess `feed_start` API shape, mapped onto the
//! `monty-pool` [`Checkout`] turn primitives. The execution state lives in the
//! worker, so a snapshot is a *cursor* on a live suspended session rather than
//! owned, freely-copyable state: only one suspension is live per session, each
//! snapshot resumes at most once, and `resume` advances that worker forward.
//!
//! Every suspension surfaces, OS calls included — `feed_start` never answers
//! one for you, so a mounted file read comes back as a `FunctionSnapshot` with
//! `is_os_function` set. The captured `external_lookup=` / `os=` and the feed's
//! mounts are consulted only by `resume_auto`, which offers an OS call to the
//! mounts first and falls back to `os=`. Mounts are fixed for the whole feed
//! (passed to `feed_start`), so `resume` takes no `mount=`. Restoring a
//! suspended feed with `load_snapshot` re-establishes those mounts — the caller
//! re-supplies them, as they are never part of the dump.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ::monty::{ExtFunctionResult, MontyException, MontyObject};
use monty_pool::{Checkout, OnPrint, PoolError, ResumeValue, TurnEvent};
use monty_proto::python::{DcRegistry, exc_py_to_monty, monty_to_py, py_to_monty_value};
use pyo3::{
    Borrowed,
    exceptions::{PyBaseException, PyRuntimeError, PyTypeError},
    intern,
    prelude::*,
    types::{PyBytes, PyDict, PyTuple},
};
use pyo3_async_runtimes::tokio::future_into_py;
use tokio::{sync::Mutex, task::JoinSet};

use crate::{
    async_dispatch::{dispatch_function_call, spawn_coroutine_task, wait_for_futures},
    exceptions::MontyError,
    external::{CallResult, ExternalLookup},
    pool::{
        FeedArgs, SharedCheckout, discard_checkout, discard_checkout_async, dispatch_os_parts, ext_to_resume,
        finalize_turn, lock, pool_err_to_py, run_turn_async, run_turn_blocking,
    },
    print_target::PrintTarget,
};

/// Shared context threaded across a `feed_start` drive so each `resume` can
/// keep dispatching against the same worker, conversion registry, and print
/// sink. Cloning bumps the shared handles (the checkout `Arc`, the dataclass
/// registry dict, the print collector buffer) — every clone drives the **same**
/// underlying session.
///
/// It also carries the `external_lookup=` / `os=` captured at
/// `feed_start` / `load_snapshot` and a session-persistent pool of pending
/// coroutine externals: these back [`resume_auto`](PyFunctionSnapshot::resume_auto),
/// which answers each suspension automatically instead of surfacing it to the
/// caller. Plain `resume(...)` takes an explicit answer, so these fields only
/// matter to `resume_auto`.
pub(crate) struct DriveContext {
    checkout: SharedCheckout,
    dc_registry: DcRegistry,
    print_target: PrintTarget,
    script_name: String,
    /// `external_lookup=` captured at `feed_start` / `load_snapshot`; consulted
    /// only by `resume_auto` (plain `resume` never looks names up here).
    external_lookup: Option<Py<PyDict>>,
    /// `os=` captured at `feed_start` / `load_snapshot`; consulted only by
    /// `resume_auto`, and only for OS calls this feed's mounts don't cover.
    os: Option<Py<PyAny>>,
    /// Pending coroutine externals spawned by async `resume_auto`, keyed by
    /// `call_id`. `Arc` so every `clone_ref`'d snapshot of one session shares a
    /// single `JoinSet`; the `tokio` `Mutex` because `wait_for_futures` holds it
    /// across `.await`. Unused (but harmlessly present) on sync sessions, where
    /// a coroutine external is a hard error.
    pending_futures: Arc<Mutex<JoinSet<(u32, ExtFunctionResult)>>>,
}

impl DriveContext {
    pub(crate) fn new(
        checkout: SharedCheckout,
        dc_registry: DcRegistry,
        print_target: PrintTarget,
        script_name: String,
        external_lookup: Option<Py<PyDict>>,
        os: Option<Py<PyAny>>,
    ) -> Self {
        Self {
            checkout,
            dc_registry,
            print_target,
            script_name,
            external_lookup,
            os,
            pending_futures: Arc::new(Mutex::new(JoinSet::new())),
        }
    }

    fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            checkout: SharedCheckout::clone(&self.checkout),
            dc_registry: self.dc_registry.clone_ref(py),
            print_target: self.print_target.clone_handle(py),
            script_name: self.script_name.clone(),
            external_lookup: self.external_lookup.as_ref().map(|d| d.clone_ref(py)),
            os: self.os.as_ref().map(|o| o.clone_ref(py)),
            pending_futures: Arc::clone(&self.pending_futures),
        }
    }
}

// =============================================================================
// feed_start entry points (called from the session pymethods)
// =============================================================================

/// Runs the first feed turn synchronously and returns the resulting snapshot
/// (or [`MontyComplete`]). `external_lookup` / `os` are stored on the
/// [`DriveContext`] for later `resume_auto` calls; every suspension the feed
/// reaches — OS calls included — surfaces as a snapshot.
pub(crate) fn feed_start_sync(
    py: Python<'_>,
    args: FeedArgs,
    external_lookup: Option<Py<PyDict>>,
    script_name: String,
) -> PyResult<Py<PyAny>> {
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
    let ctx = DriveContext::new(checkout, dc_registry, print_target, script_name, external_lookup, os);
    drive_sync(py, ctx, move |c, p| c.feed(&code, inputs, mounts, skip_type_check, p))
}

/// Async counterpart of [`feed_start_sync`]: the returned coroutine runs the
/// first feed turn off the event loop and resolves to the snapshot (or
/// [`MontyComplete`]).
pub(crate) fn feed_start_async(
    py: Python<'_>,
    args: FeedArgs,
    external_lookup: Option<Py<PyDict>>,
    script_name: String,
) -> PyResult<Bound<'_, PyAny>> {
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
    let ctx = DriveContext::new(checkout, dc_registry, print_target, script_name, external_lookup, os);
    future_into_py(py, async move {
        drive_async(ctx, move |c, p| c.feed(&code, inputs, mounts, skip_type_check, p)).await
    })
}

// =============================================================================
// Drive loops: run one turn, then build the snapshot for whatever it reached
// =============================================================================

/// Runs `initial` (a feed or resume turn) with the GIL released, then builds
/// the Python object for whatever event it reaches.
///
/// Every suspension — OS calls included — surfaces to the caller. Nothing is
/// answered automatically here; `resume_auto` is where mounts and the captured
/// `os=` come in.
fn drive_sync(
    py: Python<'_>,
    ctx: DriveContext,
    initial: impl FnOnce(&mut Checkout, OnPrint<'_>) -> Result<TurnEvent, PoolError> + Send,
) -> PyResult<Py<PyAny>> {
    let (result, print_err) = py.detach(|| run_turn_blocking(&ctx.checkout, &ctx.print_target, initial));
    let event = finalize_turn(py, result, print_err)?;
    build_snapshot(py, ctx, event, false)
}

/// Async counterpart of [`drive_sync`]: the worker turn runs via
/// `spawn_blocking`.
async fn drive_async(
    ctx: DriveContext,
    initial: impl FnOnce(&mut Checkout, OnPrint<'_>) -> Result<TurnEvent, PoolError> + Send + 'static,
) -> PyResult<Py<PyAny>> {
    let event = run_turn_async(&ctx.checkout, &ctx.print_target, initial).await?;
    Python::attach(|py| build_snapshot(py, ctx, event, true))
}

/// Answers a suspended OS call from the feed's mounts, returning the next
/// event when one covered it and `None` when the caller must answer it itself.
fn try_mounts_sync(py: Python<'_>, ctx: &DriveContext) -> PyResult<Option<TurnEvent>> {
    let (result, print_err) =
        py.detach(|| run_turn_blocking(&ctx.checkout, &ctx.print_target, Checkout::resume_from_mounts));
    finalize_turn(py, result, print_err)
}

/// Builds the Python object for a caller-visible turn event: a snapshot for a
/// suspension or [`MontyComplete`] for completion. `is_async` selects the sync
/// or async snapshot classes (the latter expose awaitable `resume`).
pub(crate) fn build_snapshot(
    py: Python<'_>,
    ctx: DriveContext,
    event: TurnEvent,
    is_async: bool,
) -> PyResult<Py<PyAny>> {
    match event {
        TurnEvent::Complete(value) => Py::new(
            py,
            MontyComplete {
                value,
                dc_registry: ctx.dc_registry,
            },
        )
        .map(Py::into_any),
        TurnEvent::FunctionCall {
            function_name,
            args,
            kwargs,
            call_id,
            method_call,
        } => {
            let call = FunctionCallData {
                function_name,
                args,
                kwargs,
                call_id,
                is_os_function: false,
                is_method_call: method_call,
            };
            function_snapshot_py(py, ctx, call, is_async)
        }
        TurnEvent::OsCall {
            function_name,
            args,
            kwargs,
            call_id,
        } => {
            let call = FunctionCallData {
                function_name,
                args,
                kwargs,
                call_id,
                is_os_function: true,
                is_method_call: false,
            };
            function_snapshot_py(py, ctx, call, is_async)
        }
        TurnEvent::NameLookup { name } => {
            let snapshot = SnapshotState::new(ctx);
            if is_async {
                Py::new(py, PyAsyncNameLookupSnapshot(NameLookupSnapshot { snapshot, name })).map(Py::into_any)
            } else {
                Py::new(py, PyNameLookupSnapshot(NameLookupSnapshot { snapshot, name })).map(Py::into_any)
            }
        }
        TurnEvent::ResolveFutures { pending_call_ids } => {
            let snapshot = SnapshotState::new(ctx);
            if is_async {
                Py::new(
                    py,
                    PyAsyncFutureSnapshot(FutureSnapshot {
                        snapshot,
                        pending_call_ids,
                    }),
                )
                .map(Py::into_any)
            } else {
                Py::new(
                    py,
                    PyFutureSnapshot(FutureSnapshot {
                        snapshot,
                        pending_call_ids,
                    }),
                )
                .map(Py::into_any)
            }
        }
    }
}

fn function_snapshot_py(
    py: Python<'_>,
    ctx: DriveContext,
    call: FunctionCallData,
    is_async: bool,
) -> PyResult<Py<PyAny>> {
    let snapshot = SnapshotState::new(ctx);
    if is_async {
        Py::new(py, PyAsyncFunctionSnapshot(FunctionSnapshot { snapshot, call })).map(Py::into_any)
    } else {
        Py::new(py, PyFunctionSnapshot(FunctionSnapshot { snapshot, call })).map(Py::into_any)
    }
}

// =============================================================================
// Shared snapshot state and resume plumbing
// =============================================================================

/// The live-cursor state every snapshot carries: the drive context plus a
/// single-use latch enforcing "resume at most once".
struct SnapshotState {
    ctx: DriveContext,
    resumed: AtomicBool,
}

impl SnapshotState {
    fn new(ctx: DriveContext) -> Self {
        Self {
            ctx,
            resumed: AtomicBool::new(false),
        }
    }

    /// Claims the single resume for this snapshot, returning a fresh
    /// [`DriveContext`] for the continuation. Errors if already resumed.
    fn claim(&self, py: Python<'_>) -> PyResult<DriveContext> {
        if self.resumed.swap(true, Ordering::SeqCst) {
            Err(PyRuntimeError::new_err("snapshot has already been resumed"))
        } else {
            Ok(self.ctx.clone_ref(py))
        }
    }

    fn dump(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        // Check resumed only under the checkout lock
        let checkout = SharedCheckout::clone(&self.ctx.checkout);
        let resumed = &self.resumed;
        let state = py
            .detach(|| {
                let mut guard = lock(&checkout);
                if resumed.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                guard
                    .as_mut()
                    .ok_or(PoolError::Finished)
                    .and_then(Checkout::dump)
                    .map(Some)
            })
            .map_err(|e| pool_err_to_py(py, e))?;
        match state {
            Some(state) => Ok(PyBytes::new(py, &state).unbind()),
            None => Err(PyRuntimeError::new_err(
                "cannot dump a snapshot that has already been resumed",
            )),
        }
    }
}

/// Maps an `ExtFunctionResult` onto a pool `ResumeValue`, preserving the
/// `future` answer (which the sandbox uses to register an external future and
/// keep running other tasks — valid in both sync and async drives).
fn ext_result_to_resume(result: ExtFunctionResult) -> ResumeValue {
    match result {
        ExtFunctionResult::Return(value) => ResumeValue::Return(value),
        ExtFunctionResult::Error(exc) => ResumeValue::Error(exc),
        ExtFunctionResult::Future(_) => ResumeValue::Future,
        ExtFunctionResult::NotFound(name) => {
            // Preserve the name so the worker raises the right NameError; the
            // pool fills it from the pending call when resuming.
            let _ = name;
            ResumeValue::NotFound
        }
    }
}

/// Resolves a name against the [`DriveContext`]'s captured `external_lookup=`,
/// shared by the sync and async name-lookup `resume_auto`. `None` leaves the
/// name undefined so the sandbox raises `NameError`, matching `feed_run`.
fn resolve_captured_name(py: Python<'_>, ctx: &DriveContext, name: &str) -> PyResult<Option<MontyObject>> {
    ExternalLookup::new(py, ctx.external_lookup.as_ref().map(|d| d.bind(py)), &ctx.dc_registry).resolve_name(name)
}

/// Parses an `ExternalResult` TypedDict — one of `{'return_value': obj}`,
/// `{'exception': exc}`, `{'exc_type': str, 'message'?: str}`, or
/// `{'future': ...}` — into a [`ResumeValue`]. `call_id` is unused by the pool
/// (the worker tracks it) but kept for parity with the documented shape.
fn parse_external_result(
    py: Python<'_>,
    result: &Bound<'_, PyDict>,
    dc_registry: &DcRegistry,
) -> PyResult<ResumeValue> {
    const ARGS_ERROR: &str = "ExternalResult must be a dict with one of: 'return_value', 'exception', 'exc_type' (with optional 'message'), or 'future'";

    if let Some(exc_type_val) = result.get_item(intern!(py, "exc_type"))? {
        let message_val = result.get_item(intern!(py, "message"))?;
        let expected_len = if message_val.is_some() { 2 } else { 1 };
        if result.len() != expected_len {
            return Err(PyTypeError::new_err(ARGS_ERROR));
        }
        let exc_type_str: String = exc_type_val
            .extract()
            .map_err(|_| PyTypeError::new_err("'exc_type' must be a string"))?;
        let exc_type = exc_type_str
            .parse()
            .map_err(|_| PyTypeError::new_err(format!("Unknown exception type: '{exc_type_str}'")))?;
        let message = message_val
            .map(|m| {
                m.extract::<String>()
                    .map_err(|_| PyTypeError::new_err("'message' must be a string"))
            })
            .transpose()?;
        return Ok(ResumeValue::Error(MontyException::new(exc_type, message)));
    }

    if result.len() != 1 {
        Err(PyTypeError::new_err(ARGS_ERROR))
    } else if let Some(rv) = result.get_item(intern!(py, "return_value"))? {
        let value = py_to_monty_value(&rv, dc_registry).map_err(|e| MontyError::new_err(py, e))?;
        Ok(ResumeValue::Return(value))
    } else if let Some(exc) = result.get_item(intern!(py, "exception"))? {
        if exc.is_instance_of::<PyBaseException>() {
            let py_err = PyErr::from_value(exc);
            Ok(ResumeValue::Error(exc_py_to_monty(py, &py_err)))
        } else {
            Err(PyTypeError::new_err("'exception' must be a BaseException instance"))
        }
    } else if let Some(fut) = result.get_item(intern!(py, "future"))? {
        if fut.is(py.Ellipsis()) {
            Ok(ResumeValue::Future)
        } else {
            Err(PyTypeError::new_err(
                "value for the 'future' key must be Ellipsis (...)",
            ))
        }
    } else {
        Err(PyTypeError::new_err(ARGS_ERROR))
    }
}

fn args_to_py<'py>(py: Python<'py>, args: &[MontyObject], dc_registry: &DcRegistry) -> PyResult<Bound<'py, PyTuple>> {
    let items = args
        .iter()
        .map(|arg| monty_to_py(py, arg, dc_registry))
        .collect::<PyResult<Vec<_>>>()?;
    PyTuple::new(py, items)
}

fn kwargs_to_py<'py>(
    py: Python<'py>,
    kwargs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (key, value) in kwargs {
        dict.set_item(monty_to_py(py, key, dc_registry)?, monty_to_py(py, value, dc_registry)?)?;
    }
    Ok(dict)
}

// =============================================================================
// FunctionSnapshot (external / OS call) — sync and async
// =============================================================================

/// The pending-call payload shared by the sync and async function snapshots.
///
/// `Clone` so async `resume_auto` can move an owned copy into the `'static`
/// dispatch future (the snapshot itself is borrowed for only the pymethod call).
#[derive(Clone)]
struct FunctionCallData {
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
    call_id: u32,
    is_os_function: bool,
    is_method_call: bool,
}

struct FunctionSnapshot {
    snapshot: SnapshotState,
    call: FunctionCallData,
}

impl FunctionSnapshot {
    fn resume_value(&self, py: Python<'_>, result: &Bound<'_, PyDict>) -> PyResult<ResumeValue> {
        parse_external_result(py, result, &self.snapshot.ctx.dc_registry)
    }

    /// `ResumeValue::NotHandled` for an OS-call snapshot: the sandbox raises
    /// the call's own no-handler default.
    fn not_handled_value(&self) -> PyResult<ResumeValue> {
        if self.call.is_os_function {
            Ok(ResumeValue::NotHandled)
        } else {
            Err(PyRuntimeError::new_err(
                "resume_not_handled() is only valid for OS function snapshots",
            ))
        }
    }
}

/// A paused execution waiting for an external function or OS call result.
/// Resume with [`Self::resume`] (or [`Self::resume_not_handled`] for OS calls).
#[pyclass(name = "FunctionSnapshot", module = "pydantic_monty", frozen)]
pub struct PyFunctionSnapshot(FunctionSnapshot);

#[pymethods]
impl PyFunctionSnapshot {
    #[getter]
    fn script_name(&self) -> &str {
        &self.0.snapshot.ctx.script_name
    }

    #[getter]
    fn is_os_function(&self) -> bool {
        self.0.call.is_os_function
    }

    #[getter]
    fn is_method_call(&self) -> bool {
        self.0.call.is_method_call
    }

    #[getter]
    fn function_name(&self) -> &str {
        &self.0.call.function_name
    }

    #[getter]
    fn call_id(&self) -> u32 {
        self.0.call.call_id
    }

    #[getter]
    fn args<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        args_to_py(py, &self.0.call.args, &self.0.snapshot.ctx.dc_registry)
    }

    #[getter]
    fn kwargs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        kwargs_to_py(py, &self.0.call.kwargs, &self.0.snapshot.ctx.dc_registry)
    }

    /// Resumes execution with an `ExternalResult` (return value, exception, or
    /// future), returning the next snapshot or [`MontyComplete`]. Resumes once.
    fn resume(&self, py: Python<'_>, result: &Bound<'_, PyDict>) -> PyResult<Py<PyAny>> {
        let value = self.0.resume_value(py, result)?;
        let ctx = self.0.snapshot.claim(py)?;
        drive_sync(py, ctx, move |c, p| c.resume(value, p))
    }

    /// Resumes an OS-call snapshot with monty's default unhandled-OS behaviour.
    fn resume_not_handled(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let value = self.0.not_handled_value()?;
        let ctx = self.0.snapshot.claim(py)?;
        drive_sync(py, ctx, move |c, p| c.resume(value, p))
    }

    /// Answers this call automatically, then drives to the next snapshot (or
    /// [`MontyComplete`]). An OS call is offered to the feed's mounts first and
    /// falls back to the `os=` captured at `feed_start` / `load_snapshot`; an
    /// external call is resolved through `external_lookup=`, and a name absent
    /// from it resolves to `NotFound` so the sandbox raises `NameError` —
    /// matching `feed_run`. A coroutine external raises `RuntimeError` (async
    /// externals need `AsyncMonty`). Consumes the snapshot.
    fn resume_auto(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let ctx = self.0.snapshot.claim(py)?;
        let call = &self.0.call;
        let value = if call.is_os_function {
            if let Some(event) = try_mounts_sync(py, &ctx)? {
                return build_snapshot(py, ctx, event, false);
            }
            dispatch_os_parts(
                py,
                &call.function_name,
                &call.args,
                &call.kwargs,
                ctx.os.as_ref(),
                &ctx.dc_registry,
            )
        } else {
            match dispatch_function_call(
                &call.function_name,
                call.is_method_call,
                &call.args,
                &call.kwargs,
                ctx.external_lookup.as_ref(),
                &ctx.dc_registry,
            ) {
                CallResult::Sync(result) => ext_result_to_resume(result),
                CallResult::Coroutine(coro) => {
                    // Close the un-awaited coroutine so it doesn't leak a
                    // "coroutine was never awaited" ResourceWarning: a sync
                    // session has no event loop to drive it.
                    let _ = coro.bind(py).call_method0(intern!(py, "close"));
                    py.detach(|| discard_checkout(&ctx.checkout));
                    return Err(PyRuntimeError::new_err("async external functions require AsyncMonty"));
                }
            }
        };
        drive_sync(py, ctx, move |c, p| c.resume(value, p))
    }

    fn dump(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        self.0.snapshot.dump(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "FunctionSnapshot(function_name={:?}, is_os_function={})",
            self.0.call.function_name, self.0.call.is_os_function
        )
    }
}

/// Async sibling of [`PyFunctionSnapshot`]: `resume` / `resume_not_handled`
/// return awaitables driven off the event loop.
#[pyclass(name = "AsyncFunctionSnapshot", module = "pydantic_monty", frozen)]
pub struct PyAsyncFunctionSnapshot(FunctionSnapshot);

#[pymethods]
impl PyAsyncFunctionSnapshot {
    #[getter]
    fn script_name(&self) -> &str {
        &self.0.snapshot.ctx.script_name
    }

    #[getter]
    fn is_os_function(&self) -> bool {
        self.0.call.is_os_function
    }

    #[getter]
    fn is_method_call(&self) -> bool {
        self.0.call.is_method_call
    }

    #[getter]
    fn function_name(&self) -> &str {
        &self.0.call.function_name
    }

    #[getter]
    fn call_id(&self) -> u32 {
        self.0.call.call_id
    }

    #[getter]
    fn args<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        args_to_py(py, &self.0.call.args, &self.0.snapshot.ctx.dc_registry)
    }

    #[getter]
    fn kwargs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        kwargs_to_py(py, &self.0.call.kwargs, &self.0.snapshot.ctx.dc_registry)
    }

    fn resume<'py>(&self, py: Python<'py>, result: &Bound<'_, PyDict>) -> PyResult<Bound<'py, PyAny>> {
        let value = self.0.resume_value(py, result)?;
        let ctx = self.0.snapshot.claim(py)?;
        future_into_py(
            py,
            async move { drive_async(ctx, move |c, p| c.resume(value, p)).await },
        )
    }

    fn resume_not_handled<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let value = self.0.not_handled_value()?;
        let ctx = self.0.snapshot.claim(py)?;
        future_into_py(
            py,
            async move { drive_async(ctx, move |c, p| c.resume(value, p)).await },
        )
    }

    /// Async sibling of [`PyFunctionSnapshot::resume_auto`]. A coroutine external
    /// is spawned into the session's shared future pool and answered with a
    /// pending future — so other sandbox tasks keep running — to be settled
    /// later by an [`PyAsyncFutureSnapshot::resume_auto`].
    fn resume_auto<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let ctx = self.0.snapshot.claim(py)?;
        // owned copy: the snapshot is borrowed only for this synchronous prologue
        let call = self.0.call.clone();
        future_into_py(py, async move {
            // Dispatch inside the future: a coroutine's `into_future` needs the
            // asyncio task-locals that `future_into_py`'s scope establishes.
            let answer: PyResult<ResumeValue> = if call.is_os_function {
                // mounts get first refusal, then the captured `os=`
                match run_turn_async(&ctx.checkout, &ctx.print_target, Checkout::resume_from_mounts).await? {
                    Some(event) => return Python::attach(|py| build_snapshot(py, ctx, event, true)),
                    None => Ok(Python::attach(|py| {
                        dispatch_os_parts(
                            py,
                            &call.function_name,
                            &call.args,
                            &call.kwargs,
                            ctx.os.as_ref(),
                            &ctx.dc_registry,
                        )
                    })),
                }
            } else {
                match dispatch_function_call(
                    &call.function_name,
                    call.is_method_call,
                    &call.args,
                    &call.kwargs,
                    ctx.external_lookup.as_ref(),
                    &ctx.dc_registry,
                ) {
                    CallResult::Sync(result) => Ok(ext_result_to_resume(result)),
                    CallResult::Coroutine(coro) => {
                        let mut join_set = ctx.pending_futures.lock().await;
                        spawn_coroutine_task(&mut join_set, call.call_id, coro, &ctx.dc_registry)
                            .map(|()| ResumeValue::Future)
                    }
                }
            };
            let value = match answer {
                Ok(value) => value,
                Err(err) => {
                    discard_checkout_async(&ctx.checkout).await;
                    return Err(err);
                }
            };
            drive_async(ctx, move |c, p| c.resume(value, p)).await
        })
    }

    fn dump(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        self.0.snapshot.dump(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "AsyncFunctionSnapshot(function_name={:?}, is_os_function={})",
            self.0.call.function_name, self.0.call.is_os_function
        )
    }
}

// =============================================================================
// NameLookupSnapshot — sync and async
// =============================================================================

struct NameLookupSnapshot {
    snapshot: SnapshotState,
    name: String,
}

/// The argument to `NameLookupSnapshot.resume`, distinguishing an omitted value
/// (`Unset` — raise `NameError`) from an explicitly supplied one (`Set`,
/// including `None`).
///
/// A bare `Option<Bound<PyAny>>` cannot express this: PyO3 extracts Python
/// `None` to Rust `None`, collapsing an explicit `None` binding into the
/// "omitted" case. Capturing the object here keeps `None` a real value, while
/// the unit `Unset` default — which needs no `py` token, unlike any Python
/// object — marks omission.
enum MaybeValue<'py> {
    Unset,
    Set(Bound<'py, PyAny>),
}

impl<'a, 'py> FromPyObject<'a, 'py> for MaybeValue<'py> {
    type Error = Infallible;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        Ok(MaybeValue::Set(obj.to_owned()))
    }
}

impl NameLookupSnapshot {
    /// Converts the `resume` argument into the name's binding: an omitted value
    /// (`Unset`) leaves the name undefined so the sandbox raises `NameError`,
    /// while a supplied value — **including `None`** — binds the name to it.
    fn resume_value(&self, py: Python<'_>, value: MaybeValue<'_>) -> PyResult<Option<MontyObject>> {
        match value {
            MaybeValue::Unset => Ok(None),
            MaybeValue::Set(value) => py_to_monty_value(&value, &self.snapshot.ctx.dc_registry)
                .map(Some)
                .map_err(|e| MontyError::new_err(py, e)),
        }
    }
}

/// A paused execution waiting for the value of an undefined name. Resume with a
/// `value` to define it, or with nothing to let the sandbox raise `NameError`.
#[pyclass(name = "NameLookupSnapshot", module = "pydantic_monty", frozen)]
pub struct PyNameLookupSnapshot(NameLookupSnapshot);

#[pymethods]
impl PyNameLookupSnapshot {
    #[getter]
    fn script_name(&self) -> &str {
        &self.0.snapshot.ctx.script_name
    }

    #[getter]
    fn variable_name(&self) -> &str {
        &self.0.name
    }

    #[pyo3(signature = (*, value=MaybeValue::Unset))]
    fn resume(&self, py: Python<'_>, value: MaybeValue<'_>) -> PyResult<Py<PyAny>> {
        let value = self.0.resume_value(py, value)?;
        let ctx = self.0.snapshot.claim(py)?;
        drive_sync(py, ctx, move |c, p| c.resume_name_lookup(value, p))
    }

    /// Answers this name lookup automatically from the captured
    /// `external_lookup=`, then drives to the next snapshot. A name absent from
    /// the lookup leaves it undefined, so the sandbox raises `NameError`.
    /// Consumes the snapshot.
    fn resume_auto(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let ctx = self.0.snapshot.claim(py)?;
        let value = match resolve_captured_name(py, &ctx, &self.0.name) {
            Ok(value) => value,
            Err(err) => {
                py.detach(|| discard_checkout(&ctx.checkout));
                return Err(err);
            }
        };
        drive_sync(py, ctx, move |c, p| c.resume_name_lookup(value, p))
    }

    fn dump(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        self.0.snapshot.dump(py)
    }

    fn __repr__(&self) -> String {
        format!("NameLookupSnapshot(variable_name={:?})", self.0.name)
    }
}

/// Async sibling of [`PyNameLookupSnapshot`].
#[pyclass(name = "AsyncNameLookupSnapshot", module = "pydantic_monty", frozen)]
pub struct PyAsyncNameLookupSnapshot(NameLookupSnapshot);

#[pymethods]
impl PyAsyncNameLookupSnapshot {
    #[getter]
    fn script_name(&self) -> &str {
        &self.0.snapshot.ctx.script_name
    }

    #[getter]
    fn variable_name(&self) -> &str {
        &self.0.name
    }

    #[pyo3(signature = (*, value=MaybeValue::Unset))]
    fn resume<'py>(&self, py: Python<'py>, value: MaybeValue<'_>) -> PyResult<Bound<'py, PyAny>> {
        let value = self.0.resume_value(py, value)?;
        let ctx = self.0.snapshot.claim(py)?;
        future_into_py(py, async move {
            drive_async(ctx, move |c, p| c.resume_name_lookup(value, p)).await
        })
    }

    /// Async sibling of [`PyNameLookupSnapshot::resume_auto`].
    fn resume_auto<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let ctx = self.0.snapshot.claim(py)?;
        let name = self.0.name.clone();
        future_into_py(py, async move {
            let value = match Python::attach(|py| resolve_captured_name(py, &ctx, &name)) {
                Ok(value) => value,
                Err(err) => {
                    discard_checkout_async(&ctx.checkout).await;
                    return Err(err);
                }
            };
            drive_async(ctx, move |c, p| c.resume_name_lookup(value, p)).await
        })
    }

    fn dump(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        self.0.snapshot.dump(py)
    }

    fn __repr__(&self) -> String {
        format!("AsyncNameLookupSnapshot(variable_name={:?})", self.0.name)
    }
}

// =============================================================================
// FutureSnapshot — sync and async
// =============================================================================

struct FutureSnapshot {
    snapshot: SnapshotState,
    pending_call_ids: Vec<u32>,
}

impl FutureSnapshot {
    /// Parses the `{call_id: result}` mapping into `ResumeValue`s, rejecting a
    /// pending `future` answer up front: a future must settle to a return value
    /// or exception, not to another future. Validating here — before `resume`
    /// calls `claim()` — means an invalid resolution fails with a `PyTypeError`
    /// without consuming the (single-use) snapshot or stranding the worker.
    fn resume_values(&self, py: Python<'_>, results: &Bound<'_, PyDict>) -> PyResult<Vec<(u32, ResumeValue)>> {
        let mut resolved = Vec::with_capacity(results.len());
        for (key, value) in results {
            let call_id: u32 = key
                .extract()
                .map_err(|_| PyTypeError::new_err("future result keys must be int call ids"))?;
            let dict = value
                .cast_into::<PyDict>()
                .map_err(|_| PyTypeError::new_err("future result values must be ExternalResult dicts"))?;
            let resume = parse_external_result(py, &dict, &self.snapshot.ctx.dc_registry)?;
            if matches!(resume, ResumeValue::Future) {
                return Err(PyTypeError::new_err(format!(
                    "future {call_id} cannot resolve to another future; provide a return value or exception"
                )));
            }
            resolved.push((call_id, resume));
        }
        Ok(resolved)
    }
}

/// A paused execution where every sandbox task is blocked on external futures.
/// Resume with a `{call_id: ExternalResult}` mapping for one or more futures.
#[pyclass(name = "FutureSnapshot", module = "pydantic_monty", frozen)]
pub struct PyFutureSnapshot(FutureSnapshot);

#[pymethods]
impl PyFutureSnapshot {
    #[getter]
    fn script_name(&self) -> &str {
        &self.0.snapshot.ctx.script_name
    }

    #[getter]
    fn pending_call_ids(&self) -> Vec<u32> {
        self.0.pending_call_ids.clone()
    }

    fn resume(&self, py: Python<'_>, results: &Bound<'_, PyDict>) -> PyResult<Py<PyAny>> {
        let resolved = self.0.resume_values(py, results)?;
        let ctx = self.0.snapshot.claim(py)?;
        drive_sync(py, ctx, move |c, p| c.resume_futures(resolved, p))
    }

    /// Sync sessions have no event loop to drive coroutine externals, so this
    /// always raises; resolve the pending futures manually with
    /// `resume({call_id: ...})`. Does not consume the snapshot (no side effects).
    #[expect(clippy::unused_self, reason = "a pyclass instance method must take &self")]
    fn resume_auto(&self) -> PyResult<Py<PyAny>> {
        Err(PyRuntimeError::new_err("async external functions require AsyncMonty"))
    }

    fn dump(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        self.0.snapshot.dump(py)
    }

    fn __repr__(&self) -> String {
        format!("FutureSnapshot(pending_call_ids={:?})", self.0.pending_call_ids)
    }
}

/// Async sibling of [`PyFutureSnapshot`].
#[pyclass(name = "AsyncFutureSnapshot", module = "pydantic_monty", frozen)]
pub struct PyAsyncFutureSnapshot(FutureSnapshot);

#[pymethods]
impl PyAsyncFutureSnapshot {
    #[getter]
    fn script_name(&self) -> &str {
        &self.0.snapshot.ctx.script_name
    }

    #[getter]
    fn pending_call_ids(&self) -> Vec<u32> {
        self.0.pending_call_ids.clone()
    }

    fn resume<'py>(&self, py: Python<'py>, results: &Bound<'_, PyDict>) -> PyResult<Bound<'py, PyAny>> {
        let resolved = self.0.resume_values(py, results)?;
        let ctx = self.0.snapshot.claim(py)?;
        future_into_py(py, async move {
            drive_async(ctx, move |c, p| c.resume_futures(resolved, p)).await
        })
    }

    /// Waits for one or more of the coroutine externals spawned by earlier
    /// `resume_auto` calls to settle, delivers them, and drives to the next
    /// snapshot. Raises if there are no pending coroutines to await — e.g. on a
    /// snapshot restored via `load_snapshot`, whose spawned coroutines lived in
    /// the previous process (resolve those manually with `resume({...})`).
    fn resume_auto<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let ctx = self.0.snapshot.claim(py)?;
        future_into_py(py, async move {
            let resolved = {
                let mut join_set = ctx.pending_futures.lock().await;
                wait_for_futures(&mut join_set).await
            }
            .and_then(|results| {
                results
                    .into_iter()
                    .map(|(call_id, result)| Ok((call_id, ext_to_resume(result)?)))
                    .collect::<PyResult<Vec<_>>>()
            });
            let results = match resolved {
                Ok(results) => results,
                Err(err) => {
                    discard_checkout_async(&ctx.checkout).await;
                    return Err(err);
                }
            };
            drive_async(ctx, move |c, p| c.resume_futures(results, p)).await
        })
    }

    fn dump(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        self.0.snapshot.dump(py)
    }

    fn __repr__(&self) -> String {
        format!("AsyncFutureSnapshot(pending_call_ids={:?})", self.0.pending_call_ids)
    }
}

// =============================================================================
// MontyComplete — terminal value (shared by sync and async)
// =============================================================================

/// The result of a completed `feed_start` execution. `output` converts the
/// final value from monty's representation to a Python object on each access.
#[pyclass(name = "MontyComplete", module = "pydantic_monty", frozen)]
pub struct MontyComplete {
    value: MontyObject,
    dc_registry: DcRegistry,
}

#[pymethods]
impl MontyComplete {
    #[getter]
    fn output(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        monty_to_py(py, &self.value, &self.dc_registry)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let output = self.output(py)?;
        Ok(format!("MontyComplete(output={})", output.bind(py).repr()?))
    }
}
