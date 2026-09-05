//! The elastic worker pool: prewarming, checkout, replacement, teardown.

use std::{
    mem,
    pin::pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use futures_util::future::join_all;
use monty_proto::pb;
use tokio::{
    sync::Notify,
    time::{Instant, timeout_at},
};

use crate::{
    PoolConfig, PoolError,
    checkout::{Checkout, CheckoutOptions, ReplConfig, request},
    worker::Worker,
};

/// An elastic pool of `monty subprocess` workers.
///
/// `min_processes` workers spawn eagerly so the first checkout is fast;
/// further workers spawn on demand up to `max_processes`, and dead workers
/// are detected and replaced transparently. See the crate docs for the full
/// lifecycle.
///
/// `Pool` is safe to share across tasks and threads. [`Pool::close`] asks idle
/// workers to exit cleanly; merely dropping the pool kills them instead (via
/// their kill-on-drop handles). Workers held by live [`Checkout`]s die when
/// those are finished or dropped.
pub struct Pool {
    pub(crate) inner: Arc<PoolInner>,
}

pub(crate) struct PoolInner {
    pub(crate) config: PoolConfig,
    /// Guarded by a *synchronous* mutex: every critical section is short and
    /// await-free, and worker/checkout `Drop` impls must be able to release
    /// capacity without an async context.
    state: Mutex<PoolState>,
    /// Signalled whenever a worker returns to the idle queue or capacity is
    /// released, waking blocked `checkout` calls.
    available: Notify,
}

struct PoolState {
    idle: Vec<Worker>,
    /// Live workers: idle + checked out + currently being spawned.
    total: usize,
}

impl Pool {
    /// Creates the pool and eagerly spawns `min_processes` workers, failing
    /// fast if the binary cannot be spawned. Must be called within a tokio
    /// runtime (worker process and pipe I/O is driven by the runtime).
    pub async fn new(config: PoolConfig) -> Result<Self, PoolError> {
        if config.min_processes > config.max_processes || config.max_processes == 0 {
            return Err(PoolError::Spawn(format!(
                "invalid pool size: min_processes={} max_processes={}",
                config.min_processes, config.max_processes
            )));
        }
        // Only the subprocess transport pre-warms workers; WebSocket connections
        // are made per-checkout (its `min_processes` is 0).
        let mut idle = Vec::with_capacity(config.min_processes);
        if !config.transport.is_websocket() {
            for _ in 0..config.min_processes {
                idle.push(Worker::new(&config, &[]).await?);
            }
        }
        let total = idle.len();
        let pool = Self {
            inner: Arc::new(PoolInner {
                config,
                state: Mutex::new(PoolState { idle, total }),
                available: Notify::new(),
            }),
        };
        let workers = worker_count(total);
        pool.inner.record_worker_delta(workers, workers);
        Ok(pool)
    }

    /// Dedicates a worker to one REPL session created from `repl`, with
    /// default [`CheckoutOptions`] — see [`Self::checkout_with`].
    pub async fn checkout(&self, repl: &ReplConfig) -> Result<Checkout, PoolError> {
        self.checkout_with(repl, CheckoutOptions::default()).await
    }

    /// Dedicates a worker to one REPL session created from `repl`, with the
    /// host context `options` carries.
    ///
    /// Takes an idle worker when one exists, spawns or dials a new one (with
    /// `options.connect_headers`, preceded by the W3C trace headers of
    /// `options.telemetry`) while below `max_processes`, and otherwise waits
    /// up to `checkout_timeout` (forever when `None`) before failing with
    /// [`PoolError::Exhausted`].
    pub async fn checkout_with(&self, repl: &ReplConfig, mut options: CheckoutOptions) -> Result<Checkout, PoolError> {
        // ahead of the caller's headers, which stay last-wins
        #[cfg(feature = "telemetry")]
        if let Some(telemetry) = &options.telemetry {
            options.connect_headers.splice(0..0, telemetry.propagation_headers());
        }
        let worker = self.inner.acquire_worker(&options.connect_headers).await?;
        #[cfg(feature = "telemetry")]
        let worker = worker.with_adapter_context(options.telemetry);
        Checkout::create(worker, Arc::clone(&self.inner), repl).await
    }

    /// Asks idle workers to exit cleanly and reaps them, capping the wait per
    /// worker. Sessions still checked out keep their workers until they finish.
    ///
    /// Optional: dropping the pool kills idle workers instead, which is just
    /// as safe — this only trades a SIGKILL for a clean protocol goodbye.
    ///
    /// Telemetry exporter shutdown remains the configuring application's
    /// responsibility and should happen after checked-out sessions finish.
    pub async fn close(&self) {
        // Pair each removed worker with a capacity guard immediately: if this
        // future is dropped mid-close, every unreaped worker is killed by its
        // kill-on-drop handle and its slot released by the guard, instead of
        // leaking capacity the pool can never recover.
        let mut idle: Vec<_> = {
            let mut state = lock_ignore_poison(&self.inner.state);
            mem::take(&mut state.idle)
        }
        .into_iter()
        .map(|worker| (worker, CapacityGuard::new(&self.inner)))
        .collect();
        self.inner.record_worker_delta(0, -worker_count(idle.len()));
        for _ in &idle {
            self.inner.count_termination("closed");
        }
        for (worker, _) in &mut idle {
            let _ = worker
                .send(&request(pb::parent_request::Kind::Shutdown(pb::Shutdown {})))
                .await;
        }
        // Reap concurrently: a *nonresponsive* worker costs the full grace, so
        // reaping serially would multiply it by the number of stuck workers.
        join_all(idle.into_iter().map(|(mut worker, capacity)| async move {
            let _capacity = capacity;
            worker.reap_or_kill(SHUTDOWN_EXIT_GRACE).await;
        }))
        .await;
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
            .filter_map(Worker::pid)
            .collect()
    }
}

/// How long [`Pool::close`] waits for a worker to exit on its own after the
/// `Shutdown` request before killing it.
const SHUTDOWN_EXIT_GRACE: Duration = Duration::from_millis(500);

impl PoolInner {
    /// Takes a worker, reusing/spawning a local one or connecting a fresh
    /// remote one (with `connect_headers` on its upgrade request), waiting as
    /// capacity allows — and times that for `monty.pool.checkout.wait`.
    pub(crate) async fn acquire_worker(&self, connect_headers: &[(String, String)]) -> Result<Worker, PoolError> {
        #[cfg(feature = "telemetry")]
        let started = Instant::now();
        // set by the acquisition itself: only it can tell an idle reuse from a
        // spawn, or either from a wait for capacity
        let mut outcome = "idle";
        let worker = self.acquire_worker_inner(connect_headers, &mut outcome).await;
        #[cfg(feature = "telemetry")]
        if let Some(metrics) = &self.config.metrics {
            let outcome = match &worker {
                Ok(_) => outcome,
                Err(PoolError::Exhausted) => "exhausted",
                Err(_) => "error",
            };
            metrics.checkout_wait(started.elapsed(), outcome);
        }
        worker
    }

    /// Reuses/spawns a local worker or connects a fresh remote one (with
    /// `connect_headers` on its upgrade request), waiting as capacity allows,
    /// and reports through `outcome` how it got one.
    async fn acquire_worker_inner(
        &self,
        connect_headers: &[(String, String)],
        outcome: &mut &'static str,
    ) -> Result<Worker, PoolError> {
        // WebSocket connections are single-use and never pooled idle, so the
        // idle-reuse step is skipped and each acquisition dials a fresh worker.
        let websocket = self.config.transport.is_websocket();
        let mut waited = false;
        let deadline = self.config.checkout_timeout.map(|t| Instant::now() + t);
        loop {
            // Register for wakeups BEFORE checking state: a release landing
            // between the check and the await below is then still observed
            // (`enable` is what arms an un-polled `Notified`).
            let mut notified = pin!(self.available.notified());
            notified.as_mut().enable();
            // The guard's scope must close before any await below, so the
            // spawn/connect and the notified wait never hold the lock.
            let spawn = {
                let mut state = lock_ignore_poison(&self.state);
                let mut reused = None;
                let mut died_idle = 0;
                let mut removed_idle = 0;
                if !websocket {
                    // discard workers that died while idle — their replacement
                    // is the spawn below or a later checkout's spawn
                    while let Some(mut worker) = state.idle.pop() {
                        removed_idle += 1;
                        if worker.is_dead() {
                            state.total -= 1;
                            died_idle += 1;
                            drop(worker); // kill-on-drop backstop; already dead
                        } else {
                            reused = Some(worker);
                            break;
                        }
                    }
                }
                // reserve capacity before releasing the lock to spawn/connect
                let below_cap = reused.is_none() && state.total < self.config.max_processes;
                if below_cap {
                    state.total += 1;
                }
                drop(state); // never call into the host adapter under the lock
                for _ in 0..died_idle {
                    self.count_termination("died_idle");
                }
                let spawned = i64::from(below_cap);
                self.record_worker_delta(spawned - worker_count(died_idle), -worker_count(removed_idle));
                if let Some(worker) = reused {
                    *outcome = if waited { "waited" } else { "idle" };
                    return Ok(worker);
                }
                below_cap
            };
            if spawn {
                *outcome = if waited { "waited" } else { "spawned" };
                // guard the reserved slot: a failed — or cancelled, for the
                // WebSocket dial — spawn must release it or the pool shrinks
                let capacity = CapacityGuard::new(self);
                let worker = Worker::new(&self.config, connect_headers).await?;
                capacity.disarm();
                return Ok(worker);
            }
            waited = true;
            match deadline {
                Some(deadline) => {
                    if timeout_at(deadline, notified).await.is_err() {
                        return Err(PoolError::Exhausted);
                    }
                }
                None => notified.await,
            }
        }
    }

    /// Counts one worker leaving the pool. Every path that drops a worker
    /// records here or in [`crate::checkout`], so the reasons add up to the
    /// pool's whole turnover.
    pub(crate) fn count_termination(&self, reason: &'static str) {
        #[cfg(feature = "telemetry")]
        if let Some(metrics) = &self.config.metrics {
            metrics.worker_terminated(reason);
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = reason;
    }

    /// Records changes to live and immediately available workers.
    ///
    /// Call with the pool lock released to keep telemetry outside pool
    /// synchronization. Recording only updates Rust SDK aggregates.
    pub(crate) fn record_worker_delta(&self, live: i64, idle: i64) {
        #[cfg(feature = "telemetry")]
        if let Some(metrics) = &self.config.metrics {
            if live != 0 {
                metrics.live_workers(live);
            }
            if idle != 0 {
                metrics.idle_workers(idle);
            }
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = (live, idle);
    }

    /// Returns a healthy worker to the idle queue (or retires it when it hit
    /// the recycle limit, or it is a single-use WebSocket connection).
    pub(crate) fn release_worker(&self, worker: Worker) {
        let websocket = self.config.transport.is_websocket();
        let recycle = websocket
            || self
                .config
                .max_checkouts_per_worker
                .is_some_and(|max| worker.checkouts_served >= max);
        if recycle {
            drop(worker); // kill (on drop) — reaped by tokio in the background
            self.count_termination(if websocket { "single_use" } else { "recycled" });
            self.release_capacity();
        } else {
            lock_ignore_poison(&self.state).idle.push(worker);
            self.available.notify_one();
            self.record_worker_delta(0, 1);
        }
    }

    /// Records the death/retirement of a worker, freeing capacity for a
    /// future spawn.
    pub(crate) fn release_capacity(&self) {
        lock_ignore_poison(&self.state).total -= 1;
        self.available.notify_one();
        self.record_worker_delta(-1, 0);
    }
}

#[cfg(feature = "telemetry")]
impl Drop for PoolInner {
    /// A pool dropped without [`Pool::close`] — a supported shutdown — kills
    /// its idle workers via their kill-on-drop handles; count them here so
    /// that path does not under-report worker turnover. (`close` drains the
    /// idle queue and counts, so nothing is counted twice.)
    fn drop(&mut self) {
        let (live, idle) = {
            let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
            (state.total, state.idle.len())
        };
        self.record_worker_delta(-worker_count(live), -worker_count(idle));
        for _ in 0..idle {
            self.count_termination("closed");
        }
    }
}

/// RAII hold on one reserved capacity slot: releases it on drop unless
/// [`CapacityGuard::disarm`]ed.
///
/// Teardown and spawn paths hold one across their `await`s so a future
/// dropped mid-flight (e.g. asyncio cancellation) still releases the slot —
/// a leaked slot would shrink the pool permanently, down to a pool that can
/// never check out again. The worker itself needs no such guard: its
/// kill-on-drop handle kills the child whenever it is dropped.
pub(crate) struct CapacityGuard<'a> {
    pool: Option<&'a PoolInner>,
    /// Why the worker holding this slot left the pool, counted when the slot
    /// is released. `None` for a slot merely reserved for a spawn, where no
    /// worker has left.
    reason: Option<&'static str>,
}

impl<'a> CapacityGuard<'a> {
    /// Guards a slot reserved for a spawn that has not produced a worker yet.
    pub(crate) fn new(pool: &'a PoolInner) -> Self {
        Self {
            pool: Some(pool),
            reason: None,
        }
    }

    /// Guards the slot of a worker being torn down, counting its termination
    /// under `reason` when the slot is released.
    ///
    /// The count belongs to the guard rather than to the teardown code because
    /// every one of those sites awaits a reap: a caller dropping the turn
    /// future there still kills the worker and frees its slot, so the
    /// termination has to be counted on that path too.
    pub(crate) fn terminating(pool: &'a PoolInner, reason: &'static str) -> Self {
        Self {
            pool: Some(pool),
            reason: Some(reason),
        }
    }

    /// Refines the reason once the worker's exit status has classified it.
    /// The reason it replaces is what a cancelled teardown records instead.
    pub(crate) const fn set_reason(&mut self, reason: &'static str) {
        self.reason = Some(reason);
    }

    /// Keeps the slot reserved: the capacity was consumed by a live worker.
    pub(crate) fn disarm(mut self) {
        self.pool = None;
    }
}

impl Drop for CapacityGuard<'_> {
    fn drop(&mut self) {
        if let Some(pool) = self.pool {
            if let Some(reason) = self.reason {
                pool.count_termination(reason);
            }
            pool.release_capacity();
        }
    }
}

/// Saturates a worker count at the largest signed metric adjustment.
fn worker_count(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Locks a possibly poisoned mutex; a panic elsewhere must not stop us from
/// killing/reaping children.
pub(crate) fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
