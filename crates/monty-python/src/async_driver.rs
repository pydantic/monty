//! Drives Rust protocol futures through PyO3's asyncio coroutine implementation.
//!
//! Python owns execution, callback context and cancellation. Tokio supplies I/O
//! readiness, timers and a cooperative budget when polling outside an existing Tokio runtime.

use std::{
    future::Future,
    pin::{Pin, pin},
    sync::Mutex,
    task::{Context, Poll},
};

use pyo3::{IntoPyObjectExt, exceptions::PyRuntimeError, prelude::*};
use pyo3_async_runtimes::tokio::get_runtime;
use tokio::{
    runtime::{Handle, RuntimeFlavor},
    task::unconstrained,
    task_local,
};

task_local! {
    /// Whether the embedding runtime supports `block_in_place`, even after we switch handles.
    static EMBEDDING_ALLOWS_BLOCKING: bool;
}

/// Checks whether a sync callback may block inside an embedding Tokio runtime.
pub(crate) fn embedding_allows_blocking() -> Option<bool> {
    EMBEDDING_ALLOWS_BLOCKING.try_with(|allowed| *allowed).ok()
}

/// Returns a coroutine which drives `future` in its awaiting task, without a Tokio task hop.
pub(crate) fn future_into_py<T>(
    py: Python<'_>,
    future: impl Future<Output = PyResult<T>> + Send + 'static,
) -> PyResult<Bound<'_, PyAny>>
where
    T: for<'py> IntoPyObject<'py> + Send + 'static,
{
    let future = RuntimeFuture(Some(Box::pin(async move {
        let result = future.await?;
        Python::attach(|py| result.into_py_any(py))
    })));
    Py::new(py, PythonFuture(Mutex::new(Some(future))))?
        .into_bound(py)
        .call_method0("run")
}

/// An owned protocol operation whose coroutine is created by a PyO3 async method.
#[pyclass(module = "pydantic_monty._monty", frozen)]
struct PythonFuture(Mutex<Option<RuntimeFuture>>);

#[pymethods]
impl PythonFuture {
    /// Transfers the operation into its coroutine so cancellation drops it synchronously.
    async fn run(&self) -> PyResult<Py<PyAny>> {
        let future = self
            .0
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("cannot await a running Monty operation"))?
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("Monty operation has already been awaited"))?;
        future.await
    }
}

/// An owned future whose result has already been converted for Python.
type ProtocolFuture = Pin<Box<dyn Future<Output = PyResult<Py<PyAny>>> + Send>>;

/// Detaches during protocol polling and retains Tokio context during cancellation cleanup.
struct RuntimeFuture(Option<ProtocolFuture>);

impl Future for RuntimeFuture {
    type Output = PyResult<Py<PyAny>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let future = self.0.as_mut().expect("protocol future is present until dropped");
        let waker = cx.waker();
        Python::attach(|py| {
            py.detach(|| {
                let runtime = get_runtime();
                let cx = &mut Context::from_waker(waker);
                if let Ok(host) = Handle::try_current() {
                    let allows_blocking = embedding_allows_blocking()
                        .unwrap_or_else(|| host.runtime_flavor() == RuntimeFlavor::MultiThread);
                    let _runtime = runtime.enter();
                    // The embedding Tokio task cannot replenish its budget while running asyncio.
                    EMBEDDING_ALLOWS_BLOCKING
                        .sync_scope(allows_blocking, || pin!(unconstrained(future.as_mut())).poll(cx))
                } else {
                    // One poll without parking, with a fresh Tokio cooperative budget.
                    runtime.block_on(async { future.as_mut().poll(cx) })
                }
            })
        })
    }
}

impl Drop for RuntimeFuture {
    fn drop(&mut self) {
        let _runtime = get_runtime().enter();
        drop(self.0.take());
    }
}
