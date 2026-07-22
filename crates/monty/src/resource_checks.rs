use monty_types::{ExcType, LARGE_RESULT_THRESHOLD, ResourceError, ResourceTracker};

use crate::exception_private::{RunError, SimpleException};

/// Pre-checks that an operation producing `item_len * count` bytes won't exceed resource limits.
///
/// Used for sequence repeats (`'x' * 999_999_999`), padding operations
/// (`str.ljust`, `str.center`, `str.zfill`, etc.), and any other operation
/// where the result size is a simple product of two known values.
pub fn check_repeat_size(item_len: usize, count: usize, tracker: &impl ResourceTracker) -> Result<(), ResourceError> {
    check_estimated_size(item_len.saturating_mul(count), tracker)
}

/// Pre-checks that `base ** exponent` won't exceed resource limits before computing.
///
/// The result of `base ** exp` has approximately `base_bits * exp` bits.
/// For bases with 0 or 1 significant bits (0, 1, -1), the result is always
/// small regardless of exponent, so the check is skipped.
///
/// The estimate includes a 4× safety multiplier because `BigInt::pow` uses repeated squaring,
/// which allocates intermediate values on the Rust heap (not tracked by the resource tracker).
/// At peak, old/new base and old/new accumulator coexist simultaneously during each
/// multiplication step, requiring roughly 4× the final result size in memory.
pub fn check_pow_size(base_bits: u64, exponent: u64, tracker: &impl ResourceTracker) -> Result<(), ResourceError> {
    // 0**n = 0, 1**n = 1, (-1)**n = ±1 — always small
    if base_bits <= 1 {
        return Ok(());
    }
    let result_bytes = estimate_bits_to_bytes(base_bits.saturating_mul(exponent));
    // Repeated squaring needs ~4× result size in peak memory (old/new base + old/new accumulator
    // coexist during each multiplication step), and these are Rust heap allocations not tracked
    // by the resource tracker.
    check_estimated_size(result_bytes.saturating_mul(4), tracker)
}

/// Pre-checks that an integer multiplication won't exceed resource limits.
///
/// The result of multiplying two numbers has at most `a_bits + b_bits` bits.
pub fn check_mult_size(a_bits: u64, b_bits: u64, tracker: &impl ResourceTracker) -> Result<(), ResourceError> {
    check_estimated_size(estimate_bits_to_bytes(a_bits.saturating_add(b_bits)), tracker)
}

/// Pre-checks that a left shift won't exceed resource limits.
///
/// The result of `value << shift` has approximately `value_bits + shift` bits.
/// For zero values the result is always zero, so the check is skipped.
pub fn check_lshift_size(
    value_bits: u64,
    shift_amount: u64,
    tracker: &impl ResourceTracker,
) -> Result<(), ResourceError> {
    if value_bits == 0 {
        return Ok(());
    }
    check_estimated_size(estimate_bits_to_bytes(value_bits.saturating_add(shift_amount)), tracker)
}

/// Pre-checks that an integer division overflow promotion won't exceed resource limits.
///
/// Division results are bounded by the dividend size, but we still check for consistency
/// with other BigInt promotion paths.
pub fn check_div_size(dividend_bits: u64, tracker: &impl ResourceTracker) -> Result<(), ResourceError> {
    check_estimated_size(estimate_bits_to_bytes(dividend_bits), tracker)
}

/// Pre-checks that a string/bytes replace won't exceed resource limits before allocating.
///
/// Expanding replacements use the maximum possible match count. Shrinking
/// replacements use `input_len`, since zero matches copies the full input into
/// an otherwise untracked Rust `String` or `Vec`.
pub fn check_replace_size(
    input_len: usize,
    old_len: usize,
    new_len: usize,
    count: i64,
    tracker: &impl ResourceTracker,
) -> Result<(), ResourceError> {
    let estimated = if new_len < old_len {
        input_len
    } else {
        // Empty pattern (old_len == 0): inserts before each element + after the last = input_len + 1
        let max_replacements = input_len
            .checked_div(old_len)
            .unwrap_or_else(|| input_len.saturating_add(1));

        let replacements = if count < 0 {
            max_replacements
        } else {
            max_replacements.min(usize::try_from(count).unwrap_or(usize::MAX))
        };

        // Result = input_len - (replacements * old_len) + (replacements * new_len)
        let removed = replacements.saturating_mul(old_len);
        let added = replacements.saturating_mul(new_len);
        input_len.saturating_sub(removed).saturating_add(added)
    };

    check_estimated_size(estimated, tracker)
}

/// Checks an estimated result size against the resource tracker.
///
/// Only calls the tracker when the estimate exceeds `LARGE_RESULT_THRESHOLD`
/// to avoid overhead on small operations.
pub(crate) fn check_estimated_size(
    estimated_bytes: usize,
    tracker: &impl ResourceTracker,
) -> Result<(), ResourceError> {
    if estimated_bytes > LARGE_RESULT_THRESHOLD {
        tracker.check_large_result(estimated_bytes)?;
    }
    Ok(())
}

/// Converts an estimated bit count to bytes, saturating to `usize::MAX` on overflow.
///
/// Overflow means the result is astronomically large, so saturating ensures
/// the resource limit check always triggers rather than being silently skipped.
fn estimate_bits_to_bytes(bits: u64) -> usize {
    usize::try_from(bits.saturating_add(7) / 8).unwrap_or(usize::MAX)
}

/// Converts a resource error to its host-visible Python exception.
///
/// Recursion errors remain catchable for CPython compatibility; terminal
/// memory and time errors cannot be suppressed by sandboxed code.
impl From<ResourceError> for RunError {
    fn from(err: ResourceError) -> Self {
        let (exc_type, catchable) = match &err {
            ResourceError::Memory { .. } => (ExcType::MemoryError, false),
            ResourceError::Time { .. } => (ExcType::TimeoutError, false),
            ResourceError::Recursion { .. } => (ExcType::RecursionError, true),
        };
        let exc = SimpleException::new_msg(exc_type, err).into();
        if catchable {
            Self::Exc(exc)
        } else {
            Self::UncatchableExc(exc)
        }
    }
}
