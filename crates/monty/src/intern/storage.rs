//! Stable, append-only storage for committed compiler tables.

use std::ops::Index;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::heap::{HeapId, StableHeap};

/// Keeps committed entries borrowed across subsequent compilation and insertion.
/// Removal is deliberately unavailable: rejected compilations own separate vectors.
#[derive(Debug)]
pub(super) struct Entries<T>(StableHeap<T>);

impl<T> Entries<T> {
    /// Reserves slots without constructing entries.
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self(StableHeap::with_capacity(capacity))
    }

    /// Returns the next entry's dense index.
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    /// Appends an entry without invalidating references to earlier entries.
    pub(super) fn push(&self, entry: T) {
        self.0.allocate(entry);
    }

    /// Looks up an assigned index.
    pub(super) fn get(&self, index: usize) -> Option<&T> {
        (index < self.len()).then(|| self.0.get(HeapId::from_index(index)))
    }

    /// Visits committed entries in ID order.
    pub(super) fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.len()).map(|index| &self[index])
    }
}

impl<T> Default for Entries<T> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl<T> Index<usize> for Entries<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("invalid intern slot")
    }
}

impl<T: Clone> Clone for Entries<T> {
    fn clone(&self) -> Self {
        let entries = Self::with_capacity(self.len());
        for entry in self.iter() {
            entries.push(entry.clone());
        }
        entries
    }
}

impl<T: Serialize> Serialize for Entries<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Entries<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = Vec::<T>::deserialize(deserializer)?;
        let entries = Self::with_capacity(values.len());
        for value in values {
            entries.push(value);
        }
        Ok(entries)
    }
}
