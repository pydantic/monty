//! Async-dispatch helpers shared by the `AsyncMonty` drive loop.
//!
//! Eligible coroutines are awaited at their call suspension. Other coroutines
//! are spawned as tokio tasks and resolved in batches when the sandbox blocks.

use std::{future::Future, time::Duration};

use monty_pool::ResumeValue;
use monty_proto::python::InstanceStore;
use monty_types::{CallArgs, ExtFunctionResult, MontyObject, MontyUuid, OsFunctionCall};
use pyo3::{exceptions::PyRuntimeError, prelude::*, types::PyDict};
use pyo3_async_runtimes::{into_future_with_locals, tokio::get_current_locals};
use tokio::task::{JoinError, JoinSet};

use crate::{
    external::{
        CallResult, ExternalLookup, dispatch_object_call_or_coroutine, py_err_to_ext_result, py_obj_to_ext_result,
    },
    get_not_handled,
};

/// Dispatches a function call to a host-routed method (when `object_id` is
/// set — an instance method, a classmethod, or `__call__` construction) or an
/// external function, returning `CallResult::Coroutine` (for the caller to
/// spawn) when the Python result is a coroutine.
pub(crate) fn dispatch_function_call(
    function_name: &str,
    object_id: Option<MontyUuid>,
    args: &CallArgs,
    external_lookup: Option<&Py<PyDict>>,
    instances: &InstanceStore,
) -> CallResult {
    Python::attach(|py| match object_id {
        Some(object_id) => dispatch_object_call_or_coroutine(py, function_name, &object_id, args, instances),
        None => ExternalLookup::new(py, external_lookup.map(|d| d.bind(py)), instances)
            .call_or_coroutine(function_name, args),
    })
}

/// Spawns a Python coroutine as a tokio task in the `JoinSet`, converting its
/// eventual result to an `ExtFunctionResult`.
pub(crate) fn spawn_coroutine_task(
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
    call_id: u32,
    coro: Py<PyAny>,
    instances: &InstanceStore,
) -> PyResult<()> {
    let future = coroutine_future(coro, instances)?;
    join_set.spawn(async move { (call_id, future.await) });
    Ok(())
}

/// [`spawn_coroutine_task`] for a coroutine answering `asyncio.sleep`; see
/// [`sleep_future`].
pub(crate) fn spawn_sleep_task(
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
    call_id: u32,
    coro: Py<PyAny>,
) -> PyResult<()> {
    let future = sleep_future(coro)?;
    join_set.spawn(async move { (call_id, future.await) });
    Ok(())
}

/// Converts a coroutine under the current asyncio task-locals, for eager await or spawning.
pub(crate) fn coroutine_future(
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

/// Like [`coroutine_future`] for a coroutine answering `asyncio.sleep`, whose
/// value the sandbox ignores: it settles to `None` however the coroutine
/// returns, so a value with no wire form cannot fail it. An exception still
/// reaches the `await`, and so does a `NOT_HANDLED` refusal, as the same
/// error a declining synchronous handler produces.
pub(crate) fn sleep_future(coro: Py<PyAny>) -> PyResult<impl Future<Output = ExtFunctionResult> + Send + use<>> {
    let future = python_future(coro)?;
    Ok(async move {
        match future.await {
            Ok(value) => Python::attach(|py| match get_not_handled(py) {
                Ok(not_handled) if value.is(not_handled) => {
                    ExtFunctionResult::Error(OsFunctionCall::AsyncSleep(Duration::ZERO).on_no_handler())
                }
                Ok(_) => ExtFunctionResult::Return(MontyObject::none()),
                Err(err) => py_err_to_ext_result(py, &err),
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

/// Outcome of dispatching a function call under the callback context: either
/// an answer, or an eager coroutine future still to be awaited outside the GIL.
pub(crate) enum Dispatched<F> {
    Done(ResumeValue),
    Eager(F),
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
