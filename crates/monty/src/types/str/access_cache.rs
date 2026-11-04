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

/// Belongs to one immutable nonempty string; offsets cannot be reused with different text.
#[derive(Debug)]
pub(crate) struct AccessCache {
    // None means the character length has not been counted yet.
    length: Cell<Option<NonZeroUsize>>,
    state: RefCell<Option<Box<State>>>,
}

impl Default for AccessCache {
    fn default() -> Self {
        Self {
            length: Cell::new(None),
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

/// Allocated only after a nontrivial Unicode access.
#[derive(Debug, Default)]
struct State {
    /// The same character/byte boundary serves both conversion directions.
    last: (usize, usize),
}

impl AccessCache {
    #[cfg(test)]
    pub(crate) fn has_cursor(&self) -> bool {
        self.state.borrow().is_some()
    }

    pub fn char_len(&self, text: &str) -> usize {
        if let Some(length) = self.length.get() {
            length.get()
        } else {
            let length = NonZeroUsize::new(text.chars().count()).expect("access caches require nonempty text");
            self.length.set(Some(length));
            length.get()
        }
    }

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

    /// Maps a character boundary to bytes, clamping beyond the end like slicing.
    pub fn byte_offset(&self, text: &str, target: usize) -> usize {
        if target == 0 {
            return 0;
        }
        if target == usize::MAX || self.length.get().is_some_and(|length| target >= length.get()) {
            return text.len();
        }
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let (mut character, mut byte) = (0, 0);
        let (last_character, last_byte) = state.last;
        // Reuse an earlier forward boundary, or a nearby cursor just beyond the target.
        if last_character <= target {
            (character, byte) = state.last;
        } else if last_character - target <= 8 {
            let mut chars = text[..last_byte].chars();
            chars.nth_back(last_character - target - 1);
            let result = chars.as_str().len();
            state.last = (target, result);
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
        state.last = (target, result);
        result
    }

    /// Converts a known UTF-8 boundary returned by substring matching.
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
        let (character, byte) = if state.last.1 <= target { state.last } else { (0, 0) };
        let result = character + text[byte..target].chars().count();
        state.last = (result, target);
        result
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/str_access_cache.rs"]
mod tests;
