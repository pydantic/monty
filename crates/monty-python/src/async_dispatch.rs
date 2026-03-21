//! Async dispatch loop for driving Monty execution with async external functions.
//!
//! This module provides async versions of the dispatch loops in `monty_cls.rs`
//! and `repl.rs`. Instead of rejecting `ResolveFutures` snapshots, it manages
//! async external function calls by spawning them as tokio tasks and awaiting
//! their results.
//!
//! VM resume calls are offloaded to `tokio::task::spawn_blocking()` to avoid
//! blocking the Python event loop.

use ::monty::{
    ExtFunctionResult, MontyException, MontyObject, NameLookupResult, OsFunction, PrintWriter, ReplProgress,
    ResourceTracker, RunProgress,
};
use monty::ExcType;
use pyo3::{
    exceptions::PyRuntimeError,
    prelude::*,
    types::{PyDict, PyTuple},
};
use tokio::task::JoinSet;

use crate::{
    convert::{get_docstring, monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    exceptions::{MontyError, exc_py_to_monty},
    external::{
        CallResult, ExternalFunctionRegistry, dispatch_method_call_or_coroutine, py_err_to_ext_result,
        py_obj_to_ext_result,
    },
    monty_cls::CallbackStringPrint,
    repl::{EitherRepl, FromCoreRepl, PyMontyRepl},
};

/// Drives the async dispatch loop for a non-REPL `Monty.run_async()` call.
///
/// Processes `RunProgress` snapshots in a loop, handling:
/// - `FunctionCall`: calls Python external functions, detecting coroutines for async dispatch
/// - `OsCall`: calls the Python OS handler synchronously
/// - `NameLookup`: resolves names from the external functions dict
/// - `ResolveFutures`: awaits completion of spawned async tasks via `JoinSet`
/// - `Complete`: converts the final `MontyObject` to a Python value and returns
///
/// VM resume calls run in `spawn_blocking` to avoid blocking the event loop.
pub(crate) async fn dispatch_loop_run<T: ResourceTracker + Send + 'static>(
    progress: RunProgress<T>,
    external_functions: Option<Py<PyDict>>,
    os: Option<Py<PyAny>>,
    dc_registry: DcRegistry,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>> {
    let mut progress = progress;
    let mut join_set: JoinSet<(u32, ExtFunctionResult)> = JoinSet::new();

    loop {
        match progress {
            RunProgress::Complete(result) => {
                return Python::attach(|py| monty_to_py(py, &result, &dc_registry));
            }
            RunProgress::FunctionCall(call) => {
                let call_result = Python::attach(|py| {
                    if call.method_call {
                        dispatch_method_call_or_coroutine(
                            py,
                            &call.function_name,
                            &call.args,
                            &call.kwargs,
                            &dc_registry,
                        )
                    } else if let Some(ref ext_fns) = external_functions {
                        let ext_fns = ext_fns.bind(py);
                        let registry = ExternalFunctionRegistry::new(py, ext_fns, &dc_registry);
                        registry.call_or_coroutine(&call.function_name, &call.args, &call.kwargs)
                    } else {
                        CallResult::Sync(ExtFunctionResult::NotFound(call.function_name.clone()))
                    }
                });

                match call_result {
                    CallResult::Sync(result) => {
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress = spawn_resume_fn(call, result, print_cb).await?;
                    }
                    CallResult::Coroutine(coro) => {
                        let call_id = call.call_id;
                        spawn_coroutine_task(&mut join_set, call_id, coro, &dc_registry)?;
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress = spawn_resume_fn(call, ExtFunctionResult::Future(call_id), print_cb).await?;
                    }
                }
            }
            RunProgress::OsCall(call) => {
                let result = dispatch_os_call_py(call.function, &call.args, &call.kwargs, os.as_ref(), &dc_registry);
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume_os(call, result, print_cb).await?;
            }
            RunProgress::NameLookup(lookup) => {
                let result = resolve_name_lookup(&lookup.name, external_functions.as_ref());
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume_lookup(lookup, result, print_cb).await?;
            }
            RunProgress::ResolveFutures(state) => {
                let results = wait_for_futures(&mut join_set, state.pending_call_ids()).await?;
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume_futures(state, results, print_cb).await?;
            }
        }
    }
}

/// Drives the async dispatch loop for a REPL `MontyRepl.run_async()` call.
///
/// Same as `dispatch_loop_run` but works with `ReplProgress` and restores the
/// REPL session when execution completes or on error.
pub(crate) async fn dispatch_loop_repl<T: ResourceTracker + Send + 'static>(
    progress: ReplProgress<T>,
    repl_owner: Py<PyMontyRepl>,
    external_functions: Option<Py<PyDict>>,
    os: Option<Py<PyAny>>,
    dc_registry: DcRegistry,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>>
where
    EitherRepl: FromCoreRepl<T>,
{
    let mut progress = progress;
    let mut join_set: JoinSet<(u32, ExtFunctionResult)> = JoinSet::new();

    loop {
        match progress {
            ReplProgress::Complete { repl, value } => {
                Python::attach(|py| {
                    let owner = repl_owner.bind(py).get();
                    owner.put_repl(EitherRepl::from_core(repl));
                });
                return Python::attach(|py| monty_to_py(py, &value, &dc_registry));
            }
            ReplProgress::FunctionCall(call) => {
                let call_result = Python::attach(|py| {
                    if call.method_call {
                        dispatch_method_call_or_coroutine(
                            py,
                            &call.function_name,
                            &call.args,
                            &call.kwargs,
                            &dc_registry,
                        )
                    } else if let Some(ref ext_fns) = external_functions {
                        let ext_fns = ext_fns.bind(py);
                        let registry = ExternalFunctionRegistry::new(py, ext_fns, &dc_registry);
                        registry.call_or_coroutine(&call.function_name, &call.args, &call.kwargs)
                    } else {
                        CallResult::Sync(ExtFunctionResult::NotFound(call.function_name.clone()))
                    }
                });

                match call_result {
                    CallResult::Sync(result) => {
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress = spawn_resume_repl_fn(call, result, print_cb, &repl_owner).await?;
                    }
                    CallResult::Coroutine(coro) => {
                        let call_id = call.call_id;
                        spawn_coroutine_task(&mut join_set, call_id, coro, &dc_registry)?;
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress =
                            spawn_resume_repl_fn(call, ExtFunctionResult::Future(call_id), print_cb, &repl_owner)
                                .await?;
                    }
                }
            }
            ReplProgress::OsCall(call) => {
                let result = dispatch_os_call_py(call.function, &call.args, &call.kwargs, os.as_ref(), &dc_registry);
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume_repl_os(call, result, print_cb, &repl_owner).await?;
            }
            ReplProgress::NameLookup(lookup) => {
                let result = resolve_name_lookup(&lookup.name, external_functions.as_ref());
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume_repl_lookup(lookup, result, print_cb, &repl_owner).await?;
            }
            ReplProgress::ResolveFutures(state) => {
                let results = wait_for_futures(&mut join_set, state.pending_call_ids()).await?;
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume_repl_futures(state, results, print_cb, &repl_owner).await?;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// spawn_blocking helpers for non-REPL RunProgress
// ---------------------------------------------------------------------------

/// Resumes a `FunctionCall` in a blocking thread with an `ExtFunctionResult`.
async fn spawn_resume_fn<T: ResourceTracker + Send + 'static>(
    call: ::monty::FunctionCall<T>,
    result: impl Into<ExtFunctionResult> + Send + 'static,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<RunProgress<T>> {
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| call.resume(result, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))
}

/// Resumes an `OsCall` in a blocking thread.
async fn spawn_resume_os<T: ResourceTracker + Send + 'static>(
    call: ::monty::OsCall<T>,
    result: impl Into<ExtFunctionResult> + Send + 'static,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<RunProgress<T>> {
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| call.resume(result, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))
}

/// Resumes a `NameLookup` in a blocking thread.
async fn spawn_resume_lookup<T: ResourceTracker + Send + 'static>(
    lookup: ::monty::NameLookup<T>,
    result: NameLookupResult,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<RunProgress<T>> {
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| lookup.resume(result, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))
}

/// Resumes a `ResolveFutures` in a blocking thread with completed task results.
async fn spawn_resume_futures<T: ResourceTracker + Send + 'static>(
    state: ::monty::ResolveFutures<T>,
    results: Vec<(u32, ExtFunctionResult)>,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<RunProgress<T>> {
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| state.resume(results, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))
}

// ---------------------------------------------------------------------------
// spawn_blocking helpers for REPL ReplProgress
// ---------------------------------------------------------------------------

/// Resumes a `ReplFunctionCall` in a blocking thread.
async fn spawn_resume_repl_fn<T: ResourceTracker + Send + 'static>(
    call: ::monty::ReplFunctionCall<T>,
    result: impl Into<ExtFunctionResult> + Send + 'static,
    print_callback: Option<Py<PyAny>>,
    repl_owner: &Py<PyMontyRepl>,
) -> PyResult<ReplProgress<T>>
where
    EitherRepl: FromCoreRepl<T>,
{
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| call.resume(result, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| restore_repl_from_error(repl_owner, *e))
}

/// Resumes a `ReplOsCall` in a blocking thread.
async fn spawn_resume_repl_os<T: ResourceTracker + Send + 'static>(
    call: ::monty::ReplOsCall<T>,
    result: impl Into<ExtFunctionResult> + Send + 'static,
    print_callback: Option<Py<PyAny>>,
    repl_owner: &Py<PyMontyRepl>,
) -> PyResult<ReplProgress<T>>
where
    EitherRepl: FromCoreRepl<T>,
{
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| call.resume(result, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| restore_repl_from_error(repl_owner, *e))
}

/// Resumes a `ReplNameLookup` in a blocking thread.
async fn spawn_resume_repl_lookup<T: ResourceTracker + Send + 'static>(
    lookup: ::monty::ReplNameLookup<T>,
    result: NameLookupResult,
    print_callback: Option<Py<PyAny>>,
    repl_owner: &Py<PyMontyRepl>,
) -> PyResult<ReplProgress<T>>
where
    EitherRepl: FromCoreRepl<T>,
{
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| lookup.resume(result, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| restore_repl_from_error(repl_owner, *e))
}

/// Resumes a `ReplResolveFutures` in a blocking thread.
async fn spawn_resume_repl_futures<T: ResourceTracker + Send + 'static>(
    state: ::monty::ReplResolveFutures<T>,
    results: Vec<(u32, ExtFunctionResult)>,
    print_callback: Option<Py<PyAny>>,
    repl_owner: &Py<PyMontyRepl>,
) -> PyResult<ReplProgress<T>>
where
    EitherRepl: FromCoreRepl<T>,
{
    tokio::task::spawn_blocking(move || with_print_writer(print_callback, |writer| state.resume(results, writer)))
        .await
        .map_err(join_error_to_py)?
        .map_err(|e| restore_repl_from_error(repl_owner, *e))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Creates a `PrintWriter` from an optional print callback and invokes `f` with it.
///
/// If a callback is provided, creates a `CallbackStringPrint` that acquires
/// the GIL internally via `Python::attach()` when the VM calls print.
/// Otherwise uses `PrintWriter::Stdout`.
///
/// Uses a closure pattern because `PrintWriter::Callback` borrows the
/// `CallbackStringPrint`, so the writer can't outlive this function.
fn with_print_writer<R>(print_callback: Option<Py<PyAny>>, f: impl FnOnce(PrintWriter<'_>) -> R) -> R {
    match print_callback {
        Some(cb) => {
            let mut print_cb = CallbackStringPrint::from_py(cb);
            f(PrintWriter::Callback(&mut print_cb))
        }
        None => f(PrintWriter::Stdout),
    }
}

/// Dispatches an OS function call to the Python OS handler.
///
/// Acquires the GIL, converts args/kwargs to Python, calls the handler,
/// and converts the result back to `ExtFunctionResult`.
fn dispatch_os_call_py(
    function: OsFunction,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    os: Option<&Py<PyAny>>,
    dc_registry: &DcRegistry,
) -> ExtFunctionResult {
    Python::attach(|py| {
        let Some(os_callback) = os else {
            return MontyException::new(
                ExcType::NotImplementedError,
                Some(format!("OS function '{function}' not implemented")),
            )
            .into();
        };

        let py_args: Result<Vec<Py<PyAny>>, _> = args.iter().map(|arg| monty_to_py(py, arg, dc_registry)).collect();
        let py_args = match py_args {
            Ok(a) => a,
            Err(err) => return ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
        };
        let py_args_tuple = match PyTuple::new(py, py_args) {
            Ok(t) => t,
            Err(err) => return ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
        };

        let py_kwargs = PyDict::new(py);
        for (k, v) in kwargs {
            let py_key = match monty_to_py(py, k, dc_registry) {
                Ok(k) => k,
                Err(err) => return ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
            };
            let py_value = match monty_to_py(py, v, dc_registry) {
                Ok(v) => v,
                Err(err) => return ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
            };
            if let Err(err) = py_kwargs.set_item(py_key, py_value) {
                return ExtFunctionResult::Error(exc_py_to_monty(py, &err));
            }
        }

        match os_callback
            .bind(py)
            .call1((function.to_string(), py_args_tuple, py_kwargs))
        {
            Ok(result) => match py_to_monty(&result, dc_registry) {
                Ok(obj) => ExtFunctionResult::Return(obj),
                Err(err) => ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
            },
            Err(err) => ExtFunctionResult::Error(exc_py_to_monty(py, &err)),
        }
    })
}

/// Resolves a name lookup against the external functions dict.
///
/// If the name is found, returns `NameLookupResult::Value` with a function object.
/// Otherwise returns `NameLookupResult::Undefined`.
fn resolve_name_lookup(name: &str, external_functions: Option<&Py<PyDict>>) -> NameLookupResult {
    Python::attach(|py| {
        if let Some(ext_fns) = external_functions {
            let ext_fns = ext_fns.bind(py);
            if let Ok(Some(value)) = ext_fns.get_item(name) {
                return NameLookupResult::Value(MontyObject::Function {
                    name: name.to_owned(),
                    docstring: get_docstring(&value),
                });
            }
        }
        NameLookupResult::Undefined
    })
}

/// Spawns a Python coroutine as a tokio task in the `JoinSet`.
///
/// Converts the coroutine to a Rust future via `pyo3_async_runtimes::tokio::into_future()`
/// and spawns it. When the future completes, the result is converted to an
/// `ExtFunctionResult`.
fn spawn_coroutine_task(
    join_set: &mut JoinSet<(u32, ExtFunctionResult)>,
    call_id: u32,
    coro: Py<PyAny>,
    dc_registry: &DcRegistry,
) -> PyResult<()> {
    let dc_registry = Python::attach(|py| dc_registry.clone_ref(py));
    let future = Python::attach(|py| pyo3_async_runtimes::tokio::into_future(coro.into_bound(py)))?;

    join_set.spawn(async move {
        match future.await {
            Ok(py_result) => Python::attach(|py| {
                let bound = py_result.bind(py);
                (call_id, py_obj_to_ext_result(py, bound, &dc_registry))
            }),
            Err(err) => Python::attach(|py| (call_id, py_err_to_ext_result(py, &err))),
        }
    });

    Ok(())
}

/// Waits for at least one async task to complete from the `JoinSet`.
///
/// Collects the first completed result, then drains any other immediately-ready
/// results to batch them together for the VM resume.
async fn wait_for_futures(
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
fn join_error_to_py(err: tokio::task::JoinError) -> PyErr {
    PyRuntimeError::new_err(format!("Async task failed: {err}"))
}

/// Clones an optional `Py<PyAny>` by acquiring the GIL.
fn clone_py_opt(opt: Option<&Py<PyAny>>) -> Option<Py<PyAny>> {
    opt.map(|v| Python::attach(|py| v.clone_ref(py)))
}

/// Restores the REPL session from a `ReplStartError` and returns a `PyErr`.
fn restore_repl_from_error<T: ResourceTracker>(repl_owner: &Py<PyMontyRepl>, err: ::monty::ReplStartError<T>) -> PyErr
where
    EitherRepl: FromCoreRepl<T>,
{
    Python::attach(|py| {
        let owner = repl_owner.bind(py).get();
        owner.put_repl(EitherRepl::from_core(err.repl));
        MontyError::new_err(py, err.error)
    })
}
