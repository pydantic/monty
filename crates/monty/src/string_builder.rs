//! Resource-tracked builder for `String` values.
//!
//! `StringBuilder` is the canonical way to build a Python-visible string whose
//! final size is *not* already bounded by an already-tracked input. Operations
//! that grow a `String` in a loop — padding methods (`ljust`, `center`, …),
//! tab expansion, string repetition, container `repr()`, etc. — must use
//! `StringBuilder` rather than `String::with_capacity(...).push(...)`, because
//! the intermediate `String` lives on the Rust heap *outside* the
//! [`ResourceTracker`]. Without a builder, a malicious script can amplify a
//! small tracked input into a multi-gigabyte intermediate before the final
//! [`allocate_string`](crate::types::str::allocate_string) ever consults the
//! tracker — bypassing the configured memory limit and OOMing the host.
//!
//! # Active reservation, not preview
//!
//! Each growth actively *reserves* bytes with the tracker via
//! [`ResourceTracker::on_grow`]. This matters because Monty allows nested
//! operations: a [`str.join`](crate::types::str) over arbitrary objects can
//! invoke user-defined `__str__`/`__repr__` methods, which may themselves
//! build strings. A preview-only check (`check_estimated_size`) would let the
//! inner build pass against the *committed* memory state while ignoring the
//! outer builder's in-progress buffer — so the two could collectively exceed
//! the configured memory limit. By reserving instead, the outer builder's
//! bytes are visible to every nested operation, and the limit applies
//! cumulatively. Reservations are released on drop (cleanup on `?` /
//! early-return paths) or in [`finish`](StringBuilder::finish), which folds
//! the handoff to [`allocate_string`](crate::types::str::allocate_string)
//! into a single method so the final size is re-added via `on_grow`
//! without double-counting and without exposing the brief release window to
//! callers.
//!
//! # Growth policy
//!
//! Capacity doubles on each growth (matching `Vec`'s policy), so an `n`-byte
//! build incurs `O(log n)` tracker calls rather than `O(n)`. Use
//! [`with_capacity`](StringBuilder::with_capacity) when an upper bound is
//! known up front (e.g. padding to a width) — a single reservation covers
//! every subsequent push. Use [`new`](StringBuilder::new) when the size is
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

use std::{fmt, mem};

use monty_types::{ResourceError, ResourceTracker};

use crate::{exception_private::RunResult, heap::Heap, types::str::allocate_string, value::Value};

/// Resource-tracked builder for a `String`.
///
/// Holds an inner `String`, a tracker reference, and the byte count currently
/// reserved with the tracker. Growth calls [`ResourceTracker::on_grow`] to
/// reserve additional bytes (which fails fast if the memory limit would be
/// exceeded), and [`Drop`] / [`finish`](Self::finish) release the reservation
/// via [`ResourceTracker::on_free`].
///
/// Typical use:
///
/// ```ignore
/// let mut builder = StringBuilder::with_capacity(cap, vm.heap.tracker())?;
/// builder.push_str(prefix)?;
/// for _ in 0..pad { builder.push(fill)?; }
/// builder.finish(vm.heap)
/// ```
pub struct StringBuilder<'t, T: ResourceTracker> {
    inner: String,
    tracker: &'t T,
    /// Bytes currently reserved with `tracker` via `on_grow`. Always released
    /// before the builder ceases to exist — either in `finish` (so the
    /// follow-up `allocate_string` can re-add the final size without
    /// double-counting) or in `Drop` (for early-return paths).
    reserved: usize,
    /// Tracker error captured during a [`fmt::Write`] call. `fmt::Error` is
    /// payload-free, so we stash the real error here and surface it via
    /// [`finish`](Self::finish) or [`finish_raw`](Self::finish_raw). Direct
    /// callers of [`push`](Self::push) / [`push_str`](Self::push_str) never
    /// set this — they receive the [`ResourceError`] in the return value.
    pending_error: Option<ResourceError>,
}

impl<'t, T: ResourceTracker> StringBuilder<'t, T> {
    /// Creates an empty builder with no pre-approved capacity.
    ///
    /// Use when the final size is not bounded up front. The builder will
    /// request additional reservation from the tracker on each 2× growth.
    pub fn new(tracker: &'t T) -> Self {
        Self {
            inner: String::new(),
            tracker,
            reserved: 0,
            pending_error: None,
        }
    }

    /// Creates a builder with `capacity` bytes reserved up front.
    ///
    /// Use when the final size is known or bounded (e.g. padding to a given
    /// width). One up-front `on_grow` call covers every subsequent push that
    /// stays within `capacity`.
    pub fn with_capacity(capacity: usize, tracker: &'t T) -> Result<Self, ResourceError> {
        tracker.on_grow(|| capacity)?;
        Ok(Self {
            inner: String::with_capacity(capacity),
            tracker,
            reserved: capacity,
            pending_error: None,
        })
    }

    /// Appends a single character, reserving more capacity from the tracker if
    /// the resulting length would exceed the currently reserved bytes.
    pub fn push(&mut self, c: char) -> Result<(), ResourceError> {
        let needed = self.inner.len().saturating_add(c.len_utf8());
        self.ensure(needed)?;
        self.inner.push(c);
        Ok(())
    }

    /// Appends a string slice, reserving more capacity from the tracker if the
    /// resulting length would exceed the currently reserved bytes.
    pub fn push_str(&mut self, s: &str) -> Result<(), ResourceError> {
        let needed = self.inner.len().saturating_add(s.len());
        self.ensure(needed)?;
        self.inner.push_str(s);
        Ok(())
    }

    /// Consumes the builder and allocates the resulting string in `heap`.
    ///
    /// Releases the tracker reservation, then hands off to
    /// [`allocate_string`] which re-adds the final size via `on_grow`
    /// (or interns the result for empty / single-ASCII strings). If a prior
    /// [`fmt::Write`] call captured a tracker error, that error is returned
    /// here rather than the (now-stale) inner string.
    pub fn finish(mut self, heap: &Heap<T>) -> RunResult<Value> {
        if let Some(e) = self.pending_error.take() {
            // The reservation is released by Drop when `self` goes out of
            // scope at function return — no need to release here.
            return Err(e.into());
        }
        self.release();
        Ok(allocate_string(mem::take(&mut self.inner), heap)?)
    }

    /// Consumes the builder and returns the raw `String`, releasing the
    /// tracker reservation. Like [`finish`](Self::finish), a stashed
    /// [`fmt::Write`] tracker error is surfaced here.
    ///
    /// The result is **not** re-tracked — the caller must immediately hand it
    /// to a tracked allocation (e.g. `Heap::allocate` for a `bytes` object
    /// built via `String::into_bytes`) so the final size is counted exactly
    /// once, mirroring `finish`'s handoff to `allocate_string`.
    pub fn finish_raw(mut self) -> RunResult<String> {
        if let Some(e) = self.pending_error.take() {
            // The reservation is released by Drop, as in `finish`.
            return Err(e.into());
        }
        self.release();
        Ok(mem::take(&mut self.inner))
    }

    fn ensure(&mut self, needed: usize) -> Result<(), ResourceError> {
        if needed > self.reserved {
            // Double the reservation (saturating) but at least to `needed`.
            // Matches `Vec`'s growth policy so an `n`-byte build incurs
            // `O(log n)` tracker calls rather than `O(n)`.
            let new_reserved = self.reserved.saturating_mul(2).max(needed);
            let additional = new_reserved - self.reserved;
            self.tracker.on_grow(|| additional)?;
            self.reserved = new_reserved;
        }
        Ok(())
    }

    fn release(&mut self) {
        if self.reserved > 0 {
            let reserved = self.reserved;
            self.tracker.on_free(|| reserved);
            self.reserved = 0;
        }
    }
}

/// `fmt::Write` impl so `write!(builder, ...)` and `format_args!` work
/// against any tracker-protected builder. A tracker rejection is converted
/// into the payload-free [`fmt::Error`] and stashed in `pending_error`;
/// short-circuits subsequent writes so a partially-built string doesn't keep
/// accruing reservations after the limit has been hit.
impl<T: ResourceTracker> fmt::Write for StringBuilder<'_, T> {
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

impl<T: ResourceTracker> Drop for StringBuilder<'_, T> {
    fn drop(&mut self) {
        // Release any outstanding reservation if the builder is dropped without
        // finishing (e.g. an early return via `?` during a push, or a stashed
        // `pending_error` short-circuiting `finish`). `finish`'s success path
        // zeroes `reserved` before this runs, so it's a no-op there.
        self.release();
    }
}
