//! Keep upstream cancellation handling, but prevent its callback from signalling a
//! parent-runtime channel if an inherited asyncio future is cancelled after fork.

use std::{future::Future, mem, process};

use pyo3::prelude::*;
use pyo3_async_runtimes::{TaskLocals, generic};

use super::{DrainingRuntime, get_current_locals};

pub(super) fn future_into_py<'py, F, T>(py: Python<'py>, future: F) -> PyResult<Bound<'py, PyAny>>
where
    F: Future<Output = PyResult<T>> + Send + 'static,
    T: for<'a> IntoPyObject<'a> + Send + 'static,
{
    let locals = get_current_locals(py)?;
    let event_loop = locals.event_loop(py);
    // Nested bridge calls can inherit these task locals from a synchronous host callback.
    // Unwrap before creating another proxy so the returned Future remains the real one.
    let event_loop = match event_loop.cast::<LoopProxy>() {
        Ok(proxy) => proxy.get().event_loop.clone_ref(py),
        Err(_) => event_loop.unbind(),
    };
    let loop_proxy = Py::new(py, LoopProxy { event_loop })?;
    let locals = TaskLocals::new(loop_proxy.into_bound(py).into_any()).with_context(locals.context(py));
    let bridge = generic::future_into_py_with_locals::<DrainingRuntime, _, _>(py, locals, future)?;
    // Only the upstream bridge sees the proxy; Python callers receive the real Future.
    Ok(bridge.cast::<FutureProxy>()?.get().future.bind(py).clone())
}

/// Override future creation while scheduling still uses the caller's real event loop.
#[pyclass(frozen)]
struct LoopProxy {
    event_loop: Py<PyAny>,
}

#[pymethods]
impl LoopProxy {
    fn create_future(&self, py: Python<'_>) -> PyResult<Py<FutureProxy>> {
        Py::new(
            py,
            FutureProxy {
                future: self.event_loop.call_method0(py, "create_future")?,
            },
        )
    }

    fn __getattr__<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
        self.event_loop.bind(py).getattr(name)
    }
}

/// Intercept only upstream cancellation registration; delegate completion to the real future.
#[pyclass(frozen)]
struct FutureProxy {
    future: Py<PyAny>,
}

#[pymethods]
impl FutureProxy {
    fn add_done_callback(&self, py: Python<'_>, callback: Py<PyAny>) -> PyResult<()> {
        self.future.call_method1(
            py,
            "add_done_callback",
            (ForkSafeCallback {
                pid: process::id(),
                callback: Some(callback),
            },),
        )?;
        Ok(())
    }

    fn __getattr__<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
        self.future.bind(py).getattr(name)
    }
}

#[pyclass(frozen)]
struct ForkSafeCallback {
    pid: u32,
    callback: Option<Py<PyAny>>,
}

#[pymethods]
impl ForkSafeCallback {
    fn __call__(&self, py: Python<'_>, future: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.pid == process::id()
            && let Some(callback) = &self.callback
        {
            callback.call1(py, (future,))?;
        }
        Ok(())
    }
}

impl Drop for ForkSafeCallback {
    fn drop(&mut self) {
        if self.pid != process::id() {
            // Dropping the upstream callback also drops its cancellation sender.
            // The child must neither signal nor drop that inherited channel.
            mem::forget(self.callback.take());
        }
    }
}
