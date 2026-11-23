//! Python-to-Rust completion callbacks must not wake inherited Tokio tasks after fork.
//! The upstream callback drops its channel sender unconditionally, which can wake a
//! vanished runtime when the fork child garbage-collects a pending asyncio task.

use std::{future::Future, mem, process};

use pyo3::{intern, prelude::*, types::PyDict};
use pyo3_async_runtimes::TaskLocals;
use tokio::sync::oneshot;

type Answer = PyResult<Py<PyAny>>;

/// Mirrors upstream scheduling and context propagation, with a fork-safe sender.
pub(crate) fn into_future_with_locals(
    locals: &TaskLocals,
    awaitable: Bound<'_, PyAny>,
) -> PyResult<impl Future<Output = Answer> + Send + use<>> {
    let py = awaitable.py();
    let (tx, rx) = oneshot::channel();
    let kwargs = PyDict::new(py);
    kwargs.set_item("context", locals.context(py))?;
    locals.event_loop(py).call_method(
        "call_soon_threadsafe",
        (EnsureFuture {
            awaitable: awaitable.unbind(),
            sender: Sender {
                pid: process::id(),
                tx: Some(tx),
            },
        },),
        Some(&kwargs),
    )?;
    Ok(async move {
        match rx.await {
            Ok(answer) => answer,
            Err(_) => {
                Python::attach(|py| Err(PyErr::from_value(py.import("asyncio")?.call_method0("CancelledError")?)))
            }
        }
    })
}

struct Sender {
    pid: u32,
    tx: Option<oneshot::Sender<Answer>>,
}

impl Drop for Sender {
    fn drop(&mut self) {
        if self.pid != process::id() {
            // No receiver thread survives in the child. Dropping this inherited
            // sender would wake its invalid runtime; let process exit reclaim it.
            mem::forget(self.tx.take());
        }
    }
}

#[pyclass]
struct EnsureFuture {
    awaitable: Py<PyAny>,
    sender: Sender,
}

#[pymethods]
impl EnsureFuture {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.sender.pid != process::id() {
            return Ok(());
        }
        let task = py
            .import("asyncio")?
            .call_method1("ensure_future", (self.awaitable.bind(py),))?;
        task.call_method1(
            "add_done_callback",
            (Complete {
                sender: Sender {
                    pid: self.sender.pid,
                    tx: self.sender.tx.take(),
                },
            },),
        )?;
        Ok(())
    }
}

#[pyclass]
struct Complete {
    sender: Sender,
}

#[pymethods]
impl Complete {
    fn __call__(&mut self, task: &Bound<'_, PyAny>) {
        if self.sender.pid == process::id()
            && let Some(tx) = self.sender.tx.take()
        {
            let answer = task.call_method0(intern!(task.py(), "result")).map(Bound::unbind);
            // The Rust operation may already have been cancelled.
            let _ = tx.send(answer);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::Pin,
        process,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Wake, Waker},
    };

    use tokio::sync::oneshot;

    use super::Sender;

    #[test]
    fn only_parent_sender_drop_wakes_the_receiver() {
        struct CountWake(AtomicUsize);

        impl Wake for CountWake {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        for inherited in [false, true] {
            let (tx, mut rx) = oneshot::channel();
            let counter = Arc::new(CountWake(AtomicUsize::new(0)));
            let waker = Waker::from(Arc::clone(&counter));
            assert!(Pin::new(&mut rx).poll(&mut Context::from_waker(&waker)).is_pending());
            drop(Sender {
                pid: process::id().wrapping_add(u32::from(inherited)),
                tx: Some(tx),
            });
            assert_eq!(counter.0.load(Ordering::Relaxed), usize::from(!inherited));
        }
    }
}
