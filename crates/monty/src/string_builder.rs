//! Resource-tracked builder for `String` values.
//!
//! `StringBuilder` is the canonical way to build a Python-visible string whose
//! final size is not already bounded by an existing input. Operations that grow
//! a `String` in a loop — padding, tab expansion, repetition, container repr,
//! etc. — must use `StringBuilder` rather than unchecked `String` growth. One
//! large reservation could otherwise jump across both allocator limits before
//! execution reaches another checkpoint.
//!
//! # Allocator checks
//!
//! Each capacity increase is preflighted against allocator-backed usage. The
//! in-progress buffer is itself visible to the allocator, including during
//! nested string builds.
//!
//! # Growth policy
//!
//! Capacity doubles on each growth (matching `Vec`'s policy), so an `n`-byte
//! build incurs `O(log n)` tracker calls rather than `O(n)`. Use
//! [`with_capacity`](StringBuilder::with_capacity) when an upper bound is
//! known up front (e.g. padding to a width) — a single check covers every
//! subsequent push. Use [`new`](StringBuilder::new) when the size is
//! not bounded up front.
//!
//! # Two APIs: direct push and `fmt::Write`
//!
//! Callers that build strings imperatively use [`push`](StringBuilder::push)
//! and [`push_str`](StringBuilder::push_str), which return [`ResourceError`]
//! directly. Callers that need to plug into `fmt::Write`-based machinery
//! (`write!`, `format_args!`, the existing `py_repr_fmt` recursion) use the
//! builder's [`fmt::Write`] impl, which captures any [`ResourceError`] into
//! an internal slot since `fmt::Error` is payload-free. The stored error is
//! surfaced automatically by [`finish`](StringBuilder::finish), so the
//! tracker error reaches the caller even when the intermediate
//! `fmt::Error` is swallowed by a downstream formatter.

use std::{fmt, iter, mem};

use monty_types::{ResourceError, ResourceTracker};

use crate::{exception_private::RunResult, heap::Heap, types::str::allocate_string, value::Value};

/// Resource-tracked builder for a `String`.
///
/// Holds an inner `String`, its tracker, and the capacity approved so far.
/// Growth is preflighted against real allocator usage before reserving.
///
/// Typical use:
///
/// ```ignore
/// let mut builder = StringBuilder::with_capacity(cap, &vm.heap.tracker)?;
/// builder.push_str(prefix)?;
/// for _ in 0..pad { builder.push(fill)?; }
/// builder.finish(vm.heap)
/// ```
pub struct StringBuilder<'t> {
    inner: String,
    tracker: &'t ResourceTracker,
    /// Capacity already approved by the tracker.
    approved_capacity: usize,
    /// Tracker error captured during a [`fmt::Write`] call. `fmt::Error` is
    /// payload-free, so we stash the real error here and surface it via
    /// [`finish`](Self::finish) or [`finish_raw`](Self::finish_raw). Direct
    /// callers of [`push`](Self::push) / [`push_str`](Self::push_str) never
    /// set this — they receive the [`ResourceError`] in the return value.
    pending_error: Option<ResourceError>,
}

impl<'t> StringBuilder<'t> {
    /// Creates an empty builder with no pre-approved capacity.
    ///
    /// Use when the final size is not bounded up front. Each 2× growth is
    /// checked before the underlying string reserves more capacity.
    pub fn new(tracker: &'t ResourceTracker) -> Self {
        Self {
            inner: String::new(),
            tracker,
            approved_capacity: 0,
            pending_error: None,
        }
    }

    /// Existing capacity is already tracker-accounted; only later growth needs preflight.
    pub fn from_existing(inner: String, tracker: &'t ResourceTracker) -> Self {
        let approved_capacity = inner.capacity();
        Self {
            inner,
            tracker,
            approved_capacity,
            pending_error: None,
        }
    }

    /// Creates a builder with `capacity` bytes reserved up front.
    ///
    /// Use when the final size is known or bounded (e.g. padding to a given
    /// width). One up-front check covers pushes within `capacity`.
    pub fn with_capacity(capacity: usize, tracker: &'t ResourceTracker) -> Result<Self, ResourceError> {
        tracker.check_allocation(capacity)?;
        check_string_capacity(capacity)?;
        Ok(Self {
            inner: String::with_capacity(capacity),
            tracker,
            approved_capacity: capacity,
            pending_error: None,
        })
    }

    /// Appends a character after checking any required capacity increase.
    pub fn push(&mut self, c: char) -> Result<(), ResourceError> {
        let needed = self.inner.len().saturating_add(c.len_utf8());
        self.ensure(needed)?;
        self.inner.push(c);
        Ok(())
    }

    /// Appends a string slice after checking any required capacity increase.
    pub fn push_str(&mut self, s: &str) -> Result<(), ResourceError> {
        let needed = self.inner.len().saturating_add(s.len());
        self.ensure(needed)?;
        self.inner.push_str(s);
        Ok(())
    }

    /// Appends `count` copies of `c`, in chunks with the tracker's clock polled
    /// between them so a wide padding request stays interruptible.
    pub fn push_repeated(&mut self, c: char, count: usize) -> Result<(), ResourceError> {
        let unit: String = iter::repeat_n(c, count.min(FILL_CHUNK)).collect();
        let mut remaining = count;
        let mut chunk = 0;
        while remaining > 0 {
            self.tracker.check_time_every(chunk)?;
            chunk += 1;
            let take = remaining.min(FILL_CHUNK);
            self.push_str(&unit[..take * c.len_utf8()])?;
            remaining -= take;
        }
        Ok(())
    }

    /// Consumes the builder and allocates the resulting string in `heap`.
    ///
    /// If a prior [`fmt::Write`] call captured a tracker error, that error is
    /// returned instead of the partial string.
    pub fn finish(mut self, heap: &Heap) -> RunResult<Value> {
        if let Some(e) = self.pending_error.take() {
            return Err(e.into());
        }
        Ok(allocate_string(mem::take(&mut self.inner), heap))
    }

    /// Consumes the builder and returns the raw `String`.
    ///
    /// Like [`finish`](Self::finish), this surfaces a tracker error captured by
    /// `fmt::Write` instead of returning the partial string.
    pub fn finish_raw(mut self) -> RunResult<String> {
        if let Some(e) = self.pending_error.take() {
            return Err(e.into());
        }
        Ok(mem::take(&mut self.inner))
    }

    fn ensure(&mut self, needed: usize) -> Result<(), ResourceError> {
        approve_growth(&mut self.approved_capacity, needed, self.tracker)
    }
}

/// Resource-tracked builder for a `Vec<u8>`: the [`StringBuilder`]
/// counterpart for byte output, with the same preflighted 2× growth.
pub struct BytesBuilder<'t> {
    inner: Vec<u8>,
    tracker: &'t ResourceTracker,
    /// Capacity already approved by the tracker.
    approved_capacity: usize,
}

impl<'t> BytesBuilder<'t> {
    /// Existing capacity is already tracker-accounted; only later growth needs preflight.
    pub fn from_existing(inner: Vec<u8>, tracker: &'t ResourceTracker) -> Self {
        let approved_capacity = inner.capacity();
        Self {
            inner,
            tracker,
            approved_capacity,
        }
    }

    /// Creates a builder with `capacity` bytes reserved up front, after one tracker check.
    pub fn with_capacity(capacity: usize, tracker: &'t ResourceTracker) -> Result<Self, ResourceError> {
        tracker.check_allocation(capacity)?;
        check_string_capacity(capacity)?;
        Ok(Self {
            inner: Vec::with_capacity(capacity),
            tracker,
            approved_capacity: capacity,
        })
    }

    /// Appends a byte slice after checking any required capacity increase.
    pub fn push_slice(&mut self, bytes: &[u8]) -> Result<(), ResourceError> {
        let needed = self.inner.len().saturating_add(bytes.len());
        approve_growth(&mut self.approved_capacity, needed, self.tracker)?;
        self.inner.extend_from_slice(bytes);
        Ok(())
    }

    /// Appends `count` copies of `byte`, in chunks with the tracker's clock
    /// polled between them so a wide padding request stays interruptible.
    pub fn push_repeated(&mut self, byte: u8, count: usize) -> Result<(), ResourceError> {
        let unit = vec![byte; count.min(FILL_CHUNK)];
        let mut remaining = count;
        let mut chunk = 0;
        while remaining > 0 {
            self.tracker.check_time_every(chunk)?;
            chunk += 1;
            let take = remaining.min(FILL_CHUNK);
            self.push_slice(&unit[..take])?;
            remaining -= take;
        }
        Ok(())
    }

    /// Consumes the builder and returns the bytes.
    pub fn finish(self) -> Vec<u8> {
        self.inner
    }
}

/// Approves growth to `needed` bytes against the tracker, doubling the
/// approved capacity like `Vec` so an n-byte build incurs O(log n) checks.
fn approve_growth(approved: &mut usize, needed: usize, tracker: &ResourceTracker) -> Result<(), ResourceError> {
    if needed > *approved {
        let new_capacity = approved.saturating_mul(2).max(needed);
        tracker.check_allocation(new_capacity - *approved)?;
        check_string_capacity(new_capacity)?;
        *approved = new_capacity;
    }
    Ok(())
}

const MAX_STRING_CAPACITY: usize = isize::MAX as usize;

/// Fill bytes appended between tracker clock polls by `push_repeated`.
const FILL_CHUNK: usize = 4096;

/// A capacity above `isize::MAX` panics inside the reservation itself, before
/// the allocator can refuse it, so both reservation paths fence it here.
fn check_string_capacity(capacity: usize) -> Result<(), ResourceError> {
    if capacity > MAX_STRING_CAPACITY {
        Err(ResourceError::Memory {
            limit: MAX_STRING_CAPACITY,
            used: capacity,
        })
    } else {
        Ok(())
    }
}

/// `fmt::Write` impl so `write!(builder, ...)` and `format_args!` work
/// against any tracker-protected builder. A tracker rejection is converted
/// into the payload-free [`fmt::Error`] and stashed in `pending_error`;
/// short-circuits subsequent writes after the limit has been hit.
impl fmt::Write for StringBuilder<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.pending_error.is_some() {
            return Err(fmt::Error);
        }
        self.push_str(s).map_err(|e| {
            self.pending_error = Some(e);
            fmt::Error
        })
    }

    fn write_char(&mut self, c: char) -> fmt::Result {
        if self.pending_error.is_some() {
            return Err(fmt::Error);
        }
        self.push(c).map_err(|e| {
            self.pending_error = Some(e);
            fmt::Error
        })
    }
}
