//! Async dispatch loop for driving Monty execution with async external functions.
//!
//! This module provides async versions of the dispatch loops in `monty_cls.rs`
//! and `repl.rs`. Instead of rejecting `ResolveFutures` snapshots, it manages
//! async external function calls by spawning them as tokio tasks and awaiting
//! their results.
//!
//! VM resume calls are offloaded to `spawn_blocking()` to avoid
//! blocking the Python event loop.

use monty::{
    ExcType, ExtFunctionResult, MontyException, MontyObject, MontyRepl, NameLookupResult, OsFunction, PrintWriter,
    ReplProgress, ReplStartError, ResourceTracker, RunProgress,
};
use pyo3::{
    exceptions::PyRuntimeError,
    prelude::*,
    types::{PyDict, PyTuple},
};
use tokio::task::{JoinError, JoinSet, spawn_blocking};

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

/// Resumes a snapshot in a blocking thread via `spawn_blocking`.
///
/// Moves the snapshot and its resume input into a blocking task, creates
/// a `PrintWriter` from the optional callback, and calls `resume()`.
/// Returns the raw result — callers handle error mapping (which differs
/// between Run and REPL paths).
macro_rules! spawn_resume {
    ($snapshot:expr, $input:expr, $print_cb:expr) => {
        spawn_blocking(move || with_print_writer($print_cb, |writer| $snapshot.resume($input, writer)))
            .await
            .map_err(join_error_to_py)?
    };
}

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
    mut progress: RunProgress<T>,
    external_functions: Option<Py<PyDict>>,
    os: Option<Py<PyAny>>,
    dc_registry: DcRegistry,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>> {
    let mut join_set: JoinSet<(u32, ExtFunctionResult)> = JoinSet::new();

    loop {
        match progress {
            RunProgress::Complete(result) => {
                return Python::attach(|py| monty_to_py(py, &result, &dc_registry));
            }
            RunProgress::FunctionCall(call) => {
                let call_result = dispatch_function_call(
                    &call.function_name,
                    call.method_call,
                    &call.args,
                    &call.kwargs,
                    external_functions.as_ref(),
                    &dc_registry,
                );

                match call_result {
                    CallResult::Sync(result) => {
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress = spawn_resume!(call, result, print_cb)
                            .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))?;
                    }
                    CallResult::Coroutine(coro) => {
                        let call_id = call.call_id;
                        spawn_coroutine_task(&mut join_set, call_id, coro, &dc_registry)?;
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress = spawn_resume!(call, ExtFunctionResult::Future(call_id), print_cb)
                            .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))?;
                    }
                }
            }
            RunProgress::OsCall(call) => {
                let result = dispatch_os_call_py(call.function, &call.args, &call.kwargs, os.as_ref(), &dc_registry);
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume!(call, result, print_cb)
                    .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))?;
            }
            RunProgress::NameLookup(lookup) => {
                let result = resolve_name_lookup(&lookup.name, external_functions.as_ref());
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume!(lookup, result, print_cb)
                    .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))?;
            }
            RunProgress::ResolveFutures(state) => {
                let results = wait_for_futures(&mut join_set, state.pending_call_ids()).await?;
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress = spawn_resume!(state, results, print_cb)
                    .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))?;
            }
        }
    }
}

/// Drives the async dispatch loop for a REPL `MontyRepl.feed_run_async()` call.
///
/// Same as `dispatch_loop_run` but works with `ReplProgress` and restores the
/// REPL session when execution completes or on error.
///
/// All error paths must restore the REPL via `restore_repl` or `restore_repl_from_error`
/// before returning, otherwise the REPL session is permanently lost.
pub(crate) async fn dispatch_loop_repl<T: ResourceTracker + Send + 'static>(
    mut progress: ReplProgress<T>,
    repl_owner: Py<PyMontyRepl>,
    external_functions: Option<Py<PyDict>>,
    os: Option<Py<PyAny>>,
    dc_registry: DcRegistry,
    print_callback: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>>
where
    EitherRepl: FromCoreRepl<T>,
{
    let mut join_set: JoinSet<(u32, ExtFunctionResult)> = JoinSet::new();

    loop {
        match progress {
            ReplProgress::Complete { repl, value } => {
                return Python::attach(|py| {
                    let owner = repl_owner.bind(py).get();
                    owner.put_repl(EitherRepl::from_core(repl));
                    monty_to_py(py, &value, &dc_registry)
                });
            }
            ReplProgress::FunctionCall(call) => {
                let call_result = dispatch_function_call(
                    &call.function_name,
                    call.method_call,
                    &call.args,
                    &call.kwargs,
                    external_functions.as_ref(),
                    &dc_registry,
                );

                match call_result {
                    CallResult::Sync(result) => {
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress = spawn_resume!(call, result, print_cb)
                            .map_err(|e| restore_repl_from_error(&repl_owner, *e))?;
                    }
                    CallResult::Coroutine(coro) => {
                        let call_id = call.call_id;
                        if let Err(e) = spawn_coroutine_task(&mut join_set, call_id, coro, &dc_registry) {
                            restore_repl(&repl_owner, call.into_repl());
                            return Err(e);
                        }
                        let print_cb = clone_py_opt(print_callback.as_ref());
                        progress = spawn_resume!(call, ExtFunctionResult::Future(call_id), print_cb)
                            .map_err(|e| restore_repl_from_error(&repl_owner, *e))?;
                    }
                }
            }
            ReplProgress::OsCall(call) => {
                let result = dispatch_os_call_py(call.function, &call.args, &call.kwargs, os.as_ref(), &dc_registry);
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress =
                    spawn_resume!(call, result, print_cb).map_err(|e| restore_repl_from_error(&repl_owner, *e))?;
            }
            ReplProgress::NameLookup(lookup) => {
                let result = resolve_name_lookup(&lookup.name, external_functions.as_ref());
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress =
                    spawn_resume!(lookup, result, print_cb).map_err(|e| restore_repl_from_error(&repl_owner, *e))?;
            }
            ReplProgress::ResolveFutures(state) => {
                let results = match wait_for_futures(&mut join_set, state.pending_call_ids()).await {
                    Ok(r) => r,
                    Err(e) => {
                        restore_repl(&repl_owner, state.into_repl());
                        return Err(e);
                    }
                };
                let print_cb = clone_py_opt(print_callback.as_ref());
                progress =
                    spawn_resume!(state, results, print_cb).map_err(|e| restore_repl_from_error(&repl_owner, *e))?;
            }
        }
    }
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

/// Dispatches a function call to either a dataclass method or an external function,
/// detecting coroutines for async dispatch.
///
/// Acquires the GIL to call the Python function. If the result is a coroutine,
/// returns `CallResult::Coroutine` so the caller can spawn it as a tokio task.
fn dispatch_function_call(
    function_name: &str,
    method_call: bool,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    external_functions: Option<&Py<PyDict>>,
    dc_registry: &DcRegistry,
) -> CallResult {
    Python::attach(|py| {
        if method_call {
            dispatch_method_call_or_coroutine(py, function_name, args, kwargs, dc_registry)
        } else if let Some(ext_fns) = external_functions {
            let ext_fns = ext_fns.bind(py);
            let registry = ExternalFunctionRegistry::new(py, ext_fns, dc_registry);
            registry.call_or_coroutine(function_name, args, kwargs)
        } else {
            CallResult::Sync(ExtFunctionResult::NotFound(function_name.to_owned()))
        }
    })
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
fn join_error_to_py(err: JoinError) -> PyErr {
    PyRuntimeError::new_err(format!("Async task failed: {err}"))
}

/// Clones an optional `Py<PyAny>` by acquiring the GIL.
fn clone_py_opt(opt: Option<&Py<PyAny>>) -> Option<Py<PyAny>> {
    opt.map(|v| Python::attach(|py| v.clone_ref(py)))
}

/// Restores a REPL session into the owner, discarding in-flight execution state.
///
/// Used when an error occurs outside the VM resume path (e.g., coroutine spawn
/// failure, empty JoinSet) where the error is a plain `PyErr` rather than a
/// `ReplStartError` that already contains the REPL.
fn restore_repl<T: ResourceTracker>(repl_owner: &Py<PyMontyRepl>, repl: MontyRepl<T>)
where
    EitherRepl: FromCoreRepl<T>,
{
    Python::attach(|py| {
        let owner = repl_owner.bind(py).get();
        owner.put_repl(EitherRepl::from_core(repl));
    });
}

/// Restores the REPL session from a `ReplStartError` and returns a `PyErr`.
///
/// Used when a VM resume call fails — the `ReplStartError` bundles both
/// the REPL session (for restoration) and the error (for propagation).
fn restore_repl_from_error<T: ResourceTracker>(repl_owner: &Py<PyMontyRepl>, err: ReplStartError<T>) -> PyErr
where
    EitherRepl: FromCoreRepl<T>,
{
    Python::attach(|py| {
        let owner = repl_owner.bind(py).get();
        owner.put_repl(EitherRepl::from_core(err.repl));
        MontyError::new_err(py, err.error)
    })
}
