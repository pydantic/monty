//! Hard-deadline enforcement: a single thread per pool that kills workers
//! whose protocol turn exceeds `request_timeout`.
//!
//! The owning thread blocks on a pipe read for the whole turn, so it cannot
//! enforce a deadline itself. The watchdog kills the child instead; the
//! blocked read then fails, and the worker's `killed_for_timeout` flag tells
//! the owner to classify the failure as [`crate::PoolError::Timeout`].

use std::{
    collections::BTreeMap,
    io,
    process::Child,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::worker::{Worker, lock_ignore_poison};

/// Deadline registry plus the thread that enforces it.
pub(crate) struct Watchdog {
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    condvar: Condvar,
}

#[derive(Default)]
struct State {
    /// Armed deadlines, ordered by expiry. The `u64` disambiguates equal
    /// instants.
    deadlines: BTreeMap<(Instant, u64), KillTarget>,
    next_id: u64,
    shutdown: bool,
}

/// What to do when a deadline fires.
struct KillTarget {
    child: Arc<Mutex<Child>>,
    killed_for_timeout: Arc<AtomicBool>,
}

impl Watchdog {
    /// Spawns the enforcement thread; fails (instead of panicking) under
    /// thread-resource exhaustion so pool construction can surface the error.
    pub(crate) fn new() -> io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            condvar: Condvar::new(),
        });
        let thread = thread::Builder::new().name("monty-pool-watchdog".to_owned()).spawn({
            let shared = Arc::clone(&shared);
            move || watchdog_loop(&shared)
        })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Arms a kill deadline for `worker`. The deadline is disarmed when the
    /// returned guard drops (i.e. when the turn ends first).
    pub(crate) fn arm(&self, worker: &Worker, timeout: Option<Duration>) -> Option<DeadlineGuard> {
        let timeout = timeout?;
        let (child, killed_for_timeout) = worker.kill_handles();
        let key = {
            let mut state = lock_ignore_poison(&self.shared.state);
            let key = (Instant::now() + timeout, state.next_id);
            state.next_id += 1;
            state.deadlines.insert(
                key,
                KillTarget {
                    child,
                    killed_for_timeout,
                },
            );
            key
        };
        self.shared.condvar.notify_one();
        Some(DeadlineGuard {
            shared: Arc::clone(&self.shared),
            key,
        })
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        lock_ignore_poison(&self.shared.state).shutdown = true;
        self.shared.condvar.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// RAII disarm handle returned by [`Watchdog::arm`].
pub(crate) struct DeadlineGuard {
    shared: Arc<Shared>,
    key: (Instant, u64),
}

impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        lock_ignore_poison(&self.shared.state).deadlines.remove(&self.key);
        self.shared.condvar.notify_one();
    }
}

/// Fires expired deadlines, then sleeps until the next one (or until armed /
/// disarmed / shut down).
fn watchdog_loop(shared: &Shared) {
    let mut state = lock_ignore_poison(&shared.state);
    loop {
        if state.shutdown {
            return;
        }
        let now = Instant::now();
        while let Some((&(at, _), _)) = state.deadlines.first_key_value() {
            if at > now {
                break;
            }
            let (_, target) = state.deadlines.pop_first().expect("checked non-empty");
            // flag BEFORE killing so the owner's failed read always sees it
            target.killed_for_timeout.store(true, Ordering::SeqCst);
            let _ = lock_ignore_poison(&target.child).kill();
        }
        state = match state.deadlines.first_key_value().map(|(&(at, _), _)| at) {
            Some(at) => {
                // a fresh `now`: time spent killing expired workers above must
                // not stretch the sleep and delay the next deadline
                let wait = at.saturating_duration_since(Instant::now());
                shared
                    .condvar
                    .wait_timeout(state, wait)
                    .unwrap_or_else(|err| {
                        let (guard, timeout) = err.into_inner();
                        (guard, timeout)
                    })
                    .0
            }
            None => shared.condvar.wait(state).unwrap_or_else(PoisonError::into_inner),
        };
    }
}
