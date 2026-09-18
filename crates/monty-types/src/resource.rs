//! Resource limits: the [`ResourceTracker`] used by the interpreter heap/VM
//! and its [`ResourceLimits`] configuration.

#[cfg(target_arch = "wasm32")]
use std::hint;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
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
// on `wasm32-unknown-unknown`, so any duration limit aborts there. Swap in
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

/// Headroom for exception machinery and work between interpreter checkpoints.
const MEMORY_LIMIT_HEADROOM: usize = 4 * 1024 * 1024;
/// Extra headroom for type-checker stubs and caches outside Python execution.
const TYPE_CHECK_MEMORY_LIMIT_HEADROOM: usize = 32 * 1024 * 1024;

/// Converts an interpreter soft memory limit into a worker allocator budget.
///
/// The returned budget includes operational headroom but remains relative to
/// the worker baseline, which `monty-alloc` adds when arming its hard ceiling.
#[must_use]
pub fn memory_limit_with_headroom(max_memory: Option<usize>, type_check: bool) -> Option<usize> {
    max_memory.map(|bytes| {
        let headroom = if type_check {
            TYPE_CHECK_MEMORY_LIMIT_HEADROOM
        } else {
            MEMORY_LIMIT_HEADROOM
        };
        bytes.saturating_add(headroom)
    })
}

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
    /// One of the two execution-time budgets was exceeded; `scope` says which.
    Time {
        scope: TimeLimitScope,
        limit: Duration,
        elapsed: Duration,
    },
    /// Maximum memory usage exceeded.
    Memory { limit: usize, used: usize },
    /// Maximum recursion depth exceeded.
    Recursion { limit: usize, depth: usize },
}

/// Which of the two nested execution-time budgets a [`ResourceError::Time`]
/// refers to.
///
/// Both read the same clock and differ only in when they are reset, so a
/// turn's time is charged to its feed as well: `Feed >= Turn` always holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeLimitScope {
    /// [`ResourceLimits::max_feed_duration`] — reset at each feed.
    Feed,
    /// [`ResourceLimits::max_turn_duration`] — reset at each feed and each
    /// resume.
    Turn,
}

impl TimeLimitScope {
    /// The word naming this scope in an error message.
    fn prefix(self) -> &'static str {
        match self {
            Self::Feed => "feed ",
            Self::Turn => "turn ",
        }
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Time { scope, limit, elapsed } => {
                write!(f, "{}time limit exceeded: {elapsed:?} > {limit:?}", scope.prefix())
            }
            Self::Memory { limit, used } => {
                write!(f, "memory limit exceeded: {used} bytes > {limit} bytes")
            }
            Self::Recursion { .. } => {
                write!(f, "maximum recursion depth exceeded")
            }
        }
    }
}

impl Error for ResourceError {}

/// Configuration for resource limits.
///
/// The time/memory/GC limits are optional — set to `None` to disable — but
/// recursion depth and the suspension budget are always bounded (defaults
/// [`DEFAULT_MAX_RECURSION_DEPTH`] and [`DEFAULT_MAX_SUSPENSIONS`]): unbounded
/// recursion would let sandboxed code overflow the native stack and abort the
/// process, and unbounded suspensions would let it loop on host calls. Use
/// `ResourceLimits::default()` for the recursion-only defaults, or build
/// custom limits with the builder pattern.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResourceLimits {
    /// Maximum execution time for a single feed (`feed_start`, `feed_run` or
    /// `call_function`), summed over the turns it takes and excluding time
    /// suspended on the host. Bounds one snippet, not the session.
    ///
    /// Defaulted on deserialization so limits written by a build without this
    /// field still load from a self-describing format; a postcard dump of an
    /// older layout is rejected by `DUMP_VERSION` instead.
    #[serde(default)]
    pub max_feed_duration: Option<Duration>,
    /// Maximum execution time for a single host turn, reset at each feed and
    /// each resume. Bounds the stretch of sandbox code between two host round
    /// trips, so a host can bound its own response time per call.
    ///
    /// Defaulted on deserialization like
    /// [`max_feed_duration`](Self::max_feed_duration).
    #[serde(default)]
    pub max_turn_duration: Option<Duration>,
    /// Maximum allocator-backed memory in bytes.
    ///
    /// Requires the executable to install and arm `monty-alloc`.
    pub max_memory: Option<usize>,
    /// Run garbage collection every N GC-tracked allocations.
    pub gc_interval: Option<usize>,
    /// Maximum recursion depth (function call stack depth).
    pub max_recursion_depth: usize,
    /// Maximum suspensions the host may service (default
    /// [`DEFAULT_MAX_SUSPENSIONS`]; always bounded, like recursion depth).
    /// The interpreter only stores this limit; hosts must enforce it.
    pub max_suspensions: usize,
}

/// Recommended maximum recursion depth if not otherwise specified.
pub const DEFAULT_MAX_RECURSION_DEPTH: usize = 1000;

/// Maximum suspensions a host services per session if not otherwise
/// specified: a backstop against a sandbox looping on host calls while the
/// execution clock is paused.
pub const DEFAULT_MAX_SUSPENSIONS: usize = 1000;

/// Creates a new ResourceLimits with all limits disabled, except max recursion
/// depth and max suspensions, which are set to 1000.
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_feed_duration: None,
            max_turn_duration: None,
            max_memory: None,
            gc_interval: None,
            max_recursion_depth: DEFAULT_MAX_RECURSION_DEPTH,
            max_suspensions: DEFAULT_MAX_SUSPENSIONS,
        }
    }
}

impl ResourceLimits {
    /// Sets the maximum execution duration for any single feed.
    #[must_use]
    pub fn max_feed_duration(mut self, limit: Duration) -> Self {
        self.max_feed_duration = Some(limit);
        self
    }

    /// Sets the maximum execution duration for any single host turn.
    #[must_use]
    pub fn max_turn_duration(mut self, limit: Duration) -> Self {
        self.max_turn_duration = Some(limit);
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

    /// Sets the host-enforced maximum number of suspensions.
    #[must_use]
    pub fn max_suspensions(mut self, limit: usize) -> Self {
        self.max_suspensions = limit;
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
/// The two duration limits share one *execution time* clock: it runs only
/// between the outermost `on_execution_start`/`on_execution_stop` pair, so it
/// is paused while suspended on the host and between REPL feeds. They differ
/// only in when their accumulator resets — at
/// [`on_feed_start`](Self::on_feed_start), at [`on_turn_start`](Self::on_turn_start)
/// — and the feed total is serialized, so a session loaded mid-feed resumes
/// that budget rather than restarting from zero.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ResourceTracker {
    limits: ResourceLimits,
    /// Execution time accumulated by completed `on_execution_start`/`stop`
    /// windows. Bounds nothing — it is what [`elapsed`](Self::elapsed) reports
    /// to hosts for telemetry — but is serialized so a loaded session keeps
    /// counting from where it left off. The serde default is for
    /// self-describing formats; a postcard dump of an older layout is rejected
    /// by `DUMP_VERSION` instead.
    #[serde(default)]
    total_execution_time: Cell<Duration>,
    /// Execution time accumulated since the last [`on_feed_start`](Self::on_feed_start).
    /// Serialized like `total_execution_time`: a dump taken mid-feed resumes
    /// that feed, so its budget must survive the round trip.
    #[serde(default)]
    feed_execution_time: Cell<Duration>,
    /// Execution time accumulated since the last [`on_turn_start`](Self::on_turn_start).
    /// Not serialized — a dump is taken between turns, and the resume that
    /// loads it starts a fresh turn.
    #[serde(skip)]
    turn_execution_time: Cell<Duration>,
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
    /// the first run without consuming a duration budget. A configured
    /// `max_memory` requires `monty-alloc` installed as the global allocator
    /// and armed via `set_hard_limit(memory_limit_with_headroom(...))`;
    /// otherwise it is silently not enforced.
    #[must_use]
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            total_execution_time: Cell::new(Duration::ZERO),
            feed_execution_time: Cell::new(Duration::ZERO),
            turn_execution_time: Cell::new(Duration::ZERO),
            running_since: Cell::new(None),
            recursion_limit_override: Cell::new(None),
        }
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
    ///
    /// Nothing is bounded by this — it is reported to the host for telemetry.
    /// The budgets are [`feed_elapsed`](Self::feed_elapsed) and
    /// [`turn_elapsed`](Self::turn_elapsed).
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.total_execution_time.get() + self.running_window()
    }

    /// Returns the execution time consumed by the current feed; see
    /// [`elapsed`](Self::elapsed), which reports the same clock over the
    /// whole session.
    #[must_use]
    pub fn feed_elapsed(&self) -> Duration {
        self.feed_execution_time.get() + self.running_window()
    }

    /// Returns the execution time consumed by the current host turn; see
    /// [`elapsed`](Self::elapsed), which reports the same clock over the
    /// whole session.
    #[must_use]
    pub fn turn_elapsed(&self) -> Duration {
        self.turn_execution_time.get() + self.running_window()
    }

    /// The in-progress execution window, or zero when not executing. Read
    /// once per check so both budgets are compared against one clock reading
    /// rather than two.
    #[inline]
    fn running_window(&self) -> Duration {
        self.running_since.get().map_or(Duration::ZERO, |t| t.elapsed())
    }

    /// Returns the configured per-feed execution time limit, if any.
    #[must_use]
    pub fn max_feed_duration(&self) -> Option<Duration> {
        self.limits.max_feed_duration
    }

    /// Returns the configured per-turn execution time limit, if any.
    #[must_use]
    pub fn max_turn_duration(&self) -> Option<Duration> {
        self.limits.max_turn_duration
    }

    /// Returns the configured memory budget, if any. Hosts that bound a worker
    /// process from outside the interpreter size that bound from this.
    #[must_use]
    pub fn max_memory(&self) -> Option<usize> {
        self.limits.max_memory
    }

    /// Returns the host-enforced suspension budget (default
    /// [`DEFAULT_MAX_SUSPENSIONS`]; never unlimited).
    #[must_use]
    pub fn max_suspensions(&self) -> usize {
        self.limits.max_suspensions
    }

    /// Returns whether the VM has a memory or time limit configured.
    #[must_use]
    pub fn has_memory_time_limit(&self) -> bool {
        self.limits.max_memory.is_some() || self.has_time_limit()
    }

    /// Returns whether either execution-time budget is configured.
    ///
    /// Public so callers that pay for finer-grained clock polling only when a
    /// budget exists (`fstring`'s incremental large-result path) can ask.
    #[must_use]
    #[inline]
    pub fn has_time_limit(&self) -> bool {
        self.limits.max_feed_duration.is_some() || self.limits.max_turn_duration.is_some()
    }

    /// Sets the per-feed execution limit as a fresh budget from now, resetting
    /// the feed (and so the turn) clock.
    ///
    /// This lets a host enforce a different (typically shorter) time limit
    /// for a resumed phase — e.g. allowing a long build phase, then giving
    /// `repr()` of the result only a few milliseconds. Time spent suspended
    /// in the host never counts toward the budget either way.
    pub fn set_max_feed_duration(&mut self, duration: Duration) {
        self.limits.max_feed_duration = Some(duration);
        self.on_feed_start();
    }

    /// Sets the per-turn execution limit as a fresh budget from now, resetting
    /// the turn clock.
    pub fn set_max_turn_duration(&mut self, duration: Duration) {
        self.limits.max_turn_duration = Some(duration);
        self.on_turn_start();
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

    /// Called periodically to check both execution-time budgets.
    ///
    /// Each clock is monotonic within its scope, so once a budget is exceeded
    /// every later call in that scope fails too. The feed budget is tested
    /// first, since it is the one a new turn cannot recover from.
    #[inline]
    pub fn check_time(&self) -> Result<(), ResourceError> {
        if !self.has_time_limit() {
            return Ok(());
        }
        // One clock reading shared by both comparisons.
        let running = self.running_window();
        check_budget(
            TimeLimitScope::Feed,
            self.limits.max_feed_duration,
            self.feed_execution_time.get() + running,
        )
        .and_then(|()| {
            check_budget(
                TimeLimitScope::Turn,
                self.limits.max_turn_duration,
                self.turn_execution_time.get() + running,
            )
        })
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

    /// Preflights the reallocation that pushing one more element onto a dense
    /// buffer causes; a push that fits the existing capacity costs nothing.
    ///
    /// A `Vec` charges its whole doubling in one allocation, so a push
    /// straddling the soft limit can land past the allocator's fixed
    /// hard-limit headroom, killing the worker with no checkpoint in between
    /// at which to raise `MemoryError`. Only for the one-push shape: a bulk
    /// reservation needs [`ResourceTracker::check_allocation`] sized for the
    /// whole result, since preflighting less than the final buffer — one
    /// operand of a merge, say — leaves the same window open.
    #[inline]
    pub fn check_growth(&self, len: usize, capacity: usize, elem_size: usize) -> Result<(), ResourceError> {
        self.check_pending_allocation(Self::growth_bytes(len, capacity, elem_size))
    }

    /// The bytes [`check_growth`](Self::check_growth) would preflight, or zero
    /// if the push allocates nothing.
    ///
    /// Split out for containers that grow two buffers on one insertion, such
    /// as a dict's entry vector and index table: checking each increment alone
    /// passes both while their sum clears the headroom, so the caller sums
    /// them and passes the total to
    /// [`check_pending_allocation`](Self::check_pending_allocation).
    #[inline]
    #[must_use]
    pub fn growth_bytes(len: usize, capacity: usize, elem_size: usize) -> usize {
        if len < capacity {
            0
        } else {
            // A buffer growing from nothing jumps straight to
            // `RawVec::MIN_NON_ZERO_CAP`, which is also what stops the
            // increment coming out as zero.
            let min_non_zero_capacity = match elem_size {
                1 => 8,
                2..=1024 => 4,
                _ => 1,
            };
            let new_capacity = capacity
                .saturating_mul(2)
                .max(len.saturating_add(1))
                .max(min_non_zero_capacity);
            new_capacity.saturating_sub(capacity).saturating_mul(elem_size)
        }
    }

    /// [`check_allocation`](Self::check_allocation) for preflights whose
    /// increment may be zero: a push that allocates nothing must not pay for
    /// the usage probe.
    #[inline]
    pub fn check_pending_allocation(&self, additional: usize) -> Result<(), ResourceError> {
        if additional == 0 {
            Ok(())
        } else {
            self.check_allocation(additional)
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
    /// estimated result sizes above [`LARGE_RESULT_THRESHOLD`] to avoid overhead
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
            let window = started.elapsed();
            for clock in [
                &self.total_execution_time,
                &self.feed_execution_time,
                &self.turn_execution_time,
            ] {
                clock.set(clock.get() + window);
            }
        }
    }

    /// Called when the host begins a new feed, resetting the `max_feed_duration`
    /// budget (and, since a feed opens a turn, the `max_turn_duration` one).
    ///
    /// A feed spans every turn its snippet takes, so this must fire only at
    /// the snippet's first turn — not at the resumes that continue it.
    pub fn on_feed_start(&self) {
        self.feed_execution_time.set(Duration::ZERO);
        self.on_turn_start();
    }

    /// Called when the host hands control back to the interpreter — at a feed
    /// or a resume — resetting the `max_turn_duration` budget.
    ///
    /// Continuations the VM resolves without the host (an already-settled
    /// future, a task switch) stay inside the turn that started them.
    pub fn on_turn_start(&self) {
        self.turn_execution_time.set(Duration::ZERO);
    }

    /// Blocks for `duration` with the execution clock stopped: a sleep the
    /// sandbox serves itself (`time.sleep`, a sandbox `asyncio.sleep` timer)
    /// counts against neither `max_feed_duration` nor the host's suspension budget,
    /// exactly as a host-performed one would not. The clock restarts only if
    /// it was running, so this is safe outside an execution window too.
    ///
    /// The one place the interpreter waits, so a platform without a blocking
    /// sleep has a single function to adapt (see [`block_for`]).
    pub fn sandbox_sleep(&self, duration: Duration) {
        let was_running = self.running_since.get().is_some();
        self.on_execution_stop();
        block_for(duration);
        if was_running {
            self.on_execution_start();
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

/// Reports `elapsed` against one optional budget, naming `scope` in the error.
#[inline]
fn check_budget(scope: TimeLimitScope, limit: Option<Duration>, elapsed: Duration) -> Result<(), ResourceError> {
    match limit {
        Some(limit) if elapsed > limit => Err(ResourceError::Time { scope, limit, elapsed }),
        _ => Ok(()),
    }
}

/// Returns memory used in bytes
fn probe_memory() -> usize {
    LIVE_MEMORY
        .load(Ordering::Relaxed)
        .saturating_sub(BASELINE_MEMORY.load(Ordering::Relaxed))
}

/// Blocks the calling thread for `duration`.
#[cfg(not(target_arch = "wasm32"))]
fn block_for(duration: Duration) {
    thread::sleep(duration);
}

/// Blocks for `duration` on wasm, where `std::thread::sleep` needs
/// `wasi:io/poll` and a browser host can only serve that asynchronously — a
/// synchronous component call cannot wait on it. The monotonic clock is
/// served synchronously everywhere, so the wait spins on it instead; the
/// worker is idle during a sleep anyway, and the cap on each sleep bounds the
/// spin.
#[cfg(target_arch = "wasm32")]
fn block_for(duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        hint::spin_loop();
    }
}
