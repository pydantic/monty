//! Resource limits: the [`ResourceTracker`] trait the interpreter heap/VM
//! are generic over, plus the stock [`NoLimitTracker`]/[`LimitedTracker`]
//! implementations and their [`ResourceLimits`] configuration.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;
use std::{cell::Cell, error::Error, fmt, time::Duration};

// `std::time::Instant::now()` panics ("time not implemented on this platform")
// on `wasm32-unknown-unknown`, so any `max_duration` limit aborts there. Swap in
// `web_time::Instant` (a `performance.now()`-backed drop-in) only for that
// target; every other target (native, WASI) keeps std, so the `web-time`
// dependency is pulled in only where it's needed (see Cargo.toml).
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_time::Instant;

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
        }
    }
}

impl Error for ResourceError {}
/// Trait for tracking resource usage and scheduling garbage collection.
///
/// Implementations can enforce limits on time and memory, as well as
/// schedule periodic garbage collection.
///
/// All implementations should eventually trigger garbage collection to handle
/// reference cycles. [`gc_interval`](Self::gc_interval) controls *frequency*,
/// not whether GC runs at all.
pub trait ResourceTracker: fmt::Debug {
    /// Called when memory is freed (during dec_ref or garbage collection).
    ///
    /// # Arguments
    /// * `size` - Size in bytes of the freed allocation
    fn on_free(&self, get_size: impl FnOnce() -> usize);

    /// Called periodically (at statement boundaries) to check time limits.
    ///
    /// Returns `Ok(())` if within time limit, or `Err(ResourceError::Time)`
    /// if the limit is exceeded.
    ///
    /// Takes `&self` rather than `&mut self` because checking elapsed time is a
    /// read-only operation. This allows time checks in contexts that only have
    /// an immutable heap reference, such as `py_repr_fmt`.
    fn check_time(&self) -> Result<(), ResourceError>;

    /// Called before pushing a new call frame to check recursion depth.
    ///
    /// Returns `Ok(())` if within recursion limit, or `Err(ResourceError::Recursion)`
    /// if the limit would be exceeded.
    ///
    /// # Arguments
    /// * `current_depth` - Current call stack depth (before the new frame is pushed)
    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError>;

    /// Called before operations that may produce large results (>100KB).
    ///
    /// This allows pre-emptive rejection of operations like `2 ** 10_000_000`
    /// before the memory is actually allocated. The check only happens for
    /// estimated result sizes above `LARGE_RESULT_THRESHOLD` to avoid overhead
    /// on small operations.
    ///
    /// # Arguments
    /// * `estimated_bytes` - Approximate size of the result in bytes
    ///
    /// Returns `Ok(())` to allow the operation, or `Err(ResourceError)` to reject.
    fn check_large_result(&self, estimated_bytes: usize) -> Result<(), ResourceError>;

    /// Called before tracked memory grows: a new heap allocation, in-place
    /// container growth (`list.append`, `dict[k] = v`), or a `StringBuilder`
    /// reservation.
    ///
    /// Returns `Ok(())` if the growth should proceed, or `Err(ResourceError)`
    /// if a limit would be exceeded. Balanced by [`on_free`](Self::on_free):
    /// entry release reads `py_estimate_size()`, which includes in-place growth.
    ///
    /// # Arguments
    /// * `get_additional` - Lazily computes the approximate growth in bytes;
    ///   implementations that ignore size (`NoLimitTracker`, or `LimitedTracker`
    ///   with no memory limit) never pay for it
    fn on_grow(&self, get_additional: impl FnOnce() -> usize) -> Result<(), ResourceError>;

    /// Returns the configured garbage collection interval, in GC-tracked
    /// allocations.
    ///
    /// The cycle collector runs at most once per `gc_interval` GC-tracked
    /// allocations, and additionally short-circuits when no cycle candidates
    /// are pending — so programs that never form cycles pay no collector
    /// cost regardless of their allocation rate.
    ///
    /// Implementations that do not expose a configurable GC interval should
    /// return `None`, which tells the heap to use its built-in default
    /// scheduling threshold.
    fn gc_interval(&self) -> Option<usize>;

    /// Called when the VM enters its execution loop from a host boundary
    /// (`VM::run_external`), starting one execution window.
    ///
    /// Paired with [`on_execution_stop`](Self::on_execution_stop) and never
    /// nested — VM-internal re-entry (task switches, host-initiated function
    /// evaluation) uses the raw run loop, so its time falls inside the
    /// enclosing window. Trackers that measure execution time run their
    /// clock between the pair; the clock is *not* running while execution is
    /// suspended waiting on the host (external function calls) or between
    /// feeds. Default is a no-op.
    fn on_execution_start(&self) {}

    /// Called when the VM leaves its execution loop — on completion, error,
    /// or suspension at an external call. See [`on_execution_start`](Self::on_execution_start).
    fn on_execution_stop(&self) {}

    /// Lowers the active recursion-depth limit to `new_limit`.
    ///
    /// Exposed under the `test-hooks` feature so `sys.setrecursionlimit` can
    /// tighten the depth ceiling from inside fixture code. Implementations
    /// MUST refuse to *raise* the limit above whatever ceiling the host
    /// configured at construction time — that would let sandboxed code
    /// escape the host-imposed safety bound.
    ///
    /// Returns `Ok(())` when the requested limit was applied (including the
    /// no-op case `new_limit == current`). Returns `Err(current)` when the
    /// request would raise the limit, where `current` is the active limit
    /// (or `None` if the tracker has no settable limit at all). Callers
    /// surface this as a `ValueError` in the Python layer.
    ///
    /// The default implementation rejects all requests, so wrapper trackers
    /// that should expose this capability must explicitly delegate to their
    /// inner tracker.
    #[cfg(feature = "test-hooks")]
    fn lower_recursion_limit(&self, _new_limit: usize) -> Result<(), Option<usize>> {
        Err(None)
    }
}

/// A resource tracker that imposes no limits except default recursion limit.
///
/// Recursion limit is set to the cpython default of 1000.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoLimitTracker;

impl ResourceTracker for NoLimitTracker {
    #[inline]
    fn on_free(&self, _: impl FnOnce() -> usize) {}

    #[inline]
    fn check_time(&self) -> Result<(), ResourceError> {
        Ok(())
    }

    #[inline]
    fn on_grow(&self, _: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        Ok(())
    }

    /// Set the recursion limit to 1000.
    ///
    /// The high limit here may cause stack overflow errors in debug mode, but do not those errors should
    /// not occur with release builds.
    #[inline]
    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError> {
        const DEFAULT_RECURSION_LIMIT: usize = 1000;
        if current_depth >= DEFAULT_RECURSION_LIMIT {
            Err(ResourceError::Recursion {
                limit: DEFAULT_RECURSION_LIMIT,
                depth: current_depth + 1,
            })
        } else {
            Ok(())
        }
    }

    #[inline]
    fn check_large_result(&self, _estimated_bytes: usize) -> Result<(), ResourceError> {
        // No limit - always allow operations regardless of result size
        Ok(())
    }

    #[inline]
    fn gc_interval(&self) -> Option<usize> {
        None
    }
}

/// Configuration for resource limits.
///
/// All limits are optional - set to `None` to disable a specific limit.
/// Use `ResourceLimits::default()` for no limits, or build custom limits
/// with the builder pattern.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ResourceLimits {
    /// Maximum execution time.
    pub max_duration: Option<Duration>,
    /// Maximum heap memory in bytes (approximate).
    pub max_memory: Option<usize>,
    /// Run garbage collection every N GC-tracked allocations.
    pub gc_interval: Option<usize>,
    /// Maximum recursion depth (function call stack depth).
    pub max_recursion_depth: Option<usize>,
}

/// Recommended maximum recursion depth if not otherwise specified.
pub const DEFAULT_MAX_RECURSION_DEPTH: usize = 1000;

impl ResourceLimits {
    /// Creates a new ResourceLimits with all limits disabled, except max recursion which is set to 1000.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_recursion_depth: Some(1000),
            ..Default::default()
        }
    }

    /// Sets the maximum execution duration.
    #[must_use]
    pub fn max_duration(mut self, limit: Duration) -> Self {
        self.max_duration = Some(limit);
        self
    }

    /// Sets the maximum memory usage in bytes.
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
    pub fn max_recursion_depth(mut self, limit: Option<usize>) -> Self {
        self.max_recursion_depth = limit;
        self
    }
}

/// How often to actually check `Instant::elapsed()` in `check_time`.
///
/// Calling `Instant::elapsed()` on every `check_time` invocation adds measurable
/// overhead in tight loops (the VM calls `check_time` on every instruction).
/// By only checking every N calls, we reduce this overhead while still catching
/// timeouts promptly.
const TIME_CHECK_INTERVAL: u16 = 10;

/// A resource tracker that enforces configurable limits.
///
/// Tracks memory usage and execution time, returning errors when limits
/// are exceeded. Also schedules garbage collection at configurable
/// intervals.
///
/// Uses `Cell` for interior mutability to allow many methods which take
/// `&self` (enabling `&self` on critical methods such as `Heap::allocate`).
///
/// `max_duration` limits *cumulative execution time*: the clock runs only
/// while the VM is executing bytecode (between the outermost
/// `on_execution_start`/`on_execution_stop` pair) and is paused while
/// execution is suspended waiting on the host — external function calls,
/// OS callbacks — and between REPL feeds. The accumulated time is
/// serialized, so a deserialized session resumes its budget where it left
/// off rather than restarting from zero.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct LimitedTracker {
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
    /// Current approximate memory usage in bytes.
    current_memory: Cell<usize>,
    /// Counter for rate-limiting `Instant::elapsed()` calls in `check_time`.
    check_counter: Cell<u16>,
    /// Optional override applied on top of `limits.max_recursion_depth`.
    ///
    /// `None` (the default — also the value any pre-`test-hooks` snapshot
    /// deserializes to) means "no override, use the configured ceiling".
    /// `Some(N)` means "use `N` as the live recursion ceiling instead", and
    /// is only ever populated by
    /// [`lower_recursion_limit`](ResourceTracker::lower_recursion_limit)
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

impl LimitedTracker {
    /// Creates a new LimitedTracker with the given limits.
    ///
    /// The execution-time clock starts at zero and only runs while the VM
    /// executes, so the tracker can be created any amount of time before
    /// the first run without consuming the duration budget.
    #[must_use]
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            total_execution_time: Cell::new(Duration::ZERO),
            running_since: Cell::new(None),
            current_memory: Cell::new(0),
            check_counter: Cell::new(0),
            recursion_limit_override: Cell::new(None),
        }
    }

    /// Returns the live recursion ceiling: the override if one is in effect,
    /// otherwise the configured `max_recursion_depth`.
    fn active_recursion_limit(&self) -> Option<usize> {
        self.recursion_limit_override.get().or(self.limits.max_recursion_depth)
    }

    /// Returns the current approximate memory usage.
    ///
    /// Only meaningful when a `max_memory` limit is configured — without one
    /// the tracker skips memory accounting entirely and this stays 0.
    #[must_use]
    pub fn current_memory(&self) -> usize {
        self.current_memory.get()
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
}

impl ResourceTracker for LimitedTracker {
    fn on_free(&self, get_size: impl FnOnce() -> usize) {
        // Memory is only tracked when a limit is configured (`on_grow` skips
        // the size computation otherwise), so skip symmetrically here.
        if self.limits.max_memory.is_some() {
            let current = self.current_memory.get();
            self.current_memory.set(current.saturating_sub(get_size()));
        }
    }

    fn on_grow(&self, get_additional: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        if let Some(max) = self.limits.max_memory {
            // Saturating: a wrapping add on 32-bit targets must not slip past
            // the limit.
            let new_memory = self.current_memory.get().saturating_add(get_additional());
            if new_memory > max {
                return Err(ResourceError::Memory {
                    limit: max,
                    used: new_memory,
                });
            }
            self.current_memory.set(new_memory);
        }
        // No memory limit: skip the check AND the (possibly costly) size
        // computation — `get_additional` is never called.
        Ok(())
    }

    fn check_time(&self) -> Result<(), ResourceError> {
        if let Some(max) = self.limits.max_duration {
            self.check_counter.update(|c| c.wrapping_add(1));
            if self.check_counter.get().is_multiple_of(TIME_CHECK_INTERVAL) {
                // Only call Instant::elapsed() every TIME_CHECK_INTERVAL calls
                let elapsed = self.elapsed();
                if elapsed > max {
                    // Reset counter so the very next check_time call also triggers
                    // an elapsed check. This is important because some callers
                    // (e.g. repr_sequence_fmt) catch the error and return normally,
                    // and we need the VM loop's next check_time to re-detect timeout.
                    self.check_counter.set(TIME_CHECK_INTERVAL.wrapping_sub(1));
                    return Err(ResourceError::Time { limit: max, elapsed });
                }
            }
        }
        Ok(())
    }

    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError> {
        if let Some(max) = self.active_recursion_limit() {
            // current_depth is before push, so new depth would be current_depth + 1
            if current_depth >= max {
                return Err(ResourceError::Recursion {
                    limit: max,
                    depth: current_depth + 1,
                });
            }
        }
        Ok(())
    }

    fn check_large_result(&self, estimated_bytes: usize) -> Result<(), ResourceError> {
        if let Some(max) = self.limits.max_memory {
            let new_memory = self.current_memory.get().saturating_add(estimated_bytes);
            if new_memory > max {
                return Err(ResourceError::Memory {
                    limit: max,
                    used: new_memory,
                });
            }
        }
        Ok(())
    }

    fn gc_interval(&self) -> Option<usize> {
        self.limits.gc_interval
    }

    fn on_execution_start(&self) {
        debug_assert!(
            self.running_since.get().is_none(),
            "nested on_execution_start: VM-internal re-entry must use the raw run loop, not run_external"
        );
        self.running_since.set(Some(Instant::now()));
    }

    fn on_execution_stop(&self) {
        if let Some(started) = self.running_since.take() {
            self.total_execution_time
                .set(self.total_execution_time.get() + started.elapsed());
        }
    }

    /// Lowers the live recursion ceiling to `new_limit`, refusing to raise it.
    ///
    /// The constructed limit (`limits.max_recursion_depth`) acts as the hard
    /// upper bound — `sys.setrecursionlimit` may only tighten it, never relax
    /// it. Crossing from "no limit configured" to a concrete value counts as
    /// lowering (infinity → finite); going from `Some(N)` to `Some(K)` with
    /// `K > N` is rejected.
    #[cfg(feature = "test-hooks")]
    fn lower_recursion_limit(&self, new_limit: usize) -> Result<(), Option<usize>> {
        if let Some(current) = self.active_recursion_limit()
            && new_limit > current
        {
            return Err(Some(current));
        }
        self.recursion_limit_override.set(Some(new_limit));
        Ok(())
    }
}
