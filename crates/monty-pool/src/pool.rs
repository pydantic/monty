//! The elastic worker pool: prewarming, checkout, replacement, teardown.

use std::{
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::Instant,
};

use monty_proto::pb;

use crate::{
    PoolConfig, PoolError,
    checkout::{Checkout, ReplConfig},
    watchdog::Watchdog,
    worker::{Worker, lock_ignore_poison},
};

/// An elastic pool of `monty --subprocess` workers.
///
/// `min_processes` workers spawn eagerly so the first checkout is fast;
/// further workers spawn on demand up to `max_processes`, and dead workers
/// are detected and replaced transparently. See the crate docs for the full
/// lifecycle.
///
/// `Pool` is safe to share across threads. Dropping it kills and reaps all
/// idle workers; workers held by live [`Checkout`]s die when those are
/// finished or dropped.
pub struct Pool {
    pub(crate) inner: Arc<PoolInner>,
}

pub(crate) struct PoolInner {
    pub(crate) config: PoolConfig,
    state: Mutex<PoolState>,
    /// Signalled whenever a worker returns to the idle queue or capacity is
    /// released, waking blocked `checkout` calls.
    available: Condvar,
    pub(crate) watchdog: Watchdog,
}

struct PoolState {
    idle: Vec<Worker>,
    /// Live workers: idle + checked out + currently being spawned.
    total: usize,
}

impl Pool {
    /// Creates the pool and eagerly spawns `min_processes` workers, failing
    /// fast if the binary cannot be spawned.
    pub fn new(config: PoolConfig) -> Result<Self, PoolError> {
        if config.min_processes > config.max_processes || config.max_processes == 0 {
            return Err(PoolError::Spawn(format!(
                "invalid pool size: min_processes={} max_processes={}",
                config.min_processes, config.max_processes
            )));
        }
        let watchdog =
            Watchdog::new().map_err(|err| PoolError::Spawn(format!("failed to spawn the watchdog thread: {err}")))?;
        let mut idle = Vec::with_capacity(config.min_processes);
        for _ in 0..config.min_processes {
            idle.push(Worker::spawn(&config)?);
        }
        let total = idle.len();
        Ok(Self {
            inner: Arc::new(PoolInner {
                config,
                state: Mutex::new(PoolState { idle, total }),
                available: Condvar::new(),
                watchdog,
            }),
        })
    }

    /// Dedicates a worker to one REPL session created from `repl`.
    ///
    /// Takes an idle worker when one exists, spawns a new one while below
    /// `max_processes`, and otherwise blocks up to `checkout_timeout`
    /// (forever when `None`) before failing with [`PoolError::Exhausted`].
    pub fn checkout(&self, repl: &ReplConfig) -> Result<Checkout, PoolError> {
        let worker = self.inner.acquire_worker()?;
        Checkout::create(worker, Arc::clone(&self.inner), repl)
    }

    /// Number of idle workers right now (diagnostics/tests only — the value
    /// is stale the moment it is returned).
    #[must_use]
    pub fn idle_workers(&self) -> usize {
        lock_ignore_poison(&self.inner.state).idle.len()
    }

    /// PIDs of the idle workers (diagnostics/tests only).
    #[must_use]
    pub fn idle_worker_pids(&self) -> Vec<u32> {
        lock_ignore_poison(&self.inner.state)
            .idle
            .iter()
            .map(Worker::pid)
            .collect()
    }
}

impl PoolInner {
    /// Takes an idle worker, spawning or waiting as capacity allows.
    fn acquire_worker(&self) -> Result<Worker, PoolError> {
        let deadline = self.config.checkout_timeout.map(|t| Instant::now() + t);
        let mut state = lock_ignore_poison(&self.state);
        loop {
            // discard workers that died while idle — their replacement is
            // the spawn below or a later checkout's spawn
            while let Some(worker) = state.idle.pop() {
                if worker.is_dead() {
                    state.total -= 1;
                    drop(worker); // reaps
                } else {
                    return Ok(worker);
                }
            }
            if state.total < self.config.max_processes {
                // reserve capacity before releasing the lock to spawn
                state.total += 1;
                drop(state);
                return Worker::spawn(&self.config).inspect_err(|_| self.release_capacity());
            }
            state = match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(PoolError::Exhausted);
                    }
                    let (guard, _) = self
                        .available
                        .wait_timeout(state, deadline - now)
                        .unwrap_or_else(PoisonError::into_inner);
                    guard
                }
                None => self.available.wait(state).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }

    /// Returns a healthy worker to the idle queue (or retires it when it hit
    /// the recycle limit).
    pub(crate) fn release_worker(&self, worker: Worker) {
        let recycle = self
            .config
            .max_checkouts_per_worker
            .is_some_and(|max| worker.checkouts_served >= max);
        if recycle {
            drop(worker); // kill + reap
            self.release_capacity();
        } else {
            lock_ignore_poison(&self.state).idle.push(worker);
            self.available.notify_one();
        }
    }

    /// Records the death/retirement of a worker, freeing capacity for a
    /// future spawn.
    pub(crate) fn release_capacity(&self) {
        lock_ignore_poison(&self.state).total -= 1;
        self.available.notify_one();
    }

    /// Asks idle workers to exit cleanly; called on pool drop. Workers that
    /// don't comply are killed by `Worker::drop` anyway.
    fn shutdown_idle(&self) {
        let mut state = lock_ignore_poison(&self.state);
        for worker in &mut state.idle {
            let _ = worker.send(&pb::ParentRequest {
                kind: Some(pb::parent_request::Kind::Shutdown(pb::Shutdown {})),
            });
        }
        // dropping the workers reaps them (kill is a no-op if Shutdown won)
        state.idle.clear();
    }
}

impl Drop for PoolInner {
    fn drop(&mut self) {
        self.shutdown_idle();
    }
}
