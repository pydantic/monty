//! Drives Rust protocol futures through PyO3's asyncio coroutine implementation.
//!
//! Python owns execution, callback context and cancellation. Tokio supplies I/O
//! readiness, timers and a cooperative budget for each protocol poll.

use std::{
    future::Future,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
};

use pyo3::{IntoPyObjectExt, exceptions::PyRuntimeError, prelude::*};
use pyo3_async_runtimes::tokio::get_runtime;
use tokio::{runtime::Handle, task::block_in_place};

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
                // One poll without parking, with a fresh Tokio cooperative budget.
                let poll =
                    move || runtime.block_on(async move { future.as_mut().poll(&mut Context::from_waker(waker)) });
                match Handle::try_current() {
                    Err(_) => poll(),
                    // A synchronous Monty print callback can start its own asyncio loop.
                    Ok(handle) if handle.id() == runtime.handle().id() => block_in_place(poll),
                    Ok(_) => Poll::Ready(Err(PyRuntimeError::new_err(
                        "cannot run async Monty inside another Tokio runtime",
                    ))),
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
