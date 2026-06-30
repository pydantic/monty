//! Async-dispatch helpers shared by the `AsyncMonty` drive loop.
//!
//! Coroutine external functions are converted to Rust futures and spawned as
//! tokio tasks; when the sandbox blocks on its external futures
//! (`ResolveFutures`), the completed task results are batched back to the
//! worker.

use ::monty::{ExtFunctionResult, MontyObject};
use pyo3::{exceptions::PyRuntimeError, prelude::*, types::PyDict};
use pyo3_async_runtimes::tokio::into_future;
use tokio::task::{JoinError, JoinSet};

use crate::{
    dataclass::DcRegistry,
    external::{
        CallResult, ExternalLookup, dispatch_method_call_or_coroutine, py_err_to_ext_result, py_obj_to_ext_result,
    },
};

/// Dispatches a function call to a dataclass method or external function,
/// returning `CallResult::Coroutine` (for the caller to spawn) when the Python
/// result is a coroutine.
pub(crate) fn dispatch_function_call(
    function_name: &str,
    method_call: bool,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    external_lookup: Option<&Py<PyDict>>,
    dc_registry: &DcRegistry,
) -> CallResult {
    Python::attach(|py| {
        if method_call {
            dispatch_method_call_or_coroutine(py, function_name, args, kwargs, dc_registry)
        } else {
            ExternalLookup::new(py, external_lookup.map(|d| d.bind(py)), dc_registry).call_or_coroutine(
                function_name,
                args,
                kwargs,
            )
        }
    })
}

/// Spawns a Python coroutine as a tokio task in the `JoinSet`, converting its
/// eventual result to an `ExtFunctionResult`.
pub(crate) fn spawn_coroutine_task(
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
    call_id: u32,
    coro: Py<PyAny>,
    dc_registry: &DcRegistry,
) -> PyResult<()> {
    let dc_registry = Python::attach(|py| dc_registry.clone_ref(py));
    let future = Python::attach(|py| into_future(coro.into_bound(py)))?;

    join_set.spawn(async move {
        match future.await {
            Ok(py_result) => Python::attach(|py| {
                let bound = py_result.bind(py);
                (call_id, py_obj_to_ext_result(bound, &dc_registry))
            }),
            Err(err) => Python::attach(|py| (call_id, py_err_to_ext_result(py, &err))),
        }
    });

    Ok(())
}

/// Waits for at least one `JoinSet` task to complete, then drains any other
/// immediately-ready results to batch them into one worker resume.
pub(crate) async fn wait_for_futures(
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
    _pending_call_ids: &[u32],
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
