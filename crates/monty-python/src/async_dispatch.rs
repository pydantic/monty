//! Async-dispatch helpers shared by the `AsyncMonty` drive loop.
//!
//! Eligible coroutines are awaited at their call suspension. Other coroutines
//! are collected and resolved in batches when the sandbox blocks.
//! System sleeps use the same scheduling with tokio timers.

use std::{
    future::{Future, ready},
    pin::Pin,
    time::Duration,
};

use futures_util::{FutureExt, StreamExt, future::try_join_all, stream::FuturesUnordered};
use monty_pool::ResumeValue;
use monty_proto::python::InstanceStore;
use monty_types::{ExtFunctionResult, MontyObject, OsFunctionCall};
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use pyo3_async_runtimes::{TaskLocals, into_future_with_locals};
use tokio::time::sleep;

use crate::external::{AfterModule, ExternalLookup, HostNames, Staged, py_err_to_ext_result, py_obj_to_ext_result};

/// How a host coroutine answers a sandbox call, chosen by whether the sandbox
/// code `await`s that call and so expects an awaitable rather than a value.
#[derive(Clone, Copy)]
pub(crate) enum CoroutineMode {
    /// The sandbox awaits the call and has nothing else to run: the host awaits
    /// the coroutine now and answers with `resume_futures`, skipping the
    /// `ResolveFutures` round trip.
    Eager,
    /// The sandbox awaits the call and may run other tasks meanwhile: the
    /// coroutine is scheduled and the call answered with a future, resolved later.
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

/// An answer the drive loop can await directly or collect with other pending calls.
pub(crate) type AnswerFuture = Pin<Box<dyn Future<Output = ExtFunctionResult> + Send>>;

/// Concurrent host answers polled by the driver, so Python conversion stays on its thread.
pub(crate) type PendingCalls = FuturesUnordered<Pin<Box<dyn Future<Output = (u32, ExtFunctionResult)> + Send>>>;

/// A module factory's awaitable, scheduled on the caller's event loop.
type ModuleFuture = Pin<Box<dyn Future<Output = PyResult<Py<PyAny>>> + Send>>;

/// Schedules a host coroutine and either collects its answer or awaits it at the call.
pub(crate) fn dispatch_coroutine(
    coro: Py<PyAny>,
    call_id: u32,
    mode: CoroutineMode,
    join_set: &mut PendingCalls,
    instances: &InstanceStore,
) -> PyResult<Dispatched<AnswerFuture>> {
    let future = coroutine_future(coro, instances)?;
    Ok(dispatch_future(Box::pin(future), call_id, mode, join_set))
}

/// Converts the coroutine a host callback answered with into its result
/// future; it is already scheduled on the event loop, so it runs unpolled.
pub(crate) fn coroutine_answer(coro: Py<PyAny>, instances: &InstanceStore) -> PyResult<AnswerFuture> {
    Ok(Box::pin(coroutine_future(coro, instances)?))
}

/// Schedules module factories, returning them for the caller to settle.
pub(crate) fn pending_module(names: Vec<String>, coros: Vec<Py<PyAny>>, then: AfterModule) -> PyResult<PendingModule> {
    let futures = coros
        .into_iter()
        .map(|coro| python_future(coro).map(|future| Box::pin(future) as ModuleFuture))
        .collect::<PyResult<Vec<_>>>()?;
    Ok(PendingModule { names, then, futures })
}

/// Pending factories, one per module. Await [`settle`](Self::settle) outside the GIL,
/// then call [`SettledModule::dispatch`] inside the callback context to install
/// modules and invoke any waiting host function.
pub(crate) struct PendingModule {
    names: Vec<String>,
    then: AfterModule,
    futures: Vec<ModuleFuture>,
}

impl PendingModule {
    /// Awaits every factory, concurrently; the first failure is the result.
    pub(crate) async fn settle(self) -> SettledModule {
        SettledModule {
            names: self.names,
            then: self.then,
            result: try_join_all(self.futures).await,
        }
    }
}

/// Settled module factories, ready to install their modules and answer.
pub(crate) struct SettledModule {
    names: Vec<String>,
    then: AfterModule,
    result: PyResult<Vec<Py<PyAny>>>,
}

impl SettledModule {
    /// Installs modules and answers with a value, or schedules the called function's
    /// coroutine according to `mode`.
    pub(crate) fn dispatch(
        self,
        py: Python<'_>,
        call_id: u32,
        mode: CoroutineMode,
        join_set: &mut PendingCalls,
        names: &HostNames,
        instances: &InstanceStore,
    ) -> PyResult<Dispatched<AnswerFuture>> {
        let modules = self
            .result
            .map(|modules| modules.into_iter().map(|module| module.into_bound(py)).collect());
        match ExternalLookup::new(py, names, instances).finish_after_module(&self.names, modules, &self.then) {
            Staged::Done(result) => Ok(Dispatched::AsValue(Box::pin(ready(result)))),
            Staged::Coroutine(coro) => dispatch_coroutine(coro, call_id, mode, join_set, instances),
        }
    }

    /// Installs modules and returns the waiting call's result future, for a
    /// streamed call, which nothing awaits at the call.
    pub(crate) fn into_answer(
        self,
        py: Python<'_>,
        names: &HostNames,
        instances: &InstanceStore,
    ) -> PyResult<AnswerFuture> {
        let modules = self
            .result
            .map(|modules| modules.into_iter().map(|module| module.into_bound(py)).collect());
        match ExternalLookup::new(py, names, instances).finish_after_module(&self.names, modules, &self.then) {
            // a future settles with a value or an error, so a missing function raises here
            Staged::Done(ExtFunctionResult::NotFound(name)) => Ok(Box::pin(ready(ExtFunctionResult::Error(
                ExtFunctionResult::not_found_exception(&name),
            )))),
            Staged::Done(result) => Ok(Box::pin(ready(result))),
            Staged::Coroutine(coro) => coroutine_answer(coro, instances),
        }
    }
}

/// Schedules a system sleep like a coroutine answer, allowing gathered sleeps to overlap.
pub(crate) fn dispatch_system_sleep(
    delay: Duration,
    call_id: u32,
    mode: CoroutineMode,
    join_set: &mut PendingCalls,
) -> Dispatched<AnswerFuture> {
    let timer = sleep(delay);
    let wait = async move {
        timer.await;
        ExtFunctionResult::Return(MontyObject::none())
    };
    dispatch_future(Box::pin(wait), call_id, mode, join_set)
}

/// Collects deferred answers; returns eager answers for awaiting outside the span guard.
fn dispatch_future(
    future: AnswerFuture,
    call_id: u32,
    mode: CoroutineMode,
    join_set: &mut PendingCalls,
) -> Dispatched<AnswerFuture> {
    match mode {
        CoroutineMode::Eager => Dispatched::Eager(future),
        CoroutineMode::Future => {
            join_set.push(Box::pin(async move { (call_id, future.await) }));
            Dispatched::Done(ResumeValue::Future)
        }
        CoroutineMode::AsValue => Dispatched::AsValue(future),
    }
}

/// Converts a scheduled host coroutine's return value on the driver's thread.
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

/// Schedules `coro` on the running loop with a copy of the current Python context.
pub(crate) fn python_future(coro: Py<PyAny>) -> PyResult<impl Future<Output = PyResult<Py<PyAny>>> + Send + use<>> {
    Python::attach(|py| {
        let locals = TaskLocals::with_running_loop(py)?.copy_context(py)?;
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
    /// A module factory still to await before the answer can be made; the
    /// caller settles it and dispatches again.
    Module(PendingModule),
}

/// Waits for one host answer, then batches every other immediately-ready answer.
/// The sandbox resolves by `call_id` and asks again if it still needs a different call.
pub(crate) async fn wait_for_futures(join_set: &mut PendingCalls) -> PyResult<Vec<(u32, ExtFunctionResult)>> {
    let first = join_set
        .next()
        .await
        .ok_or_else(|| PyRuntimeError::new_err("No pending async tasks but ResolveFutures requested"))?;
    let mut results = vec![first];
    while let Some(Some(result)) = join_set.next().now_or_never() {
        results.push(result);
    }
    Ok(results)
}
