//! BigInt utilities for arbitrary precision integer support.
//!
//! This module provides helper functions for working with BigInts, including
//! conversion between `Value::Int(i64)` and `HeapData::BigInt`. Python has one
//! `int` type, and BigInt is an implementation detail - we use i64 for performance
//! when values fit, and promote to BigInt on overflow.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

use crate::{
    heap::{Heap, HeapData},
    resource::{ResourceError, ResourceTracker},
    value::Value,
};

/// Converts a BigInt to a Value, demoting to i64 if it fits.
///
/// For performance, we want to keep values as `Value::Int(i64)` whenever possible.
/// This function checks if the BigInt fits in an i64 and returns `Value::Int` if so,
/// otherwise allocates a `HeapData::BigInt` on the heap.
pub fn bigint_to_value(bi: BigInt, heap: &mut Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    // Try to demote back to i64 for performance
    if let Some(i) = bi.to_i64() {
        Ok(Value::Int(i))
    } else {
        let heap_id = heap.allocate(HeapData::BigInt(bi))?;
        Ok(Value::Ref(heap_id))
    }
}

/// Checks if a BigInt is zero.
pub fn is_zero(bi: &BigInt) -> bool {
    bi.is_zero()
}

/// Computes a hash for a BigInt that is consistent with i64 hashing.
///
/// Critical: For values that fit in i64, this must return the same hash as
/// hashing the i64 directly. This ensures dict key consistency - e.g.,
/// `hash(5)` must equal `hash(BigInt(5))`.
pub fn hash_bigint(bi: &BigInt) -> u64 {
    // If the BigInt fits in i64, hash as i64 for consistency
    if let Some(i) = bi.to_i64() {
        let mut hasher = DefaultHasher::new();
        // Hash the i64 discriminant and value to match Value::Int hashing
        std::mem::discriminant(&Value::Int(0)).hash(&mut hasher);
        i.hash(&mut hasher);
        hasher.finish()
    } else {
        // For BigInts outside i64 range, use byte representation
        let mut hasher = DefaultHasher::new();
        // Use a unique discriminant for BigInt (we use the BigInt's sign and bytes)
        let (sign, bytes) = bi.to_bytes_le();
        sign.hash(&mut hasher);
        bytes.hash(&mut hasher);
        hasher.finish()
    }
}

/// Estimates the memory size of a BigInt in bytes.
///
/// Used for resource tracking. The actual size includes the Vec overhead
/// plus the digit storage.
pub fn estimate_size(bi: &BigInt) -> usize {
    // Each BigInt digit is typically a u32 or u64
    // We estimate based on the number of significant bits
    let bits = bi.bits();
    // Convert bits to bytes, add overhead for Vec and sign
    // On 32-bit platforms, truncate to usize::MAX if bits is too large
    let bit_bytes = usize::try_from(bits).unwrap_or(usize::MAX) / 8;
    bit_bytes + std::mem::size_of::<BigInt>()
}
