use std::{
    fmt,
    time::{Duration, Instant},
};

use crate::{
    ExcType, MontyException,
    exception_private::{ExceptionRaise, RawStackFrame, RunError, SimpleException},
};

/// Error returned when a resource limit is exceeded during execution.
///
/// This allows the sandbox to enforce strict limits on allocation count,
/// execution time, and memory usage.
#[derive(Debug, Clone)]
pub enum ResourceError {
    /// Maximum number of allocations exceeded.
    Allocation { limit: usize, count: usize },
    /// Maximum execution time exceeded.
    Time { limit: Duration, elapsed: Duration },
    /// Maximum memory usage exceeded.
    Memory { limit: usize, used: usize },
    /// Maximum recursion depth exceeded.
    Recursion { limit: usize, depth: usize },
    /// Any other error, e.g. when propagating a python exception
    Exception(MontyException),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocation { limit, count } => {
                write!(f, "allocation limit exceeded: {count} > {limit}")
            }
            Self::Time { limit, elapsed } => {
                write!(f, "time limit exceeded: {elapsed:?} > {limit:?}")
            }
            Self::Memory { limit, used } => {
                write!(f, "memory limit exceeded: {used} bytes > {limit} bytes")
            }
            Self::Recursion { .. } => {
                write!(f, "maximum recursion depth exceeded")
            }
            Self::Exception(exc) => {
                write!(f, "{exc}")
            }
        }
    }
}

impl std::error::Error for ResourceError {}

impl ResourceError {
    /// Converts this resource error to a Python exception with optional stack frame.
    ///
    /// Maps resource error types to Python exception types:
    /// - `Allocation` → `MemoryError`
    /// - `Memory` → `MemoryError`
    /// - `Time` → `TimeoutError`
    /// - `Recursion` → `RecursionError`
    #[must_use]
    pub(crate) fn into_exception(self, frame: Option<RawStackFrame>) -> ExceptionRaise {
        let (exc_type, msg) = match self {
            Self::Allocation { limit, count } => (
                ExcType::MemoryError,
                Some(format!("allocation limit exceeded: {count} > {limit}")),
            ),
            Self::Memory { limit, used } => (
                ExcType::MemoryError,
                Some(format!("memory limit exceeded: {used} bytes > {limit} bytes")),
            ),
            Self::Time { limit, elapsed } => (
                ExcType::TimeoutError,
                Some(format!("time limit exceeded: {elapsed:?} > {limit:?}")),
            ),
            Self::Recursion { .. } => (
                ExcType::RecursionError,
                Some("maximum recursion depth exceeded".to_string()),
            ),
            Self::Exception(exc) => (exc.exc_type(), exc.into_message()),
        };
        let exc = SimpleException::new(exc_type, msg);
        match frame {
            Some(f) => exc.with_frame(f),
            None => exc.into(),
        }
    }
}

impl From<ResourceError> for RunError {
    fn from(err: ResourceError) -> Self {
        Self::UncatchableExc(err.into_exception(None))
    }
}

/// Trait for tracking resource usage and scheduling garbage collection.
///
/// Implementations can enforce limits on allocations, time, and memory,
/// as well as schedule periodic garbage collection.
///
/// All implementations should eventually trigger garbage collection to handle
/// reference cycles. The `should_gc` method controls *frequency*, not whether
/// GC runs at all.
pub trait ResourceTracker: fmt::Debug {
    /// Called before each heap allocation.
    ///
    /// Returns `Ok(())` if the allocation should proceed, or `Err(ResourceError)`
    /// if a limit would be exceeded.
    ///
    /// # Arguments
    /// * `size` - Approximate size in bytes of the allocation
    fn on_allocate(&mut self, get_size: impl FnOnce() -> usize) -> Result<(), ResourceError>;

    /// Called when memory is freed (during dec_ref or garbage collection).
    ///
    /// # Arguments
    /// * `size` - Size in bytes of the freed allocation
    fn on_free(&mut self, get_size: impl FnOnce() -> usize);

    /// Called periodically (at statement boundaries) to check time limits.
    ///
    /// Returns `Ok(())` if within time limit, or `Err(ResourceError::Time)`
    /// if the limit is exceeded.
    fn check_time(&mut self) -> Result<(), ResourceError>;

    /// Called before pushing a new call frame to check recursion depth.
    ///
    /// Returns `Ok(())` if within recursion limit, or `Err(ResourceError::Recursion)`
    /// if the limit would be exceeded.
    ///
    /// # Arguments
    /// * `current_depth` - Current call stack depth (before the new frame is pushed)
    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError>;
}

/// A resource tracker that imposes no limits except default recursion limit.
///
/// Recursion limit is set to the cpython default of 1000.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoLimitTracker;

impl ResourceTracker for NoLimitTracker {
    #[inline]
    fn on_allocate(&mut self, _: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        Ok(())
    }

    #[inline]
    fn on_free(&mut self, _: impl FnOnce() -> usize) {}

    #[inline]
    fn check_time(&mut self) -> Result<(), ResourceError> {
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
}

/// Configuration for resource limits.
///
/// All limits are optional - set to `None` to disable a specific limit.
/// Use `ResourceLimits::default()` for no limits, or build custom limits
/// with the builder pattern.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ResourceLimits {
    /// Maximum number of heap allocations allowed.
    pub max_allocations: Option<usize>,
    /// Maximum execution time.
    pub max_duration: Option<Duration>,
    /// Maximum heap memory in bytes (approximate).
    pub max_memory: Option<usize>,
    /// Run garbage collection every N allocations.
    pub gc_interval: Option<usize>,
    /// Maximum recursion depth (function call stack depth).
    pub max_recursion_depth: Option<usize>,
}

impl ResourceLimits {
    /// Creates a new ResourceLimits with all limits disabled, except max recursion which is set to 1000.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_recursion_depth: Some(1000),
            ..Default::default()
        }
    }

    /// Sets the maximum number of allocations.
    #[must_use]
    pub fn max_allocations(mut self, limit: usize) -> Self {
        self.max_allocations = Some(limit);
        self
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

    /// Sets the garbage collection interval (run GC every N allocations).
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

/// A resource tracker that enforces configurable limits.
///
/// Tracks allocation count, memory usage, and execution time, returning
/// errors when limits are exceeded. Also schedules garbage collection
/// at configurable intervals.
///
/// When serialized/deserialized, the `start_time` is reset to `Instant::now()`.
/// This means time limits restart from zero after deserialization.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct LimitedTracker {
    limits: ResourceLimits,
    /// When execution started (for time limit checking).
    /// Reset to `Instant::now()` on deserialization.
    #[serde(skip, default = "Instant::now")]
    start_time: Instant,
    /// Total number of allocations made.
    allocation_count: usize,
    /// Current approximate memory usage in bytes.
    current_memory: usize,
}

impl LimitedTracker {
    /// Creates a new LimitedTracker with the given limits.
    ///
    /// The start time is recorded when the tracker is created, so create
    /// it immediately before starting execution.
    #[must_use]
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            start_time: Instant::now(),
            allocation_count: 0,
            current_memory: 0,
        }
    }

    /// Returns the current allocation count.
    #[must_use]
    pub fn allocation_count(&self) -> usize {
        self.allocation_count
    }

    /// Returns the current approximate memory usage.
    #[must_use]
    pub fn current_memory(&self) -> usize {
        self.current_memory
    }

    /// Returns the elapsed time since tracker creation.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }
}

impl ResourceTracker for LimitedTracker {
    fn on_allocate(&mut self, get_size: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        // Check allocation count limit
        if let Some(max) = self.limits.max_allocations
            && self.allocation_count >= max
        {
            return Err(ResourceError::Allocation {
                limit: max,
                count: self.allocation_count + 1,
            });
        }

        let size = get_size();
        // Check memory limit
        if let Some(max) = self.limits.max_memory {
            let new_memory = self.current_memory + size;
            if new_memory > max {
                return Err(ResourceError::Memory {
                    limit: max,
                    used: new_memory,
                });
            }
        }

        // Update tracking state
        self.allocation_count += 1;
        self.current_memory += size;

        Ok(())
    }

    fn on_free(&mut self, get_size: impl FnOnce() -> usize) {
        self.current_memory = self.current_memory.saturating_sub(get_size());
    }

    fn check_time(&mut self) -> Result<(), ResourceError> {
        if let Some(max) = self.limits.max_duration {
            let elapsed = self.start_time.elapsed();
            if elapsed > max {
                return Err(ResourceError::Time { limit: max, elapsed });
            }
        }
        Ok(())
    }

    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError> {
        if let Some(max) = self.limits.max_recursion_depth {
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
}
