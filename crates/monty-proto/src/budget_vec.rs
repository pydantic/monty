//! Owned protocol buffers with fallible, budgeted growth during decoding.
//!
//! Host-side construction (`From`, `FromIterator`, `Clone`) is unbudgeted, like
//! constructing a protocol message itself. Decoders use the `try_*` methods;
//! no infallible growth or mutable access to the backing Vec is exposed.

use std::{
    ops::{Deref, DerefMut},
    slice, vec,
};

use prost::{DecodeError, bytes::Buf};

use crate::decode_budget::{charge, error, exhausted};

/// A protocol vector whose decoding growth is charged before allocation.
/// Conversion to and from a standard vector transfers ownership without copying.
/// Host construction and cloning do not require an active decode budget.
/// In-place growth must propagate a decode error:
/// ```compile_fail
/// let mut values = monty_proto::BudgetVec::<u8>::new();
/// values.push(1);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BudgetVec<T>(Vec<T>);

impl<T> BudgetVec<T> {
    /// Creates an empty buffer without allocating or requiring a decode scope.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Transfers decoded storage to a domain value without reallocating.
    #[must_use]
    pub fn into_inner(self) -> Vec<T> {
        self.0
    }

    /// Borrows the initialized elements without exposing vector growth.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    /// Returns the backing allocation's element capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Appends a decoded element, charging any required growth first.
    pub fn try_push(&mut self, value: T) -> Result<(), DecodeError> {
        self.try_reserve_slot()?;
        self.0.push(value);
        Ok(())
    }

    /// Appends within already-reserved capacity, rejecting insertion rather than growing.
    pub(crate) fn try_push_reserved(&mut self, value: T) -> Result<(), DecodeError> {
        if self.0.len() < self.0.capacity() {
            self.0.push(value);
            Ok(())
        } else {
            Err(error("decode buffer has no reserved capacity"))
        }
    }

    /// Reserves the next slot before decoding a potentially expensive payload.
    pub(crate) fn try_reserve_slot(&mut self) -> Result<(), DecodeError> {
        if self.0.len() == self.0.capacity() {
            let capacity = self.0.capacity().checked_mul(2).ok_or_else(exhausted)?.max(4);
            self.try_reserve_capacity(capacity)?;
        }
        Ok(())
    }

    /// Removes elements while retaining their already-paid backing allocation.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Discards trailing elements without changing the allocation charge.
    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    /// Charges the full replacement allocation to cover reallocation overlap.
    pub(crate) fn try_reserve_capacity(&mut self, capacity: usize) -> Result<(), DecodeError> {
        self.try_reserve_capacity_with_overhead(capacity, 0)
    }

    /// Reserves storage with an additional per-slot allowance, charging both in one preflight.
    /// The allowance applies to the full replacement capacity and cannot reduce the storage charge.
    pub(crate) fn try_reserve_capacity_with_overhead(
        &mut self,
        capacity: usize,
        overhead: usize,
    ) -> Result<(), DecodeError> {
        if capacity > self.0.capacity() {
            let cost = size_of::<T>().checked_add(overhead).ok_or_else(exhausted)?;
            charge(capacity.checked_mul(cost).ok_or_else(exhausted)?)?;
            self.0
                .try_reserve_exact(capacity - self.0.len())
                .map_err(|_| exhausted())?;
        }
        Ok(())
    }
}

impl BudgetVec<u8> {
    /// Replaces bytes from any `Buf` with one preflight and no temporary copy.
    pub(crate) fn try_replace(&mut self, mut source: impl Buf) -> Result<(), DecodeError> {
        self.try_reserve_capacity(source.remaining())?;
        self.0.clear();
        while source.has_remaining() {
            let chunk = source.chunk();
            self.0.extend_from_slice(chunk);
            let len = chunk.len();
            source.advance(len);
        }
        Ok(())
    }
}

impl<T> Default for BudgetVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<Vec<T>> for BudgetVec<T> {
    fn from(value: Vec<T>) -> Self {
        Self(value)
    }
}

impl<T> From<BudgetVec<T>> for Vec<T> {
    fn from(value: BudgetVec<T>) -> Self {
        value.into_inner()
    }
}

impl<T> FromIterator<T> for BudgetVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<T: PartialEq> PartialEq<&[T]> for BudgetVec<T> {
    fn eq(&self, other: &&[T]) -> bool {
        self.0 == *other
    }
}

impl<T: PartialEq> PartialEq<Vec<T>> for BudgetVec<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.0 == *other
    }
}

impl<T> Deref for BudgetVec<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> DerefMut for BudgetVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<T> AsRef<[T]> for BudgetVec<T> {
    fn as_ref(&self) -> &[T] {
        &self.0
    }
}

impl<T> IntoIterator for BudgetVec<T> {
    type Item = T;
    type IntoIter = vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a BudgetVec<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut BudgetVec<T> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}
