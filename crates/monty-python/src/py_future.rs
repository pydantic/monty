//! Rust-future to asyncio-future bridge that drains before interpreter exit.
//!
//! `pyo3_async_runtimes::future_into_py` resolves the asyncio future from a
//! blocking thread, and `call_soon_threadsafe` releases the GIL while waking
//! the loop. The loop can then run the program to its end and start finalizing
//! the interpreter while that thread still holds Python objects; CPython before
//! 3.14 ends such a thread with `pthread_exit` when it retakes the GIL, and the
//! forced unwind decrefs on a dead interpreter (a segfault). This bridge delivers
//! the same way but counts deliveries, so [`drain_deliveries`], registered with
//! `atexit`, can wait for them before finalization and stop later ones attaching.

use std::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    task::{Context, Poll},
};

use pyo3::{IntoPyObjectExt, exceptions::PyRuntimeError, intern, prelude::*};
use pyo3_async_runtimes::{
    TaskLocals,
    tokio::{get_current_locals, get_runtime, scope},
};
use tokio::{sync::oneshot, task::spawn_blocking};

/// Deliveries in flight, and whether the interpreter has begun exiting.
static DELIVERIES: Deliveries = Deliveries {
    state: Mutex::new(DeliveryState {
        in_flight: 0,
        exiting: false,
    }),
    idle: Condvar::new(),
};

/// Converts a Rust future into an asyncio future on the running loop.
///
/// Mirrors `pyo3_async_runtimes::tokio::future_into_py`: cancelling the asyncio
/// future drops the Rust one, task locals propagate to nested awaits, and a
/// panic surfaces as a `RuntimeError`. Only the delivery is counted, see the
/// module docs.
pub(crate) fn future_into_py<'py, F, T>(py: Python<'py>, fut: F) -> PyResult<Bound<'py, PyAny>>
where
    F: Future<Output = PyResult<T>> + Send + 'static,
    T: for<'a> IntoPyObject<'a> + Send + 'static,
{
    let locals = get_current_locals(py)?;
    let py_future = locals.event_loop(py).call_method0(intern!(py, "create_future"))?;
    let (cancel_tx, cancel_rx) = oneshot::channel();
    py_future.call_method1(
        intern!(py, "add_done_callback"),
        (CancelCallback {
            cancel_tx: Some(cancel_tx),
        },),
    )?;
    let target = py_future.clone().unbind();
    let task_locals = locals.clone();
    get_runtime().spawn(async move {
        let cancellable = Cancellable {
            future: Box::pin(fut),
            cancel_rx: Some(cancel_rx),
        };
        let result = match get_runtime().spawn(scope(task_locals, cancellable)).await {
            Ok(Some(result)) => result,
            // cancelled from Python
            Ok(None) => return,
            Err(err) if err.is_panic() => Err(PyRuntimeError::new_err(format!(
                "rust future panicked: {}",
                panic_message(&*err.into_panic())
            ))),
            // the runtime is shutting down
            Err(_) => return,
        };
        let Some(guard) = DELIVERIES.begin() else { return };
        // the GIL must not be held on a runtime worker: other tasks may need it
        spawn_blocking(move || {
            let _guard = guard;
            Python::attach(|py| deliver(py, &locals, &target, result));
        });
    });
    Ok(py_future)
}

/// Waits for in-flight deliveries and refuses new ones; registered with `atexit`.
#[pyfunction]
pub(crate) fn drain_deliveries(py: Python<'_>) {
    // detached: a delivery may be waiting for the GIL to finish
    py.detach(|| {
        let mut state = DELIVERIES.lock();
        state.exiting = true;
        while state.in_flight > 0 {
            state = DELIVERIES.idle.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    });
}

/// Resolves the asyncio future from a blocking thread via `call_soon_threadsafe`.
fn deliver<T: for<'a> IntoPyObject<'a>>(py: Python<'_>, locals: &TaskLocals, target: &Py<PyAny>, result: PyResult<T>) {
    let future = target.bind(py);
    let delivered = (|| {
        let (method, value) = match result {
            Ok(value) => (intern!(py, "set_result"), value.into_bound_py_any(py)?),
            Err(err) => (
                intern!(py, "set_exception"),
                err.into_value(py).into_bound(py).into_any(),
            ),
        };
        locals.event_loop(py).call_method1(
            intern!(py, "call_soon_threadsafe"),
            (Completor, future, future.getattr(method)?, value),
        )
    })();
    if let Err(err) = delivered {
        err.write_unraisable(py, Some(future));
    }
}

/// Extracts a panic payload's message, as `std` prints it.
fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

/// Count of deliveries between spawning their blocking task and its end.
struct Deliveries {
    state: Mutex<DeliveryState>,
    /// Signalled whenever a delivery ends, for [`drain_deliveries`].
    idle: Condvar,
}

struct DeliveryState {
    in_flight: usize,
    /// Set by [`drain_deliveries`]; later deliveries are dropped undelivered.
    exiting: bool,
}

impl Deliveries {
    /// Registers a delivery, or `None` once the interpreter is exiting.
    fn begin(&'static self) -> Option<DeliveryGuard> {
        let mut state = self.lock();
        if state.exiting {
            None
        } else {
            state.in_flight += 1;
            Some(DeliveryGuard)
        }
    }

    fn lock(&self) -> MutexGuard<'_, DeliveryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Ends its delivery's registration on drop, even if the delivery panicked.
struct DeliveryGuard;

impl Drop for DeliveryGuard {
    fn drop(&mut self) {
        DELIVERIES.lock().in_flight -= 1;
        DELIVERIES.idle.notify_all();
    }
}

/// Runs on the loop thread; skips a future cancelled before its result arrived.
#[pyclass]
struct Completor;

#[pymethods]
impl Completor {
    #[expect(clippy::unused_self, reason = "a pyclass instance method must take &self")]
    fn __call__(&self, future: &Bound<'_, PyAny>, method: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if cancelled(future)? {
            Ok(())
        } else {
            method.call1((value,)).map(drop)
        }
    }
}

/// Done callback on the asyncio future: forwards a cancellation to the Rust task.
#[pyclass]
struct CancelCallback {
    cancel_tx: Option<oneshot::Sender<()>>,
}

#[pymethods]
impl CancelCallback {
    fn __call__(&mut self, future: &Bound<'_, PyAny>) -> PyResult<()> {
        if cancelled(future)?
            && let Some(tx) = self.cancel_tx.take()
        {
            let _ = tx.send(());
        }
        Ok(())
    }
}

fn cancelled(future: &Bound<'_, PyAny>) -> PyResult<bool> {
    future.call_method0(intern!(future.py(), "cancelled"))?.is_truthy()
}

/// Polls the Rust future until it completes, or `None` once the asyncio future is cancelled.
struct Cancellable<F> {
    future: Pin<Box<F>>,
    /// Taken once it resolves: a tokio oneshot panics if polled again.
    cancel_rx: Option<oneshot::Receiver<()>>,
}

impl<F: Future> Future for Cancellable<F> {
    type Output = Option<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.future.as_mut().poll(cx) {
            Poll::Ready(output) => Poll::Ready(Some(output)),
            Poll::Pending => match self.cancel_rx.as_mut().map(|rx| Pin::new(rx).poll(cx)) {
                Some(Poll::Ready(Ok(()))) => {
                    self.cancel_rx = None;
                    Poll::Ready(None)
                }
                // the callback was dropped without cancelling: only the future can finish this
                Some(Poll::Ready(Err(_))) => {
                    self.cancel_rx = None;
                    Poll::Pending
                }
                _ => Poll::Pending,
            },
        }
    }
}
