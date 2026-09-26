//! Async-dispatch helpers shared by the `AsyncMonty` drive loop.
//!
//! Eligible coroutines are awaited at their call suspension. Other coroutines
//! are spawned as tokio tasks and resolved in batches when the sandbox blocks.
//! System sleeps use the same scheduling with tokio timers.

use std::{future::Future, pin::Pin, time::Duration};

use monty_pool::ResumeValue;
use monty_proto::python::InstanceStore;
use monty_types::{CallArgs, ExtFunctionResult, MontyObject, MontyUuid, OsFunctionCall};
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use pyo3_async_runtimes::{into_future_with_locals, tokio::get_current_locals};
use tokio::{
    task::{JoinError, JoinSet},
    time::sleep,
};

use crate::external::{
    CallResult, ExternalLookup, HostNames, dispatch_object_call_or_coroutine, py_err_to_ext_result,
    py_obj_to_ext_result,
};

/// Dispatches a function call to a host-routed method (when `object_id` is
/// set — an instance method, a classmethod, or `__call__` construction) or an
/// external function or import (answered from `names`), returning
/// `CallResult::Coroutine` (for the caller to spawn) when the Python result
/// is a coroutine.
pub(crate) fn dispatch_function_call(
    function_name: &str,
    object_id: Option<MontyUuid>,
    args: &CallArgs,
    names: &HostNames,
    instances: &InstanceStore,
) -> CallResult {
    Python::attach(|py| match object_id {
        Some(object_id) => dispatch_object_call_or_coroutine(py, function_name, &object_id, args, instances),
        None => ExternalLookup::new(py, names, instances).call_or_coroutine(function_name, args),
    })
}

/// How a host coroutine answers a sandbox call, chosen by whether the sandbox
/// code `await`s that call and so expects an awaitable rather than a value.
#[derive(Clone, Copy)]
pub(crate) enum CoroutineMode {
    /// The sandbox awaits the call and has nothing else to run: the host awaits
    /// the coroutine now and answers with `resume_futures`, skipping the
    /// `ResolveFutures` round trip.
    Eager,
    /// The sandbox awaits the call and may run other tasks meanwhile: the
    /// coroutine is spawned and the call answered with a future, resolved later.
    Future,
    /// The sandbox does not await the call (`time.sleep`, `Path.read_text`), so a
    /// future would be an error: the host awaits the coroutine now and answers
    /// with its plain value. Only this session waits.
    AsValue,
}

impl CoroutineMode {
    /// External functions and host methods are always awaited by the sandbox.
    pub(crate) fn for_function_call(allow_eager_await: bool) -> Self {
        if allow_eager_await { Self::Eager } else { Self::Future }
    }

    /// Only an OS call that accepts a future is awaited by the sandbox.
    pub(crate) fn for_os_call(function_name: &str, allow_eager_await: bool) -> Self {
        if allow_eager_await {
            Self::Eager
        } else if OsFunctionCall::accepts_future(function_name) {
            Self::Future
        } else {
            Self::AsValue
        }
    }
}

/// An answer the drive loop can await or spawn.
pub(crate) type AnswerFuture = Pin<Box<dyn Future<Output = ExtFunctionResult> + Send>>;

/// Converts the coroutine a host callback answered `call_id` with, spawning it
/// into `join_set` or handing it back to await outside the callback context.
pub(crate) fn dispatch_coroutine(
    coro: Py<PyAny>,
    call_id: u32,
    mode: CoroutineMode,
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
    instances: &InstanceStore,
) -> PyResult<Dispatched<AnswerFuture>> {
    let future = coroutine_future(coro, instances)?;
    Ok(dispatch_future(Box::pin(future), call_id, mode, join_set))
}

/// Schedules a system sleep like a coroutine answer, allowing gathered sleeps to overlap.
pub(crate) fn dispatch_system_sleep(
    delay: Duration,
    call_id: u32,
    mode: CoroutineMode,
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
) -> Dispatched<AnswerFuture> {
    let wait = async move {
        sleep(delay).await;
        ExtFunctionResult::Return(MontyObject::none())
    };
    dispatch_future(Box::pin(wait), call_id, mode, join_set)
}

/// Spawns deferred answers; returns other futures for awaiting outside the callback context.
fn dispatch_future(
    future: AnswerFuture,
    call_id: u32,
    mode: CoroutineMode,
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
) -> Dispatched<AnswerFuture> {
    match mode {
        CoroutineMode::Eager => Dispatched::Eager(future),
        CoroutineMode::Future => {
            join_set.spawn(async move { (call_id, future.await) });
            Dispatched::Done(ResumeValue::Future)
        }
        CoroutineMode::AsValue => Dispatched::AsValue(future),
    }
}

/// Converts a coroutine under the current asyncio task-locals, for awaiting or spawning.
fn coroutine_future(
    coro: Py<PyAny>,
    instances: &InstanceStore,
) -> PyResult<impl Future<Output = ExtFunctionResult> + Send + use<>> {
    let instances = Python::attach(|py| instances.clone_ref(py));
    let future = python_future(coro)?;
    Ok(async move {
        match future.await {
            Ok(py_result) => Python::attach(|py| {
                let bound = py_result.bind(py);
                py_obj_to_ext_result(bound, &instances)
            }),
            Err(err) => Python::attach(|py| py_err_to_ext_result(py, &err)),
        }
    })
}

/// Schedules `coro` on the caller's event loop, under the task-locals the
/// current `future_into_py` scope established.
fn python_future(coro: Py<PyAny>) -> PyResult<impl Future<Output = PyResult<Py<PyAny>>> + Send + use<>> {
    Python::attach(|py| {
        let locals = get_current_locals(py)?.copy_context(py)?;
        into_future_with_locals(&locals, coro.into_bound(py))
    })
}

/// Outcome of dispatching a call under the callback context: either an answer,
/// or a coroutine future still to be awaited outside the GIL.
pub(crate) enum Dispatched<F> {
    Done(ResumeValue),
    /// Settles into a `resume_futures` answer; see [`CoroutineMode::Eager`].
    Eager(F),
    /// Settles into a plain `resume` answer; see [`CoroutineMode::AsValue`].
    AsValue(F),
}

/// Waits for at least one `JoinSet` task to complete, then drains any other
/// immediately-ready results to batch them into one worker resume. Delivering
/// any completed task is sound: the sandbox resolves futures by `call_id` and
/// re-emits `ResolveFutures` if it still needs a different one.
pub(crate) async fn wait_for_futures(
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
) -> PyResult<Vec<(u32, ExtFunctionResult)>> {
    let mut results = Vec::new();

    // Wait for at least one task to complete
    let first = join_set
        .join_next()
        .await
        .ok_or_else(|| PyRuntimeError::new_err("No pending async tasks but ResolveFutures requested"))?
        .map_err(join_error_to_py)?;
    results.push(first);

    // Drain any other immediately-ready results
    while let Some(result) = join_set.try_join_next() {
        results.push(result.map_err(join_error_to_py)?);
    }

    Ok(results)
}

/// Converts a `tokio::task::JoinError` to a `PyErr`.
#[expect(clippy::needless_pass_by_value)]
pub(crate) fn join_error_to_py(err: JoinError) -> PyErr {
    PyRuntimeError::new_err(format!("Async task failed: {err}"))
}
