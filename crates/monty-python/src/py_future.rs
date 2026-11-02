//! Keeps asyncio result deliveries alive until they release their Python references.
//!
//! `call_soon_threadsafe` releases the GIL while waking the loop. Python can finish
//! and start finalizing before the delivery thread regains the GIL; older CPython
//! versions then terminate that thread while it still owns Python objects.
//! Count the bridge's blocking deliveries and drain them from an `atexit` hook.

use std::{
    future::Future,
    pin::Pin,
    sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError},
};

use pyo3::prelude::*;
use pyo3_async_runtimes::{TaskLocals, generic, tokio::get_runtime};
use tokio::task::{JoinError, JoinHandle};

static DELIVERIES: LazyLock<Deliveries> = LazyLock::new(Deliveries::default);

tokio::task_local! {
    static TASK_LOCALS: TaskLocals;
}

/// Uses the upstream bridge's cancellation, result conversion and panic handling.
pub(crate) fn future_into_py<'py, F, T>(py: Python<'py>, future: F) -> PyResult<Bound<'py, PyAny>>
where
    F: Future<Output = PyResult<T>> + Send + 'static,
    T: for<'a> IntoPyObject<'a> + Send + 'static,
{
    generic::future_into_py::<DrainingRuntime, _, _>(py, future)
}

/// Coroutine conversion must read the same task locals that the result bridge scopes.
pub(crate) fn get_current_locals(py: Python<'_>) -> PyResult<TaskLocals> {
    generic::get_current_locals::<DrainingRuntime>(py)
}

/// Waits before Python finalization, releasing the GIL so deliveries can finish.
#[pyfunction]
pub(crate) fn drain_deliveries(py: Python<'_>) {
    py.detach(|| DELIVERIES.close());
}

struct DrainingRuntime;

impl generic::Runtime for DrainingRuntime {
    type JoinError = JoinError;
    type JoinHandle = JoinHandle<()>;

    fn spawn<F>(future: F) -> Self::JoinHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        get_runtime().spawn(future)
    }

    fn spawn_blocking<F>(deliver: F) -> Self::JoinHandle
    where
        F: FnOnce() + Send + 'static,
    {
        // Register before queuing: shutdown must also wait for tasks not yet started.
        let delivery = DELIVERIES.begin();
        get_runtime().spawn_blocking(move || {
            if let Some(_delivery) = delivery {
                deliver();
            }
            // After shutdown starts, drop the undelivered closure without attaching to Python.
        })
    }
}

impl generic::ContextExt for DrainingRuntime {
    fn scope<F, R>(locals: TaskLocals, future: F) -> Pin<Box<dyn Future<Output = R> + Send>>
    where
        F: Future<Output = R> + Send + 'static,
    {
        Box::pin(TASK_LOCALS.scope(locals, future))
    }

    fn get_task_locals() -> Option<TaskLocals> {
        TASK_LOCALS.try_with(Clone::clone).ok()
    }
}

#[derive(Default)]
struct Deliveries {
    state: Mutex<DeliveryState>,
    idle: Condvar,
}

#[derive(Default)]
struct DeliveryState {
    in_flight: usize,
    closing: bool,
}

impl Deliveries {
    fn begin(&self) -> Option<Delivery<'_>> {
        let mut state = self.lock();
        if state.closing {
            return None;
        }
        state.in_flight += 1;
        Some(Delivery(self))
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closing = true;
        while state.in_flight != 0 {
            state = self.idle.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn lock(&self) -> MutexGuard<'_, DeliveryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Outlives the delivery closure's Python references, including on panic.
struct Delivery<'a>(&'a Deliveries);

impl Drop for Delivery<'_> {
    fn drop(&mut self) {
        self.0.lock().in_flight -= 1;
        self.0.idle.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        panic,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use super::Deliveries;

    #[test]
    fn shutdown_waits_for_deliveries_and_refuses_later_ones() {
        let deliveries = Deliveries::default();
        let delivery = deliveries.begin().unwrap();
        let (done, finished) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(|| {
                deliveries.close();
                done.send(()).unwrap();
            });
            let start = Instant::now();
            while !deliveries.lock().closing {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "shutdown thread did not start"
                );
                thread::yield_now();
            }
            assert!(matches!(finished.try_recv(), Err(mpsc::TryRecvError::Empty)));
            assert!(deliveries.begin().is_none());
            drop(delivery);
            finished.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        assert!(deliveries.begin().is_none());
    }

    #[test]
    fn panicking_delivery_releases_its_registration() {
        let deliveries = Deliveries::default();
        assert!(
            panic::catch_unwind(|| {
                let _delivery = deliveries.begin().unwrap();
                panic!("delivery failed");
            })
            .is_err()
        );
        assert_eq!(deliveries.lock().in_flight, 0);
        deliveries.close();
        assert!(deliveries.begin().is_none());
    }
}
