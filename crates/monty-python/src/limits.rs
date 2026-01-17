//! Python wrapper for Monty's `ResourceLimits`.
//!
//! Provides a TypedDict interface to configure resource limits for code execution,
//! including time limits, memory limits, and recursion depth.

use std::time::Duration;

use monty::{ResourceError, ResourceTracker};
use pyo3::{prelude::*, types::PyDict};

use crate::exceptions::exc_py_to_monty;

/// Default maximum recursion depth if not specified.
const DEFAULT_MAX_RECURSION_DEPTH: usize = 1000;

/// Creates the `ResourceLimits` TypedDict class.
///
/// This is called during module initialization to create and register the TypedDict.
pub fn create_resource_limits_class(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let locals = PyDict::new(py);
    py.run(
        c"
from typing import TypedDict

class ResourceLimits(TypedDict, total=False):
    \"\"\"
    Configuration for resource limits during code execution.

    All limits are optional. Omit a key to disable that limit.
    \"\"\"
    max_allocations: int
    max_duration_secs: float
    max_memory: int
    gc_interval: int
    max_recursion_depth: int
",
        None,
        Some(&locals),
    )?;

    Ok(locals.get_item("ResourceLimits")?.unwrap())
}

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
        limits = limits.max_duration(Duration::from_secs_f64(secs));
    }
    if let Some(max) = max_memory {
        limits = limits.max_memory(max);
    }
    if let Some(interval) = gc_interval {
        limits = limits.gc_interval(interval);
    }

    Ok(limits)
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
    check_counter: u16,
}

impl<T: ResourceTracker> PySignalTracker<T> {
    /// Creates a new signal-checking tracker wrapping the given tracker.
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            check_counter: 0,
        }
    }

    fn check_python_signals(&mut self) -> Result<(), ResourceError> {
        // Periodically check Python signals
        self.check_counter = self.check_counter.wrapping_add(1);

        if self.check_counter.is_multiple_of(SIGNAL_CHECK_INTERVAL) {
            Python::attach(|py| {
                py.check_signals()
                    .map_err(|e| ResourceError::Exception(exc_py_to_monty(py, &e)))
            })?;
        }
        Ok(())
    }
}

impl<T: ResourceTracker> ResourceTracker for PySignalTracker<T> {
    fn on_allocate(&mut self, get_size: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        self.inner.on_allocate(get_size)
    }

    fn on_free(&mut self, get_size: impl FnOnce() -> usize) {
        self.inner.on_free(get_size);
    }

    fn check_time(&mut self) -> Result<(), ResourceError> {
        // First check inner tracker's time limit
        self.inner.check_time()?;

        // then periodically check for Python signals
        self.check_python_signals()
    }

    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError> {
        self.inner.check_recursion_depth(current_depth)
    }
}
