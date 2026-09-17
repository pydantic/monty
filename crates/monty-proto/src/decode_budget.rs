//! Allocation accounting shared by generated and hand-written wire decoders.
//!
//! Charges are cumulative for a frame: replacing a buffer pays for the entire
//! new allocation, without refunding the old one. This bounds reallocation
//! peaks and discarded oneof payloads without tracking allocation lifetimes.

use std::cell::Cell;

use prost::DecodeError;

use crate::frame::DEFAULT_MAX_DECODE_BYTES;

thread_local! {
    /// Prost's fixed decode signatures require an ambient, thread-local budget.
    static REMAINING: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Scopes a synchronous frame decode, restoring any enclosing budget even on
/// unwind. Nested protobuf messages share the scope; only frame entry points
/// create one. Public only through the test-util feature for integration tests.
pub fn with_decode_budget<R>(bytes: usize, f: impl FnOnce() -> R) -> R {
    let _guard = RestoreBudget(REMAINING.replace(Some(bytes.min(DEFAULT_MAX_DECODE_BYTES))));
    f()
}

/// The active scope's remaining bytes, or `None` outside a frame decode.
#[cfg(feature = "test-util")]
#[must_use]
pub fn decode_budget_remaining() -> Option<usize> {
    REMAINING.get()
}

/// Restores an enclosing decode's budget on normal return or unwind.
struct RestoreBudget(Option<usize>);

impl Drop for RestoreBudget {
    fn drop(&mut self) {
        REMAINING.set(self.0);
    }
}

/// Charges an allocation before it happens; failed charges leave the budget intact.
pub(crate) fn charge(bytes: usize) -> Result<(), DecodeError> {
    REMAINING.with(|remaining| {
        let current = remaining
            .get()
            .ok_or_else(|| error("decode allocation outside a frame; use decode_frame or FrameReader"))?;
        let next = current.checked_sub(bytes).ok_or_else(exhausted)?;
        remaining.set(Some(next));
        Ok(())
    })
}

/// Allocates a boxed payload; its inline fields need no separate charge.
pub(crate) fn boxed<T>(value: T) -> Result<Box<T>, DecodeError> {
    charge(size_of::<T>())?;
    Ok(Box::new(value))
}

/// Prost 0.14 has no public replacement for its deprecated error constructor.
#[expect(deprecated)]
pub(crate) fn error(message: &'static str) -> DecodeError {
    DecodeError::new(message)
}

/// A stable error shared by arithmetic overflow, exhaustion and allocation failure.
pub(crate) fn exhausted() -> DecodeError {
    error("frame exceeds decode memory budget")
}
