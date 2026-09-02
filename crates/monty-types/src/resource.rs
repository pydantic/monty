//! Resource limits: the [`ResourceTracker`] used by the interpreter heap/VM
//! and its [`ResourceLimits`] configuration.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;
use std::{
    cell::Cell,
    error::Error,
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

// `std::time::Instant::now()` panics ("time not implemented on this platform")
// on `wasm32-unknown-unknown`, so any `max_duration` limit aborts there. Swap in
// `web_time::Instant` (a `performance.now()`-backed drop-in) only for that
// target; every other target (native, WASI) keeps std, so the `web-time`
// dependency is pulled in only where it's needed (see Cargo.toml).
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_time::Instant;

/// Exit code a worker uses when it exceeded its memory limit or the allocator
/// refused an allocation, so the parent can report `MemoryError` instead of an
/// unclassifiable `SIGABRT`.
///
/// `EX_DATAERR` from BSD `sysexits.h`. See <https://man.freebsd.org/cgi/man.cgi?query=sysexits>.
pub const OOM_EXIT_CODE: i32 = 65;
/// Allocator-backed live bytes requested through the global allocator
pub static LIVE_MEMORY: AtomicUsize = AtomicUsize::new(0);
/// The leanest the process has ever been at an arming point: what the worker
/// costs to exist, before any session ran.
pub static BASELINE_MEMORY: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Threshold in bytes above which `check_large_result` is called.
///
/// Operations that may produce results larger than this threshold (100KB) should call
/// `check_large_result` before performing the operation. This prevents DoS attacks
/// where operations like `2 ** 10_000_000` allocate huge amounts of memory before
/// the memory check can catch them.
pub const LARGE_RESULT_THRESHOLD: usize = 100_000;
/// Error returned when a resource limit is exceeded during execution.
///
/// This allows the sandbox to enforce strict limits on execution time
/// and memory usage.
///
/// All variants except `Recursion` are **uncatchable** inside the sandbox:
/// untrusted code must never intercept resource enforcement. `Recursion`
/// surfaces as a catchable `RecursionError`, matching CPython.
#[derive(Debug, Clone)]
pub enum ResourceError {
    /// Maximum execution time exceeded.
    Time { limit: Duration, elapsed: Duration },
    /// Maximum memory usage exceeded.
    Memory { limit: usize, used: usize },
    /// Maximum recursion depth exceeded.
    Recursion { limit: usize, depth: usize },
    /// Maximum number of host suspensions exceeded.
    ///
    /// `scope` names which of the two budgets ran out, so the message can say
    /// which limit the host should raise.
    Suspensions {
        limit: usize,
        count: usize,
        scope: SuspensionScope,
    },
}

/// Which suspension budget a [`ResourceError::Suspensions`] refers to.
///
/// The two budgets are counted separately: one run's suspensions are also
/// charged to the session total, so a session cap bites across feeds even
/// when no single run comes near the per-run cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspensionScope {
    /// The per-run budget, reset at the start of every feed/run.
    Run,
    /// The cumulative per-session budget, never reset.
    Session,
}

impl SuspensionScope {
    /// The limit's configuration name, quoted in the error message so a host
    /// knows which field to raise.
    fn limit_name(self) -> &'static str {
        match self {
            Self::Run => "max_suspensions_per_run",
            Self::Session => "max_total_suspensions",
        }
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Time { limit, elapsed } => {
                write!(f, "time limit exceeded: {elapsed:?} > {limit:?}")
            }
            Self::Memory { limit, used } => {
                write!(f, "memory limit exceeded: {used} bytes > {limit} bytes")
            }
            Self::Recursion { .. } => {
                write!(f, "maximum recursion depth exceeded")
            }
            Self::Suspensions { limit, count, scope } => {
                write!(
                    f,
                    "suspension limit exceeded: {count} > {limit} ({})",
                    scope.limit_name()
                )
            }
        }
    }
}

impl Error for ResourceError {}

/// Configuration for resource limits.
///
/// The time/memory/GC limits are optional — set to `None` to disable — but
/// recursion depth and per-run suspensions are always bounded (defaults
/// [`DEFAULT_MAX_RECURSION_DEPTH`] and
/// [`DEFAULT_MAX_SUSPENSIONS_PER_RUN`]): unbounded recursion would let
/// sandboxed code overflow the native stack and abort the process, and
/// unbounded suspensions would let it grow the *host's* memory without ever
/// allocating in the sandbox. Use `ResourceLimits::default()` for those
/// always-on defaults, or build custom limits with the builder pattern.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResourceLimits {
    /// Maximum execution time.
    pub max_duration: Option<Duration>,
    /// Maximum allocator-backed memory in bytes.
    ///
    /// Requires the executable to install and arm `monty-alloc`.
    pub max_memory: Option<usize>,
    /// Run garbage collection every N GC-tracked allocations.
    pub gc_interval: Option<usize>,
    /// Maximum recursion depth (function call stack depth).
    pub max_recursion_depth: usize,
    /// Maximum host suspensions in a single run (one feed and every resume
    /// that continues it).
    ///
    /// Each suspension is a host round trip the host retains state for, so
    /// this bounds host-side growth that no sandbox-side limit can see.
    pub max_suspensions_per_run: usize,
    /// Optional cumulative cap on host suspensions across the whole session.
    ///
    /// `None` (the default) bounds only each run. Set it to stop a caller
    /// sidestepping `max_suspensions_per_run` by feeding repeatedly.
    pub max_total_suspensions: Option<usize>,
}

/// Recommended maximum recursion depth if not otherwise specified.
pub const DEFAULT_MAX_RECURSION_DEPTH: usize = 1000;

/// Suspensions a single run may make if not otherwise specified.
///
/// Sized well above what host-call-heavy code legitimately does (tens to
/// hundreds per run) while still bounding the host-side state a refusal loop
/// can accumulate.
pub const DEFAULT_MAX_SUSPENSIONS_PER_RUN: usize = 10_000;

/// Creates a new ResourceLimits with all limits disabled, except the two that
/// are always on: recursion depth (1000) and per-run suspensions (10,000).
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_duration: None,
            max_memory: None,
            gc_interval: None,
            max_recursion_depth: DEFAULT_MAX_RECURSION_DEPTH,
            max_suspensions_per_run: DEFAULT_MAX_SUSPENSIONS_PER_RUN,
            max_total_suspensions: None,
        }
    }
}

impl ResourceLimits {
    /// Sets the maximum execution duration.
    #[must_use]
    pub fn max_duration(mut self, limit: Duration) -> Self {
        self.max_duration = Some(limit);
        self
    }

    /// Sets allocator-backed maximum memory usage in bytes.
    ///
    /// Requires the executable to install and arm `monty-alloc`; otherwise
    /// the limit is silently not enforced.
    #[must_use]
    pub fn max_memory(mut self, limit: usize) -> Self {
        self.max_memory = Some(limit);
        self
    }

    /// Sets the garbage collection interval (run GC every N GC-tracked allocations).
    #[must_use]
    pub fn gc_interval(mut self, interval: usize) -> Self {
        self.gc_interval = Some(interval);
        self
    }

    /// Sets the maximum recursion depth (function call stack depth).
    #[must_use]
    pub fn max_recursion_depth(mut self, limit: usize) -> Self {
        self.max_recursion_depth = limit;
        self
    }

    /// Sets the maximum host suspensions allowed in a single run.
    #[must_use]
    pub fn max_suspensions_per_run(mut self, limit: usize) -> Self {
        self.max_suspensions_per_run = limit;
        self
    }

    /// Sets a cumulative cap on host suspensions across the whole session.
    #[must_use]
    pub fn max_total_suspensions(mut self, limit: usize) -> Self {
        self.max_total_suspensions = Some(limit);
        self
    }
}

/// A resource tracker that enforces configurable limits.
///
/// Checks allocator-backed memory usage and tracks execution time, returning
/// errors when limits are exceeded. It also schedules garbage collection.
///
/// Uses `Cell` for mutable timing and recursion state behind shared references.
///
/// `max_duration` limits *cumulative execution time*: the clock runs only
/// while the VM is executing bytecode (between the outermost
/// `on_execution_start`/`on_execution_stop` pair) and is paused while
/// execution is suspended waiting on the host — external function calls,
/// OS callbacks — and between REPL feeds. The accumulated time is
/// serialized, so a deserialized session resumes its budget where it left
/// off rather than restarting from zero.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ResourceTracker {
    limits: ResourceLimits,
    /// Execution time accumulated by completed `on_execution_start`/`stop`
    /// windows. Serialized so time budgets survive dump/load. The serde
    /// default helps self-describing formats; postcard snapshots are
    /// positional, so older snapshot layouts still fail closed at decode.
    #[serde(default)]
    total_execution_time: Cell<Duration>,
    /// When the current execution window started; `None` while suspended or
    /// idle. Never serialized — a snapshot is by definition taken while not
    /// executing.
    #[serde(skip)]
    running_since: Cell<Option<Instant>>,
    /// Optional override applied on top of `limits.max_recursion_depth`.
    ///
    /// `None` (the default — also the value any pre-`test-hooks` snapshot
    /// deserializes to) means "no override, use the configured ceiling".
    /// `Some(N)` means "use `N` as the live recursion ceiling instead", and
    /// is only ever populated by
    /// [`lower_recursion_limit`](Self::lower_recursion_limit)
    /// under the `test-hooks` feature — `sys.setrecursionlimit` uses it to
    /// tighten the bound from Python code without escaping the
    /// host-configured ceiling.
    ///
    /// Modeled as an override rather than the live limit so adding this
    /// field doesn't break deserialization of snapshots produced before it
    /// existed (`#[serde(default)]` gives back the `None` fallback case).
    #[serde(default)]
    recursion_limit_override: Cell<Option<usize>>,
    /// Suspensions made by the run in progress, zeroed by
    /// [`on_run_start`](Self::on_run_start).
    ///
    /// Serialized because a session can be dumped *while suspended*: skipping
    /// it would hand a host that snapshots every suspension a free budget
    /// reset.
    #[serde(default)]
    run_suspensions: Cell<usize>,
    /// Suspensions made across the whole session, never reset.
    #[serde(default)]
    total_suspensions: Cell<usize>,
}

impl Default for ResourceTracker {
    fn default() -> Self {
        Self::new(ResourceLimits::default())
    }
}

impl ResourceTracker {
    /// Creates a new ResourceTracker with the given limits.
    ///
    /// The execution-time clock starts at zero and only runs while the VM
    /// executes, so the tracker can be created any amount of time before
    /// the first run without consuming the duration budget. A configured
    /// `max_memory` requires `monty-alloc` installed as the global allocator
    /// and armed via its `set_limit`; otherwise it is silently not enforced.
    #[must_use]
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            total_execution_time: Cell::new(Duration::ZERO),
            running_since: Cell::new(None),
            recursion_limit_override: Cell::new(None),
            run_suspensions: Cell::new(0),
            total_suspensions: Cell::new(0),
        }
    }

    /// Starts a fresh run, zeroing the per-run suspension budget.
    ///
    /// Called by the host-facing entry points that *begin* execution
    /// (`MontyRepl::feed_start` / `feed_run` / `call_function`,
    /// `MontyRun::start` / `run`) — never by a resume, which continues the
    /// run it is resuming. The session total is deliberately untouched.
    pub fn on_run_start(&self) {
        self.run_suspensions.set(0);
    }

    /// Charges one host suspension against both budgets.
    ///
    /// Called once per suspension the VM is about to hand the host, so a
    /// refused suspension never reaches the host at all — the host-side state
    /// this bounds is allocated by the round trip, not by the sandbox.
    pub fn record_suspension(&self) -> Result<(), ResourceError> {
        let run = self.run_suspensions.get().saturating_add(1);
        self.run_suspensions.set(run);
        let total = self.total_suspensions.get().saturating_add(1);
        self.total_suspensions.set(total);

        let run_limit = self.limits.max_suspensions_per_run;
        if run > run_limit {
            Err(ResourceError::Suspensions {
                limit: run_limit,
                count: run,
                scope: SuspensionScope::Run,
            })
        } else {
            match self.limits.max_total_suspensions {
                Some(limit) if total > limit => Err(ResourceError::Suspensions {
                    limit,
                    count: total,
                    scope: SuspensionScope::Session,
                }),
                _ => Ok(()),
            }
        }
    }

    /// Suspensions made by the run in progress.
    #[must_use]
    pub fn run_suspensions(&self) -> usize {
        self.run_suspensions.get()
    }

    /// Suspensions made across the whole session.
    #[must_use]
    pub fn total_suspensions(&self) -> usize {
        self.total_suspensions.get()
    }

    /// Returns the live recursion ceiling: the override if one is in effect,
    /// otherwise the configured `max_recursion_depth`.
    #[inline]
    fn active_recursion_limit(&self) -> usize {
        self.recursion_limit_override
            .get()
            .unwrap_or(self.limits.max_recursion_depth)
    }

    /// Returns the cumulative execution time: bytecode-execution wall time
    /// accumulated across runs/feeds, excluding time suspended on the host
    /// or idle between feeds. Includes the in-progress window if the VM is
    /// currently executing.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        let running = self.running_since.get().map_or(Duration::ZERO, |t| t.elapsed());
        self.total_execution_time.get() + running
    }

    /// Returns the configured maximum cumulative execution time, if any.
    #[must_use]
    pub fn max_duration(&self) -> Option<Duration> {
        self.limits.max_duration
    }

    /// Returns the configured memory budget, if any. Hosts that bound a worker
    /// process from outside the interpreter size that bound from this.
    #[must_use]
    pub fn max_memory(&self) -> Option<usize> {
        self.limits.max_memory
    }

    /// Returns whether the VM has a memory or time limit configured.
    #[must_use]
    pub fn has_memory_time_limit(&self) -> bool {
        self.limits.max_memory.is_some() || self.limits.max_duration.is_some()
    }

    /// Sets the maximum execution duration as a fresh budget from now,
    /// resetting the accumulated execution time to zero.
    ///
    /// This lets a host enforce a different (typically shorter) time limit
    /// for a resumed phase — e.g. allowing a long build phase, then giving
    /// `repr()` of the result only a few milliseconds. Time spent suspended
    /// in the host never counts toward the budget either way.
    pub fn set_max_duration(&mut self, duration: Duration) {
        self.limits.max_duration = Some(duration);
        self.total_execution_time.set(Duration::ZERO);
    }

    /// Checks whether one up-front allocation fits the memory budget.
    ///
    /// Use this before reserving a buffer that could cross both the soft and
    /// hard allocator limits before execution reaches another checkpoint.
    #[inline]
    pub fn check_allocation(&self, additional: usize) -> Result<(), ResourceError> {
        if let Some(limit) = self.limits.max_memory {
            let used = probe_memory().saturating_add(additional);
            if used > limit {
                return Err(ResourceError::Memory { limit, used });
            }
        }
        Ok(())
    }

    /// Called periodically to check allocator-backed memory and time limits.
    ///
    /// Returns `Ok(())` while configured limits are respected, or the relevant
    /// resource error once either limit is exceeded.
    ///
    /// Takes `&self` rather than `&mut self` because checking elapsed time is a
    /// read-only operation. This allows time checks in contexts that only have
    /// an immutable heap reference, such as `py_repr_fmt`.
    #[inline]
    pub fn check_memory_time(&self) -> Result<(), ResourceError> {
        if let Some(limit) = self.limits.max_memory {
            let used = probe_memory();
            if used > limit {
                return Err(ResourceError::Memory { limit, used });
            }
        }

        self.check_time()
    }

    /// Called periodically to check the time limit. Elapsed execution time is
    /// monotonic, so once the budget is exceeded every later call fails too.
    #[inline]
    pub fn check_time(&self) -> Result<(), ResourceError> {
        if let Some(max) = self.limits.max_duration {
            let elapsed = self.elapsed();
            if elapsed > max {
                return Err(ResourceError::Time { limit: max, elapsed });
            }
        }
        Ok(())
    }

    /// Items processed between full checks in amortized per-item Rust loops
    /// (see [`check_time_every`](Self::check_time_every)). A limit can be
    /// overshot by up to this many items' work before the next check — an
    /// accepted trade for cheap loops; the process-level hard limits backstop
    /// pathological cases.
    pub const LOOP_CHECK_INTERVAL: usize = 64;

    /// Amortized per-item time check for Rust-side loops: a full clock read
    /// once per [`LOOP_CHECK_INTERVAL`](Self::LOOP_CHECK_INTERVAL) calls,
    /// free otherwise. Key `i` on the loop's index or a monotonically
    /// increasing counter. Fires at the *end* of each block (`i % N == N-1`)
    /// so loops shorter than the interval pay no clock read at all — the VM
    /// dispatch checkpoint covers cadence between short calls.
    #[inline]
    pub fn check_time_every(&self, i: usize) -> Result<(), ResourceError> {
        if i % Self::LOOP_CHECK_INTERVAL == Self::LOOP_CHECK_INTERVAL - 1 {
            self.check_time()
        } else {
            Ok(())
        }
    }

    /// Amortized per-item memory + time check; the memory-probing sibling of
    /// [`check_time_every`](Self::check_time_every), for loops that allocate
    /// per item. Between full checks the allocator's hard limit still bounds
    /// runaway growth.
    #[inline]
    pub fn check_memory_time_every(&self, i: usize) -> Result<(), ResourceError> {
        if i % Self::LOOP_CHECK_INTERVAL == Self::LOOP_CHECK_INTERVAL - 1 {
            self.check_memory_time()
        } else {
            Ok(())
        }
    }

    /// Called before pushing a new call frame to check recursion depth.
    ///
    /// Returns `Ok(())` if within recursion limit, or `Err(ResourceError::Recursion)`
    /// if the limit would be exceeded. `current_depth` is the call stack depth
    /// before the new frame is pushed.
    #[inline]
    pub fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError> {
        let limit = self.active_recursion_limit();
        // current_depth is before push, so new depth would be current_depth + 1
        if current_depth >= limit {
            return Err(ResourceError::Recursion {
                limit,
                depth: current_depth + 1,
            });
        }
        Ok(())
    }

    /// Called before operations that may produce large results (>100KB).
    ///
    /// This allows pre-emptive rejection of operations like `2 ** 10_000_000`
    /// before the memory is actually allocated. The check only happens for
    /// estimated result sizes above `LARGE_RESULT_THRESHOLD` to avoid overhead
    /// on small operations.
    #[inline]
    pub fn check_large_result(&self, estimated_bytes: usize) -> Result<(), ResourceError> {
        self.check_allocation(estimated_bytes)
    }

    /// Returns the configured garbage collection interval, in GC-tracked
    /// allocations.
    ///
    /// The cycle collector runs at most once per `gc_interval` GC-tracked
    /// allocations, and additionally short-circuits when no cycle candidates
    /// are pending — so programs that never form cycles pay no collector
    /// cost regardless of their allocation rate. `None` tells the heap to use
    /// its built-in default scheduling threshold.
    #[must_use]
    #[inline]
    pub fn gc_interval(&self) -> Option<usize> {
        self.limits.gc_interval
    }

    /// Called when the VM enters its execution loop from a host boundary
    /// (`VM::run_external`), starting one execution window.
    ///
    /// Paired with [`on_execution_stop`](Self::on_execution_stop) and never
    /// nested — VM-internal re-entry (task switches, host-initiated function
    /// evaluation) uses the raw run loop, so its time falls inside the
    /// enclosing window. The execution-time clock runs between the pair; it is
    /// *not* running while execution is suspended waiting on the host
    /// (external function calls) or between feeds.
    pub fn on_execution_start(&self) {
        debug_assert!(
            self.running_since.get().is_none(),
            "nested on_execution_start: VM-internal re-entry must use the raw run loop, not run_external"
        );
        self.running_since.set(Some(Instant::now()));
    }

    /// Called when the VM leaves its execution loop — on completion, error,
    /// or suspension at an external call. See [`on_execution_start`](Self::on_execution_start).
    pub fn on_execution_stop(&self) {
        if let Some(started) = self.running_since.take() {
            self.total_execution_time
                .set(self.total_execution_time.get() + started.elapsed());
        }
    }

    /// Lowers the live recursion ceiling to `new_limit`, refusing to raise it.
    ///
    /// Exposed under the `test-hooks` feature so `sys.setrecursionlimit` can
    /// tighten the depth ceiling from inside fixture code. The constructed
    /// limit (`limits.max_recursion_depth`) acts as the hard upper bound —
    /// raising it would let sandboxed code escape the host-imposed safety
    /// bound. A `new_limit` above the active ceiling is rejected with
    /// `Err(current)`, which callers surface as a `ValueError` in the
    /// Python layer.
    #[cfg(feature = "test-hooks")]
    pub fn lower_recursion_limit(&self, new_limit: usize) -> Result<(), usize> {
        let limit = self.active_recursion_limit();
        if new_limit > limit {
            return Err(limit);
        }
        self.recursion_limit_override.set(Some(new_limit));
        Ok(())
    }
}

/// Returns memory used in bytes
fn probe_memory() -> usize {
    LIVE_MEMORY
        .load(Ordering::Relaxed)
        .saturating_sub(BASELINE_MEMORY.load(Ordering::Relaxed))
}
