//! The elastic worker pool: prewarming, checkout, replacement, teardown.

use std::{
    mem,
    pin::pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

use futures_util::future::join_all;
use monty_proto::pb;
use monty_types::ExecutionClock;
use tokio::{
    runtime::Handle,
    sync::{Notify, oneshot},
    time::{Instant, sleep_until, timeout, timeout_at},
};

#[cfg(feature = "telemetry")]
use crate::telemetry::TelemetryContext;
use crate::{
    MontyTransport, PoolConfig, PoolError,
    checkout::{Checkout, CheckoutOptions, Redial, ReplConfig, TurnRequest, request},
    remote::Remote,
    worker::Worker,
};

/// An elastic pool of `monty subprocess` workers.
///
/// `min_processes` workers spawn eagerly so the first checkout is fast, and
/// are replaced in the background whenever one is recycled, crashes or is
/// discarded; further workers spawn on demand up to `max_processes`. See the
/// crate docs for the full lifecycle.
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
    /// The remote transport's channel, present exactly when `config.transport`
    /// is remote; every stream is opened on it. Shared with refill tasks,
    /// which open the pre-warmed sessions.
    remote: Option<Arc<Remote>>,
    /// Guarded by a *synchronous* mutex: every critical section is short and
    /// await-free, and worker/checkout `Drop` impls must be able to release
    /// capacity without an async context.
    state: Mutex<PoolState>,
    /// Signalled whenever a worker returns to the idle queue or capacity is
    /// released, waking blocked `checkout` calls.
    available: Notify,
    /// Handed to refill tasks, which must not keep a dropped pool alive.
    this: Weak<Self>,
    /// Where refills are spawned: departures are recorded from `Drop` impls,
    /// which may run on a thread outside any runtime (e.g. Python's GC).
    runtime: Handle,
}

struct PoolState {
    idle: Vec<Worker>,
    /// Live workers: idle + checked out + currently being spawned.
    total: usize,
    /// Never reused, including when a spawn fails or is cancelled.
    next_id: u64,
    /// Set by [`Pool::close`] or dropping the [`Pool`]: stops refills, and a
    /// refill landing afterwards kills its worker.
    closed: bool,
    /// Consecutive failed refills, sizing the backoff before the next one.
    refill_failures: u32,
    /// Refills are suspended until this deadline; `Some` also means the one
    /// retry task is armed.
    retry_at: Option<Instant>,
    /// Refills in flight, counted in `total` but not yet in `idle`.
    refilling: usize,
    /// Remote checkouts waiting for a refill in flight instead of dialing, at
    /// most one per refill so a burst still dials in parallel.
    refill_claims: usize,
}

impl Pool {
    /// Creates the pool and eagerly starts `min_processes` workers, and with
    /// `pre_warm_wait` the `pre_warm` ready ones, failing fast if one cannot
    /// start. Must be called within a tokio runtime (worker process and pipe
    /// I/O is driven by the runtime).
    pub async fn new(config: PoolConfig) -> Result<Self, PoolError> {
        if config.min_processes > config.max_processes || config.max_processes == 0 {
            return Err(PoolError::Configuration(format!(
                "invalid pool size: min_processes={} max_processes={}",
                config.min_processes, config.max_processes
            )));
        }
        if config.pre_warm > config.max_processes {
            return Err(PoolError::Configuration(format!(
                "invalid pool size: pre_warm={} max_processes={}",
                config.pre_warm, config.max_processes
            )));
        }
        if config.execution_clock == ExecutionClock::Cpu && !config.transport.is_remote() {
            if !cfg!(target_os = "linux") {
                return Err(PoolError::Configuration(
                    "CPU execution clock is only supported for subprocess workers on Linux".to_owned(),
                ));
            }
            if config
                .request_timeout
                .is_none_or(|limit| Instant::now().checked_add(limit).is_none())
            {
                return Err(PoolError::Configuration(
                    "CPU execution clock requires a finite request_timeout".to_owned(),
                ));
            }
        }
        // a remote pool holds one channel for all its streams; nothing dials yet
        let remote = match &config.transport {
            MontyTransport::Remote(url) => Some(Arc::new(Remote::connect(
                url,
                config.request_timeout.unwrap_or(DEFAULT_DIAL_TIMEOUT),
                config.keep_alive,
                config.max_processes,
            )?)),
            MontyTransport::Subprocess(_) => None,
        };
        // the first workers start here, failing the pool if they cannot:
        // `min_processes` subprocesses, plus the rest of `pre_warm` under
        // `pre_warm_wait`, whose remainder otherwise starts in the background
        let floor = if config.transport.is_remote() {
            0
        } else {
            config.min_processes
        };
        let eager = if config.pre_warm_wait {
            floor.max(config.pre_warm)
        } else {
            floor
        };
        let mut idle = join_all((0..eager).map(|_| new_idle_worker(&config, remote.as_deref())))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for (worker, id) in idle.iter_mut().zip(1..) {
            worker.id = id;
        }
        let total = idle.len();
        let pool = Self {
            inner: Arc::new_cyclic(|this| PoolInner {
                config,
                remote,
                state: Mutex::new(PoolState {
                    idle,
                    total,
                    next_id: total as u64 + 1,
                    closed: false,
                    refill_failures: 0,
                    retry_at: None,
                    refilling: 0,
                    refill_claims: 0,
                }),
                available: Notify::new(),
                this: this.clone(),
                runtime: Handle::current(),
            }),
        };
        let ready: Vec<_> = lock_ignore_poison(&pool.inner.state)
            .idle
            .iter_mut()
            .filter_map(|worker| Some((worker.id, worker.take_ended()?)))
            .collect();
        for (id, ended) in ready {
            pool.inner.evict_when_ended(id, ended);
        }
        let workers = worker_count(total);
        pool.inner.record_worker_delta(workers, workers);
        if eager < pool.inner.config.pre_warm {
            pool.inner.replenish();
        }
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
    /// Takes an idle worker when one exists (remote: a pre-warmed session dialed
    /// with the same `options.metadata`), spawns or dials a new one (with
    /// `options.metadata`, preceded by the W3C trace headers of
    /// `options.telemetry`) while below `max_processes`, and otherwise waits
    /// up to `checkout_timeout` (forever when `None`) before failing with
    /// [`PoolError::Exhausted`].
    pub async fn checkout_with(&self, repl: &ReplConfig, options: CheckoutOptions) -> Result<Checkout, PoolError> {
        // the pool's own trace headers go ahead of the caller's, which stay
        // last-wins, and are no part of what a pre-warmed session is matched on
        #[cfg(feature = "telemetry")]
        let trace = options
            .telemetry
            .as_ref()
            .map_or_else(Vec::new, TelemetryContext::propagation_headers);
        #[cfg(not(feature = "telemetry"))]
        let trace = Vec::new();
        let worker = self.inner.acquire_worker(&trace, &options.metadata).await?;
        let config = &self.inner.config;
        let redial = (config.auto_resume && config.transport.is_remote()).then(|| Redial {
            repl: repl.clone(),
            metadata: options.metadata.clone(),
            #[cfg(feature = "telemetry")]
            telemetry: options.telemetry.clone(),
        });
        #[cfg(feature = "telemetry")]
        let worker = worker.with_adapter_context(options.telemetry);
        Ok(Checkout::create(worker, Arc::clone(&self.inner), repl, redial))
    }

    /// Asks idle subprocess workers to exit cleanly and reaps them, capping the
    /// wait per worker; idle remote sessions are half-closed instead. Sessions
    /// still checked out keep their workers until they finish, and are not
    /// replaced once they leave.
    /// Retained remote streams finish their bounded graceful close before this returns.
    ///
    /// Optional: dropping the pool kills idle workers instead, which is just
    /// as safe — this only trades a SIGKILL for a clean protocol goodbye.
    ///
    /// Telemetry exporter shutdown remains the configuring application's
    /// responsibility and should happen after checked-out sessions finish.
    pub async fn close(&self) {
        let idle = {
            let mut state = lock_ignore_poison(&self.inner.state);
            state.closed = true;
            mem::take(&mut state.idle)
        };
        self.inner.record_worker_delta(0, -worker_count(idle.len()));
        if self.inner.config.transport.is_remote() {
            // a pre-warmed session has no interpreter to say goodbye to, and a
            // request a stalled peer never reads must not hold up the close
            for worker in idle {
                self.inner.close_remote(worker, "closed");
            }
        } else {
            // Pair each removed worker with a capacity guard immediately: if this
            // future is dropped mid-close, every unreaped worker is killed by its
            // kill-on-drop handle and its slot released by the guard, instead of
            // leaking capacity the pool can never recover.
            let mut idle: Vec<_> = idle
                .into_iter()
                .map(|worker| (worker, CapacityGuard::terminating(&self.inner, "closed")))
                .collect();
            for (worker, _) in &mut idle {
                let _ = worker
                    .send(
                        TurnRequest::new(&request(pb::client_request::Kind::Shutdown(pb::Shutdown {}))),
                        None,
                    )
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
        if let Some(remote) = &self.inner.remote {
            remote.wait_closed(self.inner.config.max_processes).await;
        }
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

impl Drop for Pool {
    /// `Pool` is the only handle that can check out, so once it is gone live
    /// checkouts must not refill a pool nobody can use.
    fn drop(&mut self) {
        lock_ignore_poison(&self.inner.state).closed = true;
    }
}

/// How long [`Pool::close`] waits for a worker to exit on its own after the
/// `Shutdown` request before killing it.
const SHUTDOWN_EXIT_GRACE: Duration = Duration::from_millis(500);

/// Bounds opening a remote stream when `request_timeout` is `None`.
const DEFAULT_DIAL_TIMEOUT: Duration = Duration::from_secs(30);

impl PoolInner {
    /// Takes a worker — an idle one (remote: a pre-warmed session dialed with
    /// `metadata`), else a spawned or dialed one (with `trace` then `metadata`
    /// on its stream), waiting as capacity allows — and times that for
    /// `monty.pool.checkout.wait`.
    pub(crate) async fn acquire_worker(
        &self,
        trace: &[(String, String)],
        metadata: &[(String, String)],
    ) -> Result<Worker, PoolError> {
        #[cfg(feature = "telemetry")]
        let started = Instant::now();
        // set by the acquisition itself: only it can tell an idle reuse from a
        // spawn, or either from a wait for capacity
        let mut outcome = "idle";
        let worker = self.acquire_worker_inner(trace, metadata, &mut outcome).await;
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

    /// Reuses an idle worker, or spawns a local one or opens a remote stream
    /// (with `trace` then `metadata` on it), waiting as capacity allows, and
    /// reports through `outcome` how it got one.
    async fn acquire_worker_inner(
        &self,
        trace: &[(String, String)],
        metadata: &[(String, String)],
        outcome: &mut &'static str,
    ) -> Result<Worker, PoolError> {
        let remote = self.config.transport.is_remote();
        // held while this acquisition waits for a refill in flight
        let mut claim = None;
        // set once a pre-warmed session's metadata differed from this checkout's:
        // the next refill would differ too, so it dials rather than wait for one
        let mut mismatched = false;
        let mut waited = false;
        let deadline = self
            .config
            .checkout_timeout
            .and_then(|timeout| Instant::now().checked_add(timeout));
        loop {
            // Register for wakeups BEFORE checking state: a release landing
            // between the check and the await below is then still observed
            // (`enable` is what arms an un-polled `Notified`).
            let mut notified = pin!(self.available.notified());
            notified.as_mut().enable();
            // The guard's scope must close before any await below, so the
            // spawn/connect and the notified wait never hold the lock.
            drop(claim.take());
            let spawn = {
                let mut state = lock_ignore_poison(&self.state);
                let mut reused = None;
                let mut died_idle = 0;
                let mut removed_idle = 0;
                let mut stale = Vec::new();
                // discard workers that died while idle, and pre-warmed sessions
                // dialed with other metadata (a rotated token, say) — their
                // replacement is the spawn below or a background refill
                while let Some(mut worker) = state.idle.pop() {
                    removed_idle += 1;
                    if worker.is_dead() {
                        state.total -= 1;
                        died_idle += 1;
                        drop(worker); // kill-on-drop backstop; already dead
                    } else if remote && worker.dial_metadata != metadata {
                        // keeps its slot until the server has ended its stream
                        stale.push(worker);
                    } else {
                        reused = Some(worker);
                        break;
                    }
                }
                mismatched |= !stale.is_empty();
                if reused.is_some() {
                    // a ready worker a checkout takes proves refills healthy
                    state.refill_failures = 0;
                }
                // a worker already starting will be ready before a new one
                // would, so one checkout per refill waits for it
                let await_refill = reused.is_none() && !mismatched && state.refilling > state.refill_claims;
                if await_refill {
                    state.refill_claims += 1;
                }
                // reserve capacity before releasing the lock to spawn/connect
                let below_cap = reused.is_none() && !await_refill && state.total < self.config.max_processes;
                // at the cap, take over a stale session's slot and dial once its
                // stream has closed: freed instead, a refill could take the slot
                let inherited = (reused.is_none() && !await_refill && !below_cap)
                    .then(|| stale.pop())
                    .flatten();
                let id = if below_cap || inherited.is_some() {
                    state.total += usize::from(below_cap);
                    let id = state.next_id;
                    state.next_id = id.checked_add(1).expect("worker identity exhausted");
                    Some(id)
                } else {
                    None
                };
                drop(state); // never call into the host adapter under the lock
                if await_refill {
                    claim = Some(RefillClaim(self));
                }
                for _ in 0..died_idle {
                    self.count_termination("died_idle");
                }
                let gone = died_idle + stale.len() + usize::from(inherited.is_some());
                let closing = inherited.map(|worker| InheritedSlot::close(self, worker));
                for worker in stale {
                    self.close_remote(worker, "stale_metadata");
                }
                let spawned = i64::from(below_cap);
                self.record_worker_delta(spawned - worker_count(died_idle), -worker_count(removed_idle));
                if gone > 0 {
                    self.replenish();
                }
                if let Some(worker) = reused {
                    *outcome = if waited { "waited" } else { "idle" };
                    // `pre_warm` is a floor on ready workers, so taking one refills
                    self.replenish();
                    return Ok(worker);
                }
                id.map(|id| (id, closing))
            };
            if let Some((id, closing)) = spawn {
                *outcome = if waited { "waited" } else { "spawned" };
                if let Some(mut closing) = closing {
                    closing.wait().await;
                }
                // guard the reserved slot: a failed — or cancelled, for the
                // remote dial — spawn must release it or the pool shrinks
                let capacity = CapacityGuard::new(self);
                let dial = [trace, metadata].concat();
                let mut worker = Worker::new(&self.config, self.remote.as_deref(), &dial, deadline).await?;
                worker.id = id;
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
    #[cfg_attr(not(feature = "telemetry"), expect(clippy::unused_self))]
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
    #[cfg_attr(not(feature = "telemetry"), expect(clippy::unused_self))]
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
    /// the recycle limit, or it is a single-use remote stream).
    ///
    /// A remote stream keeps its slot until the server has ended it: a server
    /// still counting the stream would refuse a full pool's next checkout.
    pub(crate) fn release_worker(&self, worker: Worker) {
        if self.config.transport.is_remote() {
            self.close_remote(worker, "single_use");
        } else if self
            .config
            .max_checkouts_per_worker
            .is_some_and(|max| worker.checkouts_served >= max)
        {
            drop(worker); // kill (on drop) — reaped by tokio in the background
            self.count_termination("recycled");
            self.release_capacity();
        } else {
            lock_ignore_poison(&self.state).idle.push(worker);
            self.available.notify_one();
            self.record_worker_delta(0, 1);
        }
    }

    /// Half-closes a live remote worker's stream, counting it under `reason` and
    /// freeing its slot once the server has ended the stream (or the close gave up).
    fn close_remote(&self, mut worker: Worker, reason: &'static str) {
        let pool = self.this.clone();
        worker.close_transport(move || {
            if let Some(pool) = pool.upgrade() {
                pool.count_termination(reason);
                pool.release_capacity();
            }
        });
    }

    /// Drops ready session `id` once the server ends its stream, refilling
    /// rather than leaving the next checkout to find it dead; one that ends
    /// before [`READY_HEALTHY`] counts as a failed refill, so refills back off.
    /// A session a checkout took first is no longer idle, so its end is ignored.
    fn evict_when_ended(&self, id: u64, mut ended: oneshot::Receiver<()>) {
        let pool = self.this.clone();
        self.runtime.spawn(async move {
            let healthy = timeout(READY_HEALTHY, &mut ended).await.is_err();
            if healthy {
                if let Some(pool) = pool.upgrade() {
                    lock_ignore_poison(&pool.state).refill_failures = 0;
                }
                let _ = ended.await;
            }
            let Some(pool) = pool.upgrade() else { return };
            let worker = {
                let mut state = lock_ignore_poison(&pool.state);
                let index = state.idle.iter().position(|worker| worker.id == id);
                index.map(|index| state.idle.remove(index))
            };
            if let Some(worker) = worker {
                drop(worker);
                pool.record_worker_delta(0, -1);
                if healthy {
                    pool.count_termination("died_idle");
                    pool.release_capacity();
                } else {
                    pool.refill_failed("died_idle");
                }
            }
        });
    }

    /// Records the death/retirement of a worker, freeing capacity for a
    /// future spawn, and refills towards `pre_warm` and, for a subprocess
    /// pool, `min_processes`.
    pub(crate) fn release_capacity(&self) {
        lock_ignore_poison(&self.state).total -= 1;
        self.available.notify_one();
        self.record_worker_delta(-1, 0);
        self.replenish();
    }

    /// Starts background workers until `pre_warm` are ready and, for a
    /// subprocess pool, `min_processes` are live, reserving their capacity first
    /// so a racing checkout can never overshoot `max_processes`. Does nothing
    /// once closed or while refills back off.
    fn replenish(&self) {
        let (wanted, first_id) = {
            let mut state = lock_ignore_poison(&self.state);
            if state.closed || state.retry_at.is_some() {
                (0, 0)
            } else {
                // `pre_warm` is a floor on ready workers; a subprocess pool's
                // `min_processes` is also one on live workers
                let ready = self.config.pre_warm.saturating_sub(state.idle.len() + state.refilling);
                let live = if self.config.transport.is_remote() {
                    0
                } else {
                    self.config.min_processes.saturating_sub(state.total)
                };
                let wanted = ready
                    .max(live)
                    .min(self.config.max_processes.saturating_sub(state.total));
                state.total += wanted;
                state.refilling += wanted;
                let first_id = state.next_id;
                state.next_id = first_id.checked_add(wanted as u64).expect("worker identity exhausted");
                (wanted, first_id)
            }
        };
        if wanted > 0 {
            self.record_worker_delta(worker_count(wanted), 0);
            for id in first_id..first_id + wanted as u64 {
                // a task the runtime drops mid-spawn (at its shutdown) loses its
                // slot, which only matters to a pool that is going away anyway
                self.runtime.spawn(refill_one(
                    self.this.clone(),
                    self.config.clone(),
                    self.remote.clone(),
                    id,
                ));
            }
        }
    }

    /// Records a failed refill and arms the one retry task, whose delay
    /// doubles with each consecutive failure so a broken binary cannot become
    /// a spawn loop. Must run before the failed slot is released, since that
    /// release would otherwise start the next refill at once.
    fn refill_failed(&self, reason: &'static str) {
        let retry_at = {
            let mut state = lock_ignore_poison(&self.state);
            state.refill_failures = state.refill_failures.saturating_add(1);
            if state.retry_at.is_some() {
                None
            } else {
                let at = Instant::now() + refill_backoff(state.refill_failures);
                state.retry_at = Some(at);
                Some(at)
            }
        };
        if let Some(at) = retry_at {
            let pool = self.this.clone();
            self.runtime.spawn(async move {
                sleep_until(at).await;
                if let Some(pool) = pool.upgrade() {
                    lock_ignore_poison(&pool.state).retry_at = None;
                    pool.replenish();
                }
            });
        }
        self.count_termination(reason);
        self.release_capacity();
    }
}

/// Spawns worker `id` (or opens a pre-warmed remote session) into a slot
/// [`PoolInner::replenish`] reserved.
///
/// Holds only a `Weak` across the spawn, so an in-flight refill never keeps a
/// dropped pool alive; its worker is then killed on drop.
async fn refill_one(pool: Weak<PoolInner>, config: PoolConfig, remote: Option<Arc<Remote>>, id: u64) {
    let mut slot = RefillSlot {
        pool: pool.clone(),
        filled: false,
    };
    let worker = new_idle_worker(&config, remote.as_deref()).await;
    let Some(pool) = pool.upgrade() else { return };
    let Ok(mut worker) = worker else { return };
    let mut state = lock_ignore_poison(&pool.state);
    // a closed pool kills a late worker, as `slot` then records
    if !state.closed {
        worker.id = id;
        let ended = worker.take_ended();
        state.refilling -= 1;
        // a remote session that opens may still be ended at once: it proves
        // the server healthy only by staying up (see `evict_when_ended`)
        if remote.is_none() {
            state.refill_failures = 0;
        }
        state.idle.push(worker);
        drop(state);
        slot.filled = true;
        if let Some(ended) = ended {
            pool.evict_when_ended(id, ended);
        }
        pool.available.notify_one();
        pool.record_worker_delta(0, 1);
    }
}

/// The slot a refill holds, given back when the refill ends without a worker:
/// it failed, the pool closed, or its task panicked or was dropped. A refill
/// that never reported would strand the checkout waiting for it.
struct RefillSlot {
    pool: Weak<PoolInner>,
    filled: bool,
}

impl Drop for RefillSlot {
    fn drop(&mut self) {
        if !self.filled
            && let Some(pool) = self.pool.upgrade()
        {
            let closed = {
                let mut state = lock_ignore_poison(&pool.state);
                state.refilling -= 1;
                state.closed
            };
            // a closed pool will not refill again, so it must not arm a retry
            if closed {
                pool.count_termination("closed");
                pool.release_capacity();
            } else {
                pool.refill_failed("refill_failed");
            }
        }
    }
}

/// Starts a worker for the idle queue: a subprocess, or a pre-warmed remote
/// session dialed with [`PoolConfig::pre_warm_metadata`], which it records.
async fn new_idle_worker(config: &PoolConfig, remote: Option<&Remote>) -> Result<Worker, PoolError> {
    let metadata = match (remote, &config.pre_warm_metadata) {
        // bounded like the dial it precedes, so a function that never answers
        // cannot hold a slot forever
        (Some(_), Some(metadata)) => {
            let limit = config.request_timeout.unwrap_or(DEFAULT_DIAL_TIMEOUT);
            timeout(limit, metadata.call())
                .await
                .map_err(|_| PoolError::Configuration(format!("pre_warm_metadata did not answer within {limit:?}")))??
        }
        _ => Vec::new(),
    };
    let mut worker = Worker::new(config, remote, &metadata, None).await?;
    worker.dial_metadata = metadata;
    Ok(worker)
}

/// One checkout's claim on a refill in flight, released when it stops waiting.
struct RefillClaim<'a>(&'a PoolInner);

impl Drop for RefillClaim<'_> {
    fn drop(&mut self) {
        lock_ignore_poison(&self.0.state).refill_claims -= 1;
    }
}

/// Delay before the refill following `failures` consecutive failed ones.
fn refill_backoff(failures: u32) -> Duration {
    let factor = 1u32.checked_shl(failures.saturating_sub(1)).unwrap_or(u32::MAX);
    REFILL_BACKOFF_MIN.saturating_mul(factor).min(REFILL_BACKOFF_MAX)
}

/// How long a ready remote session stays up before its server counts as
/// healthy, resetting the refill backoff.
const READY_HEALTHY: Duration = Duration::from_secs(10);

/// First delay after a failed refill.
const REFILL_BACKOFF_MIN: Duration = Duration::from_millis(100);
/// Ceiling on the doubling refill backoff.
const REFILL_BACKOFF_MAX: Duration = Duration::from_secs(30);

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

/// The slot of a stale pre-warmed session that a checkout takes over, held
/// until the server has ended that session's stream. Dropped before then (the
/// checkout was cancelled), the slot is freed once the stream has closed.
struct InheritedSlot<'a> {
    pool: &'a PoolInner,
    closed: Option<oneshot::Receiver<()>>,
}

impl<'a> InheritedSlot<'a> {
    /// Half-closes `worker`'s stream, keeping its slot for the caller.
    fn close(pool: &'a PoolInner, mut worker: Worker) -> Self {
        pool.count_termination("stale_metadata");
        let (tx, closed) = oneshot::channel();
        let weak = pool.this.clone();
        worker.close_transport(move || {
            // fails once the receiver is closed: no checkout will use the slot
            if tx.send(()).is_err()
                && let Some(pool) = weak.upgrade()
            {
                pool.release_capacity();
            }
        });
        Self {
            pool,
            closed: Some(closed),
        }
    }

    /// Waits for the stream to close; the slot is then the caller's.
    async fn wait(&mut self) {
        if let Some(closed) = &mut self.closed {
            // an error means the callback was dropped uncalled, freeing nothing
            let _ = closed.await;
        }
        self.closed = None;
    }
}

impl Drop for InheritedSlot<'_> {
    fn drop(&mut self) {
        if let Some(mut closed) = self.closed.take() {
            closed.close();
            // the stream closed before the receiver did, so the slot is ours to free
            if closed.try_recv().is_ok() {
                self.pool.release_capacity();
            }
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
