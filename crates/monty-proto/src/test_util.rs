//! Internal hooks for exercising allocation boundaries from integration tests.

use prost::DecodeError;

use crate::BudgetVec;

/// Exercises non-growing insertion, including the otherwise unreachable full-buffer case.
pub fn push_reserved<T>(vec: &mut BudgetVec<T>, value: T) -> Result<(), DecodeError> {
    vec.try_push_reserved(value)
}

/// Exercises combined storage and host-reference charges without constructing a large frame.
pub fn reserve_with_overhead<T>(vec: &mut BudgetVec<T>, capacity: usize, overhead: usize) -> Result<(), DecodeError> {
    vec.try_reserve_capacity_with_overhead(capacity, overhead)
}
