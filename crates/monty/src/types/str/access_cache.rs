//! Lazy character-to-byte offsets for immutable Python strings stored as UTF-8.
//!
//! ASCII access uses byte offsets directly. Unicode access caches its character length
//! and last character/byte position, so nearby reads can resume without scanning a prefix.
//! Forward skipping uses `Chars::nth`; reverse walks are limited to eight characters
//! because distant reverse skipping was slower in the index-policy experiments.
//!
//! Sustained distant access builds byte-offset checkpoints. Strings of at most
//! 255 bytes use u8 offsets every 8 characters; up to 65,535 bytes use u16 every
//! 16 characters; larger strings use u32 every 32 characters, or usize above 4 GiB.
//! The u8/u16/u32 policies each cost about one eighth of a byte per character.
//! German/Chinese random-access workloads improved 6–16% with this spacing; keeping
//! stride 32 with narrower offsets saved memory but barely changed lookup time.
//! Construction waits until long traversals scan 32 times the text's byte length;
//! counting bytes rather than requests avoids building indexes for short local reads.
//! The earlier 16x budget built too soon for 64-access strings and hurt their runtime.
//!
//! Cursor state stays boxed to keep unused per-string metadata small. An inline
//! `Cell`/`OnceCell` experiment saved about 0.35 MiB across 3,000 Unicode strings but
//! regressed several access workloads by 2–4%, so the boxed layout remains the default.
//! These measurements do not isolate the CPU cause; see `scripts/string_access/README.md` in Flying Circus
//! for workloads and the remaining moderate-access overhead.
//!
//! Checkpoint allocation is resource-preflighted. All caches are derived, reset on clone and omitted
//! from snapshots; the text alone determines their validity.
use std::{
    cell::{Cell, RefCell},
    fmt::Debug,
};

use monty_types::ResourceTracker;

use crate::exception_private::RunResult;

// Power-of-two strides let lookups use shifts even when the offset width varies.
const STRIDE: usize = 32;
const SHORT_STRIDE: usize = 8;
const MEDIUM_STRIDE: usize = 16;
const SCAN_BUDGET: usize = 32;

/// Belongs to one immutable string; offsets cannot be reused with different text.
#[derive(Debug)]
pub(crate) struct AccessCache {
    // Zero means unknown; every string length is representable after adding one.
    length: Cell<usize>,
    state: RefCell<Option<Box<State>>>,
}

impl Default for AccessCache {
    fn default() -> Self {
        Self {
            length: Cell::new(0),
            state: RefCell::new(None),
        }
    }
}

impl Clone for AccessCache {
    fn clone(&self) -> Self {
        // Cloning text must stay cheap even when its source has built a large index.
        Self::default()
    }
}

/// Allocated only after a nontrivial Unicode access; local reads need no offset table.
#[derive(Debug, Default)]
struct State {
    /// The same character/byte boundary serves both conversion directions.
    last: (usize, usize),
    /// Bytes traversed by nonlocal reads before checkpoints have been built.
    scanned: usize,
    offsets: Offsets,
}

/// Entry `i` stores the UTF-8 byte offset of character `i * stride`.
/// The width is chosen from the byte length; finer strides spend the saved bytes on more anchors.
#[derive(Debug, Default)]
enum Offsets {
    #[default]
    Empty,
    Small(Box<[u8]>),
    Medium(Box<[u16]>),
    Compact(Box<[u32]>),
    Wide(Box<[usize]>),
}

impl Offsets {
    fn shift(&self) -> u32 {
        match self {
            Self::Small(_) => SHORT_STRIDE.trailing_zeros(),
            Self::Medium(_) => MEDIUM_STRIDE.trailing_zeros(),
            _ => STRIDE.trailing_zeros(),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Small(v) => v.len(),
            Self::Medium(v) => v.len(),
            Self::Compact(v) => v.len(),
            Self::Wide(v) => v.len(),
        }
    }

    fn get(&self, index: usize) -> usize {
        match self {
            Self::Empty => 0,
            Self::Small(v) => usize::from(v[index]),
            Self::Medium(v) => usize::from(v[index]),
            Self::Compact(v) => v[index] as usize,
            Self::Wide(v) => v[index],
        }
    }

    /// Finds the last checkpoint at or before a known byte boundary.
    /// An empty table supplies `(0, 0)`, so cold lookups scan from the string's start.
    fn anchor(&self, byte: usize) -> (usize, usize) {
        let mut low = 0;
        let mut high = self.len();
        while low < high {
            let mid = low + (high - low) / 2;
            if self.get(mid) <= byte {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let checkpoint = low.saturating_sub(1);
        (checkpoint << self.shift(), self.get(checkpoint))
    }
}

impl AccessCache {
    #[cfg(test)]
    pub(crate) fn has_checkpoints(&self) -> bool {
        self.state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.offsets.len() != 0)
    }

    pub fn char_len(&self, text: &str) -> usize {
        if let Some(length) = self.length.get().checked_sub(1) {
            length
        } else {
            let length = text.chars().count();
            self.length.set(length + 1);
            length
        }
    }

    pub fn character(&self, text: &str, index: i64, tracker: &ResourceTracker) -> RunResult<Option<char>> {
        // Endpoints neither allocate a cursor nor trigger an index build.
        if index == 0 {
            return Ok(text.chars().next());
        }
        if index == -1 {
            return Ok(text.chars().next_back());
        }
        // Positive indices can look up a byte offset without first counting all characters.
        let target = if index < 0 {
            let Ok(distance) = usize::try_from(index.unsigned_abs()) else {
                return Ok(None);
            };
            let Some(target) = self.char_len(text).checked_sub(distance) else {
                return Ok(None);
            };
            target
        } else {
            let Ok(target) = usize::try_from(index) else {
                return Ok(None);
            };
            target
        };
        let byte = self.byte_offset(text, target, tracker)?;
        Ok(text[byte..].chars().next())
    }

    /// Maps a character boundary to bytes, clamping beyond the end like slicing.
    pub fn byte_offset(&self, text: &str, target: usize, tracker: &ResourceTracker) -> RunResult<usize> {
        if target == 0 {
            return Ok(0);
        }
        if target == usize::MAX || self.length.get().checked_sub(1).is_some_and(|length| target >= length) {
            return Ok(text.len());
        }
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let shift = state.offsets.shift();
        let checkpoint = target >> shift;
        // Start at a checkpoint when present; the cursor below can shorten the remaining walk.
        let (mut character, mut byte) = if checkpoint < state.offsets.len() {
            (checkpoint << shift, state.offsets.get(checkpoint))
        } else {
            (0, 0)
        };
        let (last_character, last_byte) = state.last;
        // Reuse a later forward boundary, or a nearby cursor just beyond the target.
        if last_character <= target && last_character >= character {
            (character, byte) = state.last;
        } else if last_character > target && last_character - target <= 8 {
            let mut chars = text[..last_byte].chars();
            chars.nth_back(last_character - target - 1);
            let result = chars.as_str().len();
            state.last = (target, result);
            return Ok(result);
        }
        if target == character {
            return Ok(byte);
        }
        let mut chars = text[byte..].chars();
        // Consume only preceding characters, leaving the suffix at the requested byte boundary.
        if chars.nth(target - character - 1).is_none() {
            // Remember the length so repeated invalid accesses do not rescan.
            self.char_len(text);
            return Ok(text.len());
        }
        let result = text.len() - chars.as_str().len();
        state.last = (target, result);
        self.charge(text, state, target - character, result - byte, tracker)?;
        Ok(result)
    }

    /// Converts a known UTF-8 boundary returned by substring matching.
    /// The byte position must be in bounds and at a character boundary; this does not round arbitrary bytes.
    pub fn char_offset(&self, text: &str, target: usize, tracker: &ResourceTracker) -> RunResult<usize> {
        if target == 0 {
            return Ok(0);
        }
        if target == text.len() {
            return Ok(self.char_len(text));
        }
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let (mut character, mut byte) = state.offsets.anchor(target);
        // Search results often advance from the previous match, beyond the nearest checkpoint.
        if state.last.1 <= target && state.last.1 >= byte {
            (character, byte) = state.last;
        }
        let traversed = text[byte..target].chars().count();
        let result = character + traversed;
        state.last = (result, target);
        self.charge(text, state, traversed, target - byte, tracker)?;
        Ok(result)
    }

    /// Counts only costly traversals so local reads do not pay for checkpoint construction.
    /// A single allocation preflight covers the known-size offset buffer.
    fn charge(
        &self,
        text: &str,
        state: &mut State,
        characters: usize,
        bytes: usize,
        tracker: &ResourceTracker,
    ) -> RunResult<()> {
        if state.offsets.len() != 0 || characters <= 8 {
            return Ok(());
        }
        state.scanned = state.scanned.saturating_add(bytes);
        if state.scanned < text.len().saturating_mul(SCAN_BUDGET) {
            return Ok(());
        }
        let length = self.char_len(text);
        // Stored offsets address bytes, so character count alone cannot determine their width.
        state.offsets = if u8::try_from(text.len()).is_ok() {
            Offsets::Small(build_offsets::<u8, SHORT_STRIDE>(text, length, tracker)?)
        } else if u16::try_from(text.len()).is_ok() {
            Offsets::Medium(build_offsets::<u16, MEDIUM_STRIDE>(text, length, tracker)?)
        } else if u32::try_from(text.len()).is_ok() {
            Offsets::Compact(build_offsets::<u32, STRIDE>(text, length, tracker)?)
        } else {
            Offsets::Wide(build_offsets::<usize, STRIDE>(text, length, tracker)?)
        };
        Ok(())
    }
}

/// Preflights the exact checkpoint storage so allocation does not grow incrementally.
fn build_offsets<T, const STEP: usize>(text: &str, length: usize, tracker: &ResourceTracker) -> RunResult<Box<[T]>>
where
    T: TryFrom<usize>,
    T::Error: Debug,
{
    let count = length.div_ceil(STEP);
    tracker.check_allocation(count.saturating_mul(size_of::<T>()))?;
    let mut offsets = Vec::with_capacity(count);
    for (character, (byte, _)) in text.char_indices().enumerate() {
        tracker.check_time_every(character)?;
        if character % STEP == 0 {
            offsets.push(T::try_from(byte).expect("offset width checked before building"));
        }
    }
    Ok(offsets.into_boxed_slice())
}

#[cfg(test)]
#[path = "../../../tests/unit/str_access_cache.rs"]
mod tests;
