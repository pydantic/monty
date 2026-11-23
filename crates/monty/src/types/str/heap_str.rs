//! Heap storage for Python strings.

use std::{cell::Cell, ops};

use crate::{
    hash::{HashValue, hash_python_str},
    heap::{HeapId, HeapItem},
};

/// Heap string with lazy character-count, ASCII and Python hash metadata.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct HeapStr {
    text: Box<str>,
    #[serde(skip)]
    pub(super) cached_hash: Cell<Option<HashValue>>,
    #[serde(skip)]
    // 0: unknown; 1: non-ASCII with unknown count; >= 2: character count.
    char_len: Cell<usize>,
}

// Loading leaves metadata lazy, just like constructing a string from owned text.
impl<'de> serde::Deserialize<'de> for HeapStr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <Box<str> as serde::Deserialize>::deserialize(deserializer).map(Self::new)
    }
}

impl PartialEq for HeapStr {
    /// Equality depends on text, independently of metadata and the cached hash.
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl HeapStr {
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

    /// Creates a heap string from owned text.
    ///
    /// Use [`super::allocate_string`] or [`super::allocate_string_no_interning`] for values.
    #[must_use]
    pub(super) fn new(text: Box<str>) -> Self {
        Self {
            text,
            cached_hash: Cell::new(None),
            char_len: Cell::new(0),
        }
    }

    /// Uses exact ASCII status supplied by the producing operation.
    pub(super) fn with_ascii(ascii: bool, text: Box<str>) -> Self {
        debug_assert_eq!(ascii, text.is_ascii());
        let value = Self::new(text);
        if ascii {
            value.cache_char_len(value.text.len());
        } else {
            value.char_len.set(1);
        }
        value
    }

    /// Uses a character count accumulated by the producing operation.
    pub(super) fn with_char_len(text: Box<str>, char_len: usize) -> Self {
        debug_assert_eq!(char_len, text.chars().count());
        let value = Self::new(text);
        value.cache_char_len(char_len);
        value
    }

    fn cache_char_len(&self, char_len: usize) {
        // Count 1 overlaps the non-ASCII sentinel. ASCII singletons remain unknown;
        // empty and single-character strings cost at most four bytes to recount.
        self.char_len.set(if char_len == 1 && self.text.len() == 1 {
            0
        } else {
            char_len
        });
    }

    /// Reuses known counts, recounting at most four bytes for short heap strings.
    pub(super) fn known_char_len(&self) -> Option<usize> {
        match self.char_len.get() {
            count @ 2.. => Some(count),
            _ if self.text.len() <= 4 => Some(self.char_len()),
            _ => None,
        }
    }

    /// Polls during a first offset lookup. An interrupted count leaves the cache untouched.
    pub(super) fn checked_char_len<E>(&self, check_time: impl Fn() -> Result<(), E>) -> Result<usize, E> {
        if let Some(count) = self.known_char_len() {
            return Ok(count);
        }
        let count = super::fast_offsets::count_chars(self.as_str(), check_time)?;
        self.cache_char_len(count);
        Ok(count)
    }

    /// Returns a reference to the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Checks ASCII independently when the character count is unknown.
    #[inline]
    pub fn is_ascii(&self) -> bool {
        match self.char_len.get() {
            0 => {
                let ascii = self.text.is_ascii();
                if ascii {
                    self.cache_char_len(self.text.len());
                } else {
                    self.char_len.set(1);
                }
                ascii
            }
            1 => false,
            count => count == self.text.len(),
        }
    }

    /// ASCII search results can use byte offsets as character indices.
    /// A single non-ASCII byte settles the classification while leaving the character count lazy.
    pub(super) fn checked_is_ascii<E>(&self, check_time: impl Fn() -> Result<(), E>) -> Result<bool, E> {
        const CHUNK: usize = 64 * 1024;
        // Cached metadata answers in O(1), and a short scan fits between deadline polls.
        if self.char_len.get() != 0 || self.text.len() < CHUNK {
            return Ok(self.is_ascii());
        }
        // An uncached ASCII string can take a full pass to classify. Poll before each chunk.
        for chunk in self.text.as_bytes().chunks(CHUNK) {
            check_time()?;
            if !chunk.is_ascii() {
                self.char_len.set(1);
                return Ok(false);
            }
        }
        // Each ASCII byte is one character, so the same pass establishes the exact count.
        self.cache_char_len(self.text.len());
        Ok(true)
    }

    /// Counts Unicode code points on first use, then reuses counts of at least two.
    #[inline]
    pub fn char_len(&self) -> usize {
        let count = self.char_len.get();
        if count >= 2 {
            return count;
        }
        let count = bytecount::num_chars(self.text.as_bytes());
        self.cache_char_len(count);
        count
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
        // Strings do not hold heap references.
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

#[cfg(test)]
#[path = "../../../tests/unit/heap_string_metadata.rs"]
mod metadata_tests;
