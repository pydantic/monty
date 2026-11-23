//! Keeps asyncio result deliveries alive until they release their Python references.
//!
//! `call_soon_threadsafe` releases the GIL while waking the loop. Python can finish
//! and start finalizing before the delivery thread regains the GIL; older CPython
//! versions then terminate that thread while it still owns Python objects.
//! Track bridge creation, pending tasks and blocking deliveries. Before finalization,
//! an `atexit` hook cancels pending Rust tasks and drains their Python references along
//! with admitted deliveries. Later bridge calls fail instead of returning an unresolved future.
//! Admission stays closed: PyO3 does not support loading this extension in another
//! interpreter, and resetting the barrier here would allow tasks to race finalization.
//! Cancellation takes effect between polls: a synchronous host callback already
//! executing inside a poll must return before its task can release its references.
//! Python coroutine tasks are not cancelled or settled here; their finally blocks
//! can run during later Python teardown, with destroyed-pending-task warnings.

use std::{
    future::Future,
    pin::Pin,
    process,
    sync::{
        Condvar, LazyLock, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
    task::{Context, Poll},
};

use pyo3::{exceptions::PyRuntimeError, prelude::*};
use pyo3_async_runtimes::{TaskLocals, generic, tokio::get_runtime};
use tokio::{
    sync::watch,
    task::{JoinError, JoinHandle},
};

mod cancellation;
mod coroutine;

pub(crate) use coroutine::into_future_with_locals;

static OWNER_PID: AtomicU32 = AtomicU32::new(0);
static DELIVERIES: LazyLock<Deliveries> = LazyLock::new(Deliveries::default);

tokio::task_local! {
    // These locals follow the future across Tokio threads and retain its Python
    // event loop and contextvars context for callbacks into async Python code.
    static TASK_LOCALS: TaskLocals;
}

/// Uses the upstream bridge's cancellation, result conversion and panic handling.
pub(crate) fn future_into_py<'py, F, T>(py: Python<'py>, future: F) -> PyResult<Bound<'py, PyAny>>
where
    F: Future<Output = PyResult<T>> + Send + 'static,
    T: for<'a> IntoPyObject<'a> + Send + 'static,
{
    if !original_process() {
        return Err(PyRuntimeError::new_err(
            "Monty async runtime cannot be reused after fork",
        ));
    }
    // Event-loop methods can release the GIL while the bridge is being created.
    // Keep its Python references registered even if shutdown rejects its spawn.
    let _creation = DELIVERIES
        .begin()
        .ok_or_else(|| PyRuntimeError::new_err("Monty async runtime is shutting down"))?;
    let future = cancellation::future_into_py(py, future)?;
    // A rejected spawn returns no error through the upstream Runtime trait.
    // Detect shutdown here instead of returning a future that cannot complete.
    if DELIVERIES.lock().closing {
        return Err(PyRuntimeError::new_err("Monty async runtime is shutting down"));
    }
    Ok(future)
}

/// Coroutine conversion must read the same task locals that the result bridge scopes.
pub(crate) fn get_current_locals(py: Python<'_>) -> PyResult<TaskLocals> {
    generic::get_current_locals::<DrainingRuntime>(py)
}

/// Waits before Python finalization, releasing the GIL so deliveries can finish.
fn drain_deliveries(py: Python<'_>) {
    // A fork child has no parent runtime threads, and even inherited locks may be held.
    if !original_process() {
        return;
    }
    // Holding the GIL here would prevent the threads we wait for from finishing.
    // Reattaching also flushes PyO3's deferred decrefs from cancelled Rust tasks.
    py.detach(|| DELIVERIES.close());
}

/// Check before touching a pool's inherited locks or moving it into a rejected future.
pub(crate) fn ensure_original_process() -> PyResult<()> {
    if original_process() {
        Ok(())
    } else {
        Err(PyRuntimeError::new_err(
            "Monty async runtime cannot be reused after fork",
        ))
    }
}

fn original_process() -> bool {
    OWNER_PID.load(Ordering::Relaxed) == process::id()
}

/// Register an exclusively atexit-owned hook so clearing a skipped callback drains too.
pub(crate) fn register_shutdown(py: Python<'_>) -> PyResult<()> {
    let pid = process::id();
    if OWNER_PID
        .compare_exchange(0, pid, Ordering::Relaxed, Ordering::Relaxed)
        .is_err_and(|owner| owner != pid)
    {
        return Err(PyRuntimeError::new_err(
            "Monty async runtime cannot be reused after fork",
        ));
    }
    // Initialize before publishing the hook; fork children check OWNER_PID before this LazyLock.
    LazyLock::force(&DELIVERIES);
    py.import("atexit")?
        .call_method1("register", (Py::new(py, ShutdownHook)?,))?;
    Ok(())
}

#[pyclass(frozen)]
struct ShutdownHook;

#[pymethods]
impl ShutdownHook {
    #[expect(clippy::unused_self, reason = "a Python callable must take &self")]
    fn __call__(&self, py: Python<'_>) {
        drain_deliveries(py);
    }
}

impl Drop for ShutdownHook {
    fn drop(&mut self) {
        if original_process() {
            // CPython clears its callback registry before marking the interpreter finalizing.
            // A hook registered by an active exit handler is skipped but released here.
            Python::try_attach(drain_deliveries);
        }
    }
}

/// Uses the shared Tokio executor, tracking all bridge-owned Python references.
struct DrainingRuntime;

impl generic::Runtime for DrainingRuntime {
    type JoinError = JoinError;
    type JoinHandle = JoinHandle<()>;

    fn spawn<F>(future: F) -> Self::JoinHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match track(future) {
            Some(task) => get_runtime().spawn(async move {
                let _ = task.await;
            }),
            None => get_runtime().spawn(async {}),
        }
    }

    fn spawn_blocking<F>(deliver: F) -> Self::JoinHandle
    where
        F: FnOnce() + Send + 'static,
    {
        // Register before queuing: shutdown must also wait for tasks not yet started.
        let Some(delivery) = DELIVERIES.begin() else {
            // Drop references synchronously while the calling task is still tracked.
            drop(deliver);
            return get_runtime().spawn(async {});
        };
        get_runtime().spawn_blocking(move || {
            let _delivery = delivery;
            // Consume the closure before dropping the guard: its Python references
            // must be released before shutdown observes this delivery as finished.
            deliver();
        })
    }
}

/// Registers before spawning so shutdown also waits for tasks that have not been polled.
/// Deferred callback tasks use this too: dropping their JoinSet only requests cancellation.
pub(crate) fn track<F>(future: F) -> Option<impl Future<Output = Option<F::Output>> + Send>
where
    F: Future + Send + 'static,
{
    if !original_process() {
        return None;
    }
    let mut closing = DELIVERIES.shutdown.subscribe();
    // On rejection, the caller still holds either the GIL or a parent registration.
    let delivery = DELIVERIES.begin()?;
    let shutdown = Box::pin(async move {
        if !*closing.borrow_and_update() {
            let _ = closing.changed().await;
        }
    });
    Some(TrackedTask {
        future: Box::pin(future),
        shutdown,
        _delivery: delivery,
    })
}

impl generic::ContextExt for DrainingRuntime {
    fn scope<F, R>(locals: TaskLocals, future: F) -> Pin<Box<dyn Future<Output = R> + Send>>
    where
        F: Future<Output = R> + Send + 'static,
    {
        // get_current_locals must use this same scope; the upstream Tokio adapter
        // has separate task locals and would lose the callback's Python context.
        Box::pin(TASK_LOCALS.scope(locals, future))
    }

    fn get_task_locals() -> Option<TaskLocals> {
        TASK_LOCALS.try_with(Clone::clone).ok()
    }
}

// Fields drop in declaration order, even if the executor drops an unpolled task.
// The future must release its Python references before its registration ends.
struct TrackedTask<'a, F> {
    future: Pin<Box<F>>,
    shutdown: Pin<Box<dyn Future<Output = ()> + Send>>,
    _delivery: Delivery<'a>,
}

impl<F: Future> Future for TrackedTask<'_, F> {
    type Output = Option<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        self.future.as_mut().poll(cx).map(Some)
    }
}

struct Deliveries {
    pid: u32,
    // Admission and shutdown share one lock so close cannot miss a queued delivery.
    state: Mutex<DeliveryState>,
    idle: Condvar,
    shutdown: watch::Sender<bool>,
}

impl Default for Deliveries {
    fn default() -> Self {
        Self {
            pid: process::id(),
            state: Mutex::default(),
            idle: Condvar::new(),
            shutdown: watch::channel(false).0,
        }
    }
}

#[derive(Default)]
struct DeliveryState {
    /// Bridge creation, pending tasks and deliveries still owning Python references.
    in_flight: usize,
    /// Permanent admission barrier for this process; reopening would race finalization.
    closing: bool,
}

impl Deliveries {
    fn begin(&self) -> Option<Delivery<'_>> {
        if self.pid != process::id() {
            return None;
        }
        let mut state = self.lock();
        if state.closing {
            return None;
        }
        state.in_flight += 1;
        Some(Delivery(self))
    }

    fn close(&self) {
        if self.pid != process::id() {
            return;
        }
        let mut state = self.lock();
        state.closing = true;
        // Wake pending async tasks so an unfinished operation cannot stall shutdown.
        // Blocking deliveries finish normally because they may already hold the GIL.
        self.shutdown.send_replace(true);
        // Condvar::wait releases the mutex so delivery guards can decrement the
        // count. Recheck it after every wake, including spurious wakes.
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
        if self.0.pid != process::id() {
            return;
        }
        self.0.lock().in_flight -= 1;
        self.0.idle.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        panic::{self, AssertUnwindSafe},
        sync::{Barrier, LazyLock, mpsc},
        task::{Context, Waker},
        thread,
        time::{Duration, Instant},
    };

    use tokio::{runtime::Builder, sync::oneshot};

    use super::{Deliveries, Future, get_runtime};

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
    fn cancelled_task_drops_references_before_registration() {
        struct Owned<'a>(&'a Deliveries);

        impl Drop for Owned<'_> {
            fn drop(&mut self) {
                assert_eq!(self.0.lock().in_flight, 1);
            }
        }

        let deliveries = Deliveries::default();
        let owned = Owned(&deliveries);
        let mut closing = deliveries.shutdown.subscribe();
        let task = super::TrackedTask {
            future: Box::pin(async move {
                let _owned = owned;
                pending::<()>().await;
            }),
            shutdown: Box::pin(async move {
                closing.changed().await.unwrap();
            }),
            _delivery: deliveries.begin().unwrap(),
        };
        thread::scope(|scope| {
            scope.spawn(|| deliveries.close());
            get_runtime().block_on(task);
        });
        assert_eq!(deliveries.lock().in_flight, 0);
    }

    #[test]
    fn unpolled_completed_and_panicking_tasks_keep_references_registered() {
        struct Owned<'a>(&'a Deliveries);

        impl Drop for Owned<'_> {
            fn drop(&mut self) {
                assert_eq!(self.0.lock().in_flight, 1);
            }
        }

        for action in ["unpolled", "complete", "panic"] {
            let deliveries = Deliveries::default();
            let owned = Owned(&deliveries);
            let mut task = Box::pin(super::TrackedTask {
                future: Box::pin(async move {
                    let _owned = owned;
                    assert_ne!(action, "panic", "task panic");
                }),
                shutdown: Box::pin(pending()),
                _delivery: deliveries.begin().unwrap(),
            });
            if action != "unpolled" {
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    task.as_mut().poll(&mut Context::from_waker(Waker::noop()))
                }));
                assert_eq!(result.is_err(), action == "panic");
                if let Ok(result) = result {
                    assert!(result.is_ready());
                }
            }
            drop(task);
            assert_eq!(deliveries.lock().in_flight, 0);
            deliveries.close();
        }
    }

    #[test]
    fn executor_abort_and_panic_release_references_before_registration() {
        static DELIVERIES: LazyLock<Deliveries> = LazyLock::new(Deliveries::default);

        struct Owned;

        impl Drop for Owned {
            fn drop(&mut self) {
                assert_eq!(DELIVERIES.lock().in_flight, 1);
            }
        }

        // A single-thread executor keeps the unpolled-abort case deterministic.
        let runtime = Builder::new_current_thread().build().unwrap();
        for action in ["abort_unpolled", "abort_pending", "panic"] {
            let owned = Owned;
            let (started, ready) = oneshot::channel();
            let task = runtime.spawn(super::TrackedTask {
                future: Box::pin(async move {
                    let _owned = owned;
                    started.send(()).unwrap();
                    assert_ne!(action, "panic", "task panic");
                    pending::<()>().await;
                }),
                shutdown: Box::pin(pending()),
                _delivery: DELIVERIES.begin().unwrap(),
            });
            if action == "abort_unpolled" {
                task.abort();
            }
            runtime.block_on(async move {
                if action == "abort_pending" {
                    ready.await.unwrap();
                    task.abort();
                }
                let error = task.await.unwrap_err();
                assert_eq!(error.is_panic(), action == "panic");
            });
            assert_eq!(DELIVERIES.lock().in_flight, 0);
        }
        DELIVERIES.close();
    }

    #[test]
    fn admission_racing_with_concurrent_drains_leaves_no_tasks() {
        for _ in 0..100 {
            let deliveries = Deliveries::default();
            let start = Barrier::new(8);
            thread::scope(|scope| {
                for _ in 0..6 {
                    scope.spawn(|| {
                        start.wait();
                        for _ in 0..100 {
                            let Some(delivery) = deliveries.begin() else { break };
                            thread::yield_now();
                            drop(delivery);
                        }
                    });
                }
                for _ in 0..2 {
                    scope.spawn(|| {
                        start.wait();
                        deliveries.close();
                        assert_eq!(deliveries.lock().in_flight, 0);
                        assert!(deliveries.begin().is_none());
                    });
                }
            });
            assert_eq!(deliveries.lock().in_flight, 0);
        }
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
