//! Heap storage for Python strings.

use std::{cell::Cell, ops};

use super::AccessCache;
use crate::{
    hash::{HashValue, hash_python_str},
    heap::{HeapId, HeapItem},
};

/// Heap string with eager character length and a lazily computed Python hash.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct HeapStr {
    text: Box<str>,
    #[serde(skip)]
    pub(super) cached_hash: Cell<Option<HashValue>>,
    #[serde(skip)]
    char_len: usize,
    #[serde(skip)]
    pub(super) access_cache: AccessCache,
}

// Loading rebuilds metadata through the normal constructor.
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
        Self::with_ascii(text.is_ascii(), text)
    }

    /// Uses exact ASCII status supplied by the producing operation.
    pub(super) fn with_ascii(ascii: bool, text: Box<str>) -> Self {
        debug_assert_eq!(ascii, text.is_ascii());
        let length = if ascii {
            text.len()
        } else {
            bytecount::num_chars(text.as_bytes())
        };
        Self::with_char_len(text, length)
    }

    /// Uses a character count accumulated by the producing operation.
    pub(super) fn with_char_len(text: Box<str>, char_len: usize) -> Self {
        debug_assert_eq!(char_len, text.chars().count());
        Self {
            text,
            cached_hash: Cell::new(None),
            char_len,
            access_cache: AccessCache::default(),
        }
    }

    /// Returns a reference to the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// UTF-8 has one byte per code point exactly when the text is ASCII.
    #[inline]
    pub fn is_ascii(&self) -> bool {
        self.char_len == self.text.len()
    }

    /// Returns the stored Unicode code-point length.
    #[inline]
    pub fn char_len(&self) -> usize {
        self.char_len
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
    use super::{super::access_cache::test_budget::Unlimited, *};

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

    #[test]
    fn cursor_state_is_omitted_from_dumps_and_reset_on_clone() {
        let text = HeapStr::new("aé中🐍".repeat(4096).into_boxed_str());
        let cold = minicbor_serde::to_vec(&text).unwrap();
        for i in 0..1000 {
            let position = i * 7919 % 16384;
            assert_eq!(
                text.access_cache
                    .character(text.as_str(), position, text.char_len(), &Unlimited)
                    .unwrap(),
                "aé中🐍".chars().nth(usize::try_from(position % 4).unwrap())
            );
        }
        assert!(text.access_cache.has_cursor());
        assert_eq!(minicbor_serde::to_vec(&text).unwrap(), cold);
        let restored: HeapStr = minicbor_serde::from_slice(&cold).unwrap();
        for reset in [text.clone(), restored] {
            assert!(!reset.access_cache.has_cursor());
            assert_eq!(reset.char_len(), text.char_len());
            assert_eq!(minicbor_serde::to_vec(&reset).unwrap(), cold);
        }
    }
}
