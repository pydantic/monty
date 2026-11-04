//! Cached character length and cursor for immutable Python strings stored as UTF-8.
//!
//! Unicode access remembers the last character/byte boundary so nearby reads can resume
//! without scanning a prefix. Forward skipping uses `Chars::nth`; reverse walks are limited
//! to eight characters because distant reverse skipping was slower in the access experiments.
//!
//! Cursor state is boxed and allocated only after a nontrivial Unicode access.
//! Clones and snapshots discard these derived caches; the text determines their validity.
use std::{
    cell::{Cell, RefCell},
    num::NonZeroUsize,
};

/// Caches character length and access positions for one immutable nonempty string.
/// Every method must receive the same text; cached byte boundaries cannot be reused for other strings.
/// Clones start uncached, and the owning string omits this derived state from snapshots.
#[derive(Debug)]
pub(crate) struct AccessCache {
    /// The actual character count; `None` means it has not been counted yet.
    char_length: Cell<Option<NonZeroUsize>>,
    /// Allocated only when an access needs to retain a cursor.
    state: RefCell<Option<Box<State>>>,
}

impl Default for AccessCache {
    fn default() -> Self {
        Self {
            char_length: Cell::new(None),
            state: RefCell::new(None),
        }
    }
}

impl Clone for AccessCache {
    fn clone(&self) -> Self {
        // Cloning text must not retain the source's access history.
        Self::default()
    }
}

/// One UTF-8 boundary expressed in both Python character and byte coordinates.
#[derive(Debug, Default, Clone, Copy)]
struct Cursor {
    /// Zero-based Unicode code-point index.
    character_index: usize,
    /// Byte offset of the same boundary in the original text.
    byte_offset: usize,
}

/// Allocated only after a nontrivial Unicode access.
#[derive(Debug, Default)]
struct State {
    /// The same character/byte boundary serves both conversion directions.
    last: Cursor,
}

impl AccessCache {
    #[cfg(test)]
    pub(crate) fn has_cursor(&self) -> bool {
        self.state.borrow().is_some()
    }

    /// Counts Unicode code points on first use, then returns the cached count.
    /// Unexpected empty text returns zero without populating the cache.
    pub fn char_len(&self, text: &str) -> usize {
        if let Some(char_length) = self.char_length.get() {
            char_length.get()
        } else {
            let Some(char_length) = NonZeroUsize::new(text.chars().count()) else {
                // Callers should bypass the cache for empty strings; leave the count unset if one reaches here.
                return 0;
            };
            self.char_length.set(Some(char_length));
            char_length.get()
        }
    }

    /// Reads a Python character index, including negative offsets from the end.
    /// Out-of-range indices return `None`; endpoint reads do not allocate cursor state.
    pub fn character(&self, text: &str, index: i64) -> Option<char> {
        // Endpoints do not allocate a cursor.
        if index == 0 {
            return text.chars().next();
        }
        if index == -1 {
            return text.chars().next_back();
        }
        // Positive indices can look up a byte offset without counting all characters.
        let target = if index < 0 {
            let distance = usize::try_from(index.unsigned_abs()).ok()?;
            self.char_len(text).checked_sub(distance)?
        } else {
            usize::try_from(index).ok()?
        };
        let byte = self.byte_offset(text, target);
        text[byte..].chars().next()
    }

    /// Maps a character index to its UTF-8 byte boundary, clamping beyond the end.
    /// `usize::MAX` denotes an omitted slice end and avoids counting characters.
    pub fn byte_offset(&self, text: &str, target: usize) -> usize {
        if target == 0 {
            return 0;
        }
        if target == usize::MAX || self.char_length.get().is_some_and(|length| target >= length.get()) {
            return text.len();
        }
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let (mut character, mut byte) = (0, 0);
        let Cursor {
            character_index: last_character,
            byte_offset: last_byte,
        } = state.last;
        // Reuse an earlier forward boundary, or a nearby cursor just beyond the target.
        if last_character <= target {
            character = state.last.character_index;
            byte = state.last.byte_offset;
        } else if last_character - target <= 8 {
            let mut chars = text[..last_byte].chars();
            chars.nth_back(last_character - target - 1);
            let result = chars.as_str().len();
            state.last = Cursor {
                character_index: target,
                byte_offset: result,
            };
            return result;
        }
        if target == character {
            return byte;
        }
        let mut chars = text[byte..].chars();
        // Consume preceding characters, leaving the suffix at the requested boundary.
        if chars.nth(target - character - 1).is_none() {
            // Remember the length so repeated invalid accesses do not rescan.
            self.char_len(text);
            return text.len();
        }
        let result = text.len() - chars.as_str().len();
        state.last = Cursor {
            character_index: target,
            byte_offset: result,
        };
        result
    }

    /// Converts a byte boundary returned by substring search to a Python character index.
    /// The byte position must be in bounds and at a character boundary.
    pub fn char_offset(&self, text: &str, target: usize) -> usize {
        if target == 0 {
            return 0;
        }
        if target == text.len() {
            return self.char_len(text);
        }
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let Cursor {
            character_index: character,
            byte_offset: byte,
        } = if state.last.byte_offset <= target {
            state.last
        } else {
            Cursor::default()
        };
        let result = character + text[byte..target].chars().count();
        state.last = Cursor {
            character_index: result,
            byte_offset: target,
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_local_and_inverse_offsets() {
        for text in ["ascii", "aé中🐍", "abcdefghüijk中🐍"].map(|s| s.repeat(300)) {
            let index = AccessCache::default();
            let characters: Vec<_> = text.chars().collect();
            let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).chain([text.len()]).collect();
            for n in 0..4096 {
                let target = n * 7919 % (characters.len() + 1);
                assert_eq!(index.byte_offset(&text, target), boundaries[target]);
                assert_eq!(index.char_offset(&text, boundaries[target]), target);
                assert_eq!(
                    index.character(&text, i64::try_from(target).unwrap()),
                    characters.get(target).copied()
                );
                let negative = -(i64::try_from(target).unwrap()) - 1;
                let expected = characters.len().checked_sub(target + 1).map(|i| characters[i]);
                assert_eq!(index.character(&text, negative), expected);
            }
            assert_eq!(index.character(&text, i64::MIN), None);
            assert_eq!(index.character(&text, i64::MAX), None);
        }
    }

    #[test]
    fn endpoints_do_not_allocate_a_cursor() {
        let text = "a中é🐍".repeat(4096);
        let index = AccessCache::default();
        for _ in 0..100 {
            assert_eq!(index.character(&text, 0), Some('a'));
            assert_eq!(index.character(&text, -1), Some('🐍'));
        }
        assert!(!index.has_cursor());
        for target in 1..text.chars().count() {
            assert!(index.character(&text, i64::try_from(target).unwrap()).is_some());
        }
        assert!(index.has_cursor());
    }

    #[test]
    fn clone_resets_length_and_cursor() {
        let text = "a中é🐍".repeat(4096);
        let index = AccessCache::default();
        assert_eq!(index.char_len(&text), 16384);
        assert_eq!(index.character(&text, 8000), Some('a'));
        assert!(index.has_cursor());
        let cloned = index.clone();
        assert!(index.has_cursor());
        assert_eq!(index.char_length.get(), NonZeroUsize::new(16384));
        assert_eq!(cloned.char_length.get(), None);
        assert!(!cloned.has_cursor());
        assert_eq!(cloned.character(&text, 8001), Some('中'));
    }

    #[test]
    fn empty_character_count_leaves_the_cache_unset() {
        let cache = AccessCache::default();
        assert_eq!(cache.char_len(""), 0);
        assert_eq!(cache.char_length.get(), None);
        assert!(cache.state.borrow().is_none());
    }

    #[test]
    fn length_cache_stores_the_character_count_directly() {
        let cache = AccessCache::default();
        assert_eq!(cache.byte_offset("aé中🐍", 2), 3);
        assert_eq!(cache.char_length.get(), None);
        assert_eq!(cache.char_len("aé中🐍"), 4);
        assert_eq!(cache.char_length.get(), NonZeroUsize::new(4));
        assert_eq!(cache.byte_offset("aé中🐍", 4), 10);
    }
}
