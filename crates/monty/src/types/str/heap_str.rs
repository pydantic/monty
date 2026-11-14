//! Heap storage for Python strings.

use std::{cell::Cell, ops};

use crate::{
    hash::{HashValue, hash_python_str},
    heap::{HeapId, HeapItem},
};

/// Heap-backed Python string with lazy hash and ASCII caches.
/// The text is immutable; caches are omitted from snapshots and recomputed on use.
/// ASCII text has one byte per character, so length and indexing take O(1) time.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct HeapStr {
    text: Box<str>,
    #[serde(skip)]
    pub(super) cached_hash: Cell<Option<HashValue>>,
    #[serde(skip)]
    cached_ascii: Cell<Option<bool>>,
}

impl PartialEq for HeapStr {
    /// Compares only the string content — the cached fields are pure
    /// optimisations and must not affect equality.
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl HeapStr {
    /// Creates heap storage; use [`super::allocate_string`] or [`super::allocate_string_no_interning`] for values.
    #[must_use]
    pub(super) fn new(s: impl Into<Box<str>>) -> Self {
        Self {
            text: s.into(),
            cached_hash: Cell::new(None),
            cached_ascii: Cell::new(None),
        }
    }

    /// Returns a reference to the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Returns the Python string hash, computing it only on the first call.
    pub fn hash(&self) -> HashValue {
        if let Some(hash) = self.cached_hash.get() {
            hash
        } else {
            // Use the same hash as inline and interned strings for dictionary lookup.
            let hash = hash_python_str(self.as_str());
            self.cached_hash.set(Some(hash));
            hash
        }
    }

    /// Whether every character is ASCII, scanning the text only on the first call.
    pub fn is_ascii(&self) -> bool {
        if let Some(ascii) = self.cached_ascii.get() {
            ascii
        } else {
            let ascii = self.text.is_ascii();
            self.cached_ascii.set(Some(ascii));
            ascii
        }
    }

    /// The length in Unicode code points, as Python's `len()` reports it.
    pub fn char_len(&self) -> usize {
        if self.is_ascii() {
            self.text.len()
        } else {
            self.text.chars().count()
        }
    }
}

impl ops::Deref for HeapStr {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl HeapItem for HeapStr {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No-op: strings don't hold Value references
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_and_text_access_ignore_cached_hashes() {
        let text = "aé🐍b";
        let hashed = HeapStr::new(Box::<str>::from(text));
        let unhashed = HeapStr::new(Box::<str>::from(text));
        assert_eq!(hashed.hash(), hash_python_str(text));
        assert_eq!(hashed, unhashed);
        assert_ne!(hashed, HeapStr::new(Box::<str>::from("different")));

        let borrowed: &str = &hashed;
        assert_eq!(borrowed, text);
        assert_eq!(hashed.char_len(), 4);
        assert!(!hashed.is_ascii());
    }
}
