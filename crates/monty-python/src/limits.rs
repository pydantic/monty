//! Python wrapper for Monty's `ResourceLimits`.
//!
//! Provides a TypedDict interface to configure resource limits for code execution,
//! including time limits, memory limits, and recursion depth.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
    time::Duration,
};

use monty::{DEFAULT_MAX_RECURSION_DEPTH, ExcType, MontyException, ResourceError, ResourceTracker};
use pyo3::{exceptions::PyValueError, prelude::*, types::PyDict};

use crate::exceptions::exc_py_to_monty;

/// Shared flag used to interrupt async Monty execution after Python task cancellation.
pub(crate) type CancellationFlag = Arc<AtomicBool>;

/// Extracts resource limits from a Python dict.
///
/// The dict should have the following optional keys:
/// - `max_allocations`: Maximum number of heap allocations allowed (int)
/// - `max_duration_secs`: Maximum execution time in seconds (float)
/// - `max_memory`: Maximum heap memory in bytes (int)
/// - `gc_interval`: Run garbage collection every N allocations (int)
/// - `max_recursion_depth`: Maximum function call stack depth (int, default: 1000)
///
/// If a key is missing or set to `None`, that limit is not applied
/// (except `max_recursion_depth` which defaults to 1000).
///
/// Raises `TypeError` if a value is present but has the wrong type.
/// Raises `ValueError` if `max_duration_secs` is not a valid duration value.
pub fn extract_limits(dict: &Bound<'_, PyDict>) -> PyResult<monty::ResourceLimits> {
    let max_allocations = extract_optional_usize(dict, "max_allocations")?;
    let max_duration_secs = extract_optional_f64(dict, "max_duration_secs")?;
    let max_memory = extract_optional_usize(dict, "max_memory")?;
    let gc_interval = extract_optional_usize(dict, "gc_interval")?;
    let max_recursion_depth =
        extract_optional_usize(dict, "max_recursion_depth")?.or(Some(DEFAULT_MAX_RECURSION_DEPTH));

    let mut limits = monty::ResourceLimits::new().max_recursion_depth(max_recursion_depth);

    if let Some(max) = max_allocations {
        limits = limits.max_allocations(max);
    }
    if let Some(secs) = max_duration_secs {
        limits = limits
            .max_duration(Duration::try_from_secs_f64(secs).map_err(|err| PyValueError::new_err(err.to_string()))?);
    }
    if let Some(max) = max_memory {
        limits = limits.max_memory(max);
    }
    if let Some(interval) = gc_interval {
        limits = limits.gc_interval(interval);
    }

    Ok(limits)
}

/// Arms a cancellation flag for the lifetime of an async Rust future.
///
/// The guard is dropped when the Rust future created by `future_into_py()` is
/// dropped due to Python task cancellation. That drop flips the shared flag so
/// any in-flight blocking Monty execution notices cancellation at its next
/// periodic tracker check.
#[derive(Debug)]
pub(crate) struct FutureCancellationGuard {
    flag: CancellationFlag,
    armed: bool,
}

impl FutureCancellationGuard {
    /// Creates a new armed cancellation guard for the given flag.
    pub fn new(flag: CancellationFlag) -> Self {
        Self { flag, armed: true }
    }

    /// Disables cancellation signalling after normal completion.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for FutureCancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Extracts an optional usize from a dict, raising `TypeError` if the value has the wrong type.
fn extract_optional_usize(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<usize>> {
    match dict.get_item(key)? {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => Ok(Some(value.extract()?)),
    }
}

/// Extracts an optional f64 from a dict, raising `TypeError` if the value has the wrong type.
fn extract_optional_f64(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<f64>> {
    match dict.get_item(key)? {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => Ok(Some(value.extract()?)),
    }
}

/// How often to check Python signals (every N calls to `check_time`).
///
/// This balances responsiveness to Ctrl+C against performance overhead.
/// With ~1000 checks, signal handling adds negligible overhead while still
/// responding to interrupts within a reasonable timeframe.
const SIGNAL_CHECK_INTERVAL: u16 = 1000;

/// A resource tracker that wraps another ResourceTracker and periodically checks Python signals.
///
/// This allows Ctrl+C and other Python signals to interrupt long-running code
/// executed through the monty interpreter. Signals are checked every
/// `SIGNAL_CHECK_INTERVAL` calls to `check_time` (at statement boundaries).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PySignalTracker<T: ResourceTracker> {
    inner: T,
    /// Counter for check_time calls, used to rate-limit signal checks.
    ///
    /// Uses `AtomicU16` for interior mutability so `check_time` can take `&self`
    /// (required by the `ResourceTracker` trait) while remaining `Sync` for PyO3.
    check_counter: AtomicU16,
    /// Async cancellation flag shared with the Python awaitable, if any.
    ///
    /// This is skipped during serialization because cancellation state is tied
    /// to the current host future, not to persisted Monty snapshots.
    #[serde(skip, default)]
    cancel_flag: Option<CancellationFlag>,
}

impl<T: ResourceTracker> PySignalTracker<T> {
    /// Creates a new signal-checking tracker wrapping the given tracker.
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            check_counter: AtomicU16::new(0),
            cancel_flag: None,
        }
    }

    /// Creates a new tracker that also watches a host-provided cancellation flag.
    pub fn new_with_cancellation(inner: T, cancel_flag: CancellationFlag) -> Self {
        Self {
            inner,
            check_counter: AtomicU16::new(0),
            cancel_flag: Some(cancel_flag),
        }
    }

    /// Replaces the cancellation flag used for async host-driven interruption.
    ///
    /// REPL sessions reuse the same tracker across snippets, so the host installs
    /// a fresh flag for each async execution and clears it again when the snippet
    /// completes or is restored after cancellation.
    pub fn set_cancellation_flag(&mut self, cancel_flag: Option<CancellationFlag>) {
        self.cancel_flag = cancel_flag;
    }

    /// Checks whether the host cancelled the owning async awaitable.
    ///
    /// This uses `KeyboardInterrupt` as the internal stop signal because it is a
    /// `BaseException`-style interruption that should never be swallowed by untrusted
    /// Monty code. In normal cancellation flow the Python task still surfaces
    /// `CancelledError`; this exception is primarily for unwinding the VM safely.
    fn check_cancellation(&self) -> Result<(), ResourceError> {
        if self
            .cancel_flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(ResourceError::Exception(MontyException::new(
                ExcType::KeyboardInterrupt,
                Some("Monty execution cancelled".to_string()),
            )));
        }
        Ok(())
    }

    fn check_python_signals(&self) -> Result<(), ResourceError> {
        // Periodically check Python signals
        let count = self.check_counter.fetch_add(1, Ordering::Relaxed).wrapping_add(1);

        if count.is_multiple_of(SIGNAL_CHECK_INTERVAL) {
            self.check_cancellation()?;
            Python::attach(|py| {
                py.check_signals()
                    .map_err(|e| ResourceError::Exception(exc_py_to_monty(py, &e)))
            })?;
        }
        Ok(())
    }
}

impl<T: ResourceTracker> ResourceTracker for PySignalTracker<T> {
    fn on_allocate(&self, get_size: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        self.inner.on_allocate(get_size)
    }

    fn on_free(&self, get_size: impl FnOnce() -> usize) {
        self.inner.on_free(get_size);
    }

    fn check_time(&self) -> Result<(), ResourceError> {
        // First check inner tracker's time limit
        self.inner.check_time()?;

        // then periodically check for Python signals
        self.check_python_signals()
    }

    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError> {
        self.inner.check_recursion_depth(current_depth)
    }

    fn check_large_result(&self, estimated_bytes: usize) -> Result<(), ResourceError> {
        self.inner.check_large_result(estimated_bytes)
    }

    fn on_grow(&self, additional_bytes: usize) -> Result<(), ResourceError> {
        self.inner.on_grow(additional_bytes)
    }

    fn gc_interval(&self) -> Option<usize> {
        self.inner.gc_interval()
    }
}
