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
    num::NonZeroUsize,
};

use monty_types::ResourceTracker;

use crate::exception_private::RunResult;

// Power-of-two strides let lookups use shifts even when the offset width varies.
const STRIDE: usize = 32;
const SHORT_STRIDE: usize = 8;
const MEDIUM_STRIDE: usize = 16;
const SCAN_BUDGET: usize = 32;

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
        // Cloning text must stay cheap even when its source has built a large index.
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

/// Allocated only after a nontrivial Unicode access; local reads need no offset table.
#[derive(Debug, Default)]
struct State {
    /// The same character/byte boundary serves both conversion directions.
    last: Cursor,
    /// UTF-8 bytes traversed by walks longer than eight characters, before checkpoints exist.
    scanned: usize,
    /// Sparse anchors built after repeated long traversals exhaust the scan budget.
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
    /// Log2 of checkpoint spacing; a character index selects its slot with a shift.
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

    /// Expands a stored byte offset; an empty table supplies the origin.
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
    /// An empty table supplies the origin, so cold lookups scan from the string's start.
    fn anchor(&self, byte: usize) -> Cursor {
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
        Cursor {
            character_index: checkpoint << self.shift(),
            byte_offset: self.get(checkpoint),
        }
    }
}

impl AccessCache {
    /// A contiguous slice already knows its character count from normalized bounds.
    pub(super) fn with_char_len(length: usize) -> Self {
        Self {
            char_length: Cell::new(NonZeroUsize::new(length)),
            state: RefCell::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn has_checkpoints(&self) -> bool {
        self.state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.offsets.len() != 0)
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

    /// Maps a character index to its UTF-8 byte boundary, clamping beyond the end.
    /// `usize::MAX` denotes an omitted slice end and avoids counting characters.
    pub fn byte_offset(&self, text: &str, target: usize, tracker: &ResourceTracker) -> RunResult<usize> {
        if target == 0 {
            return Ok(0);
        }
        if target == usize::MAX || self.char_length.get().is_some_and(|length| target >= length.get()) {
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
        let Cursor {
            character_index: last_character,
            byte_offset: last_byte,
        } = state.last;
        // Reuse a later forward boundary, or a nearby cursor just beyond the target.
        if last_character <= target && last_character >= character {
            character = state.last.character_index;
            byte = state.last.byte_offset;
        } else if last_character > target && last_character - target <= 8 {
            let mut chars = text[..last_byte].chars();
            chars.nth_back(last_character - target - 1);
            let result = chars.as_str().len();
            state.last = Cursor {
                character_index: target,
                byte_offset: result,
            };
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
        state.last = Cursor {
            character_index: target,
            byte_offset: result,
        };
        self.charge(text, state, target - character, result - byte, tracker)?;
        Ok(result)
    }

    /// Converts a byte boundary returned by substring search to a Python character index.
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
        let Cursor {
            character_index: mut character,
            byte_offset: mut byte,
        } = state.offsets.anchor(target);
        // Search results often advance from the previous match, beyond the nearest checkpoint.
        if state.last.byte_offset <= target && state.last.byte_offset >= byte {
            character = state.last.character_index;
            byte = state.last.byte_offset;
        }
        let traversed = text[byte..target].chars().count();
        let result = character + traversed;
        state.last = Cursor {
            character_index: result,
            byte_offset: target,
        };
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

/// Builds anchors every `STEP` characters after preflighting the complete offset buffer.
/// The caller chooses `T` wide enough for all byte offsets; deadline failures discard the partial table.
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
mod tests {
    use std::{hint::spin_loop, time::Duration};

    use monty_types::ResourceLimits;

    use super::*;

    #[test]
    fn offset_width_and_stride_follow_utf8_byte_length() {
        for bytes in [254, 255, 256, 257, 65_534, 65_535, 65_536, 65_537] {
            let mut text = "aé中🐍".repeat(bytes / 10 + 1);
            let mut end = bytes;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            while text.len() < bytes {
                text.push('x');
            }

            let characters: Vec<_> = text.chars().collect();
            let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).collect();
            let tracker = ResourceTracker::default();
            let index = AccessCache::default();
            let mut state = State {
                scanned: bytes * SCAN_BUDGET,
                ..State::default()
            };
            index.charge(&text, &mut state, 9, 0, &tracker).unwrap();

            let stride = if bytes <= 255 {
                assert!(matches!(&state.offsets, Offsets::Small(_)));
                8
            } else if bytes <= 65_535 {
                assert!(matches!(&state.offsets, Offsets::Medium(_)));
                16
            } else {
                assert!(matches!(&state.offsets, Offsets::Compact(_)));
                32
            };
            assert_eq!(1usize << state.offsets.shift(), stride);
            assert_eq!(state.offsets.len(), characters.len().div_ceil(stride));
            *index.state.borrow_mut() = Some(Box::new(state));

            let mut random = 0x9e37_79b9_7f4a_7c15_u64;
            for _ in 0..2048 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let target = usize::try_from(random % u64::try_from(characters.len()).unwrap()).unwrap();
                assert_eq!(
                    index
                        .character(&text, i64::try_from(target).unwrap(), &tracker)
                        .unwrap(),
                    Some(characters[target])
                );
                assert_eq!(index.char_offset(&text, boundaries[target], &tracker).unwrap(), target);
            }

            let limited = ResourceTracker::new(ResourceLimits {
                max_memory: Some(0),
                ..ResourceLimits::default()
            });
            let mut denied = State {
                scanned: bytes * SCAN_BUDGET,
                ..State::default()
            };
            assert!(index.charge(&text, &mut denied, 9, 0, &limited).is_err());
        }
    }

    #[test]
    fn checkpoint_construction_observes_deadlines_and_can_retry() {
        for limits in [
            ResourceLimits {
                max_feed_duration: Some(Duration::ZERO),
                ..ResourceLimits::default()
            },
            ResourceLimits {
                max_turn_duration: Some(Duration::ZERO),
                ..ResourceLimits::default()
            },
        ] {
            for characters in [128, 1024, 65_536] {
                let text = "é".repeat(characters);
                let index = AccessCache::default();
                let mut state = State {
                    scanned: text.len() * SCAN_BUDGET,
                    ..State::default()
                };
                let tracker = ResourceTracker::new(limits.clone());
                tracker.on_execution_start();
                while tracker.check_time().is_ok() {
                    spin_loop();
                }

                // Force construction directly so a VM dispatch check cannot mask a missing loop check.
                assert!(index.charge(&text, &mut state, 9, 0, &tracker).is_err());
                assert_eq!(state.offsets.len(), 0);

                index
                    .charge(&text, &mut state, 9, 0, &ResourceTracker::default())
                    .unwrap();
                assert!(state.offsets.len() > 0);
            }
        }
    }

    #[test]
    fn random_local_and_inverse_offsets() {
        let tracker = ResourceTracker::default();
        for text in ["ascii", "aé中🐍", "abcdefghüijk中🐍"].map(|s| s.repeat(300)) {
            let index = AccessCache::default();
            let characters: Vec<_> = text.chars().collect();
            let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).chain([text.len()]).collect();
            for n in 0..4096 {
                let target = n * 7919 % (characters.len() + 1);
                assert_eq!(index.byte_offset(&text, target, &tracker).unwrap(), boundaries[target]);
                assert_eq!(index.char_offset(&text, boundaries[target], &tracker).unwrap(), target);
                assert_eq!(
                    index
                        .character(&text, i64::try_from(target).unwrap(), &tracker)
                        .unwrap(),
                    characters.get(target).copied()
                );
                let negative = -(i64::try_from(target).unwrap()) - 1;
                let expected = characters.len().checked_sub(target + 1).map(|i| characters[i]);
                assert_eq!(index.character(&text, negative, &tracker).unwrap(), expected);
            }
            assert_eq!(index.character(&text, i64::MIN, &tracker).unwrap(), None);
            assert_eq!(index.character(&text, i64::MAX, &tracker).unwrap(), None);
        }
    }

    #[test]
    fn endpoints_and_local_reads_do_not_build_offsets() {
        let tracker = ResourceTracker::default();
        let text = "a中é🐍".repeat(4096);
        let index = AccessCache::default();
        for _ in 0..100 {
            index.character(&text, 0, &tracker).unwrap();
            index.character(&text, -1, &tracker).unwrap();
        }
        assert!(index.state.borrow().is_none());
        for target in 1..text.chars().count() {
            index
                .character(&text, i64::try_from(target).unwrap(), &tracker)
                .unwrap();
        }
        assert_eq!(index.state.borrow().as_ref().unwrap().offsets.len(), 0);
    }

    #[test]
    fn adaptive_index_waits_for_sustained_random_access() {
        let tracker = ResourceTracker::default();
        let text = "a中é🐍".repeat(1024);
        let index = AccessCache::default();
        for n in 0..64 {
            index.character(&text, i64::from(n * 7919 % 4096), &tracker).unwrap();
        }
        assert_eq!(index.state.borrow().as_ref().unwrap().offsets.len(), 0);
        for n in 64..4096 {
            index.character(&text, i64::from(n * 7919 % 4096), &tracker).unwrap();
        }
        assert!(index.state.borrow().as_ref().unwrap().offsets.len() > 0);
    }

    #[test]
    fn allocation_limit_and_clone_reset() {
        let text = "a中é🐍".repeat(4096);
        let index = AccessCache::default();
        let tracker = ResourceTracker::new(ResourceLimits {
            max_memory: Some(0),
            ..ResourceLimits::default()
        });
        // Alternating distant positions exhausts the scan budget.
        let mut rejected = false;
        for n in 0..256 {
            if index
                .character(&text, if n % 2 == 0 { 8000 } else { 1 }, &tracker)
                .is_err()
            {
                rejected = true;
                break;
            }
        }
        assert!(rejected);
        let cloned = index.clone();
        assert!(index.state.borrow().is_some());
        assert_eq!(cloned.char_length.get(), None);
        assert!(cloned.state.borrow().is_none());
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
        let tracker = ResourceTracker::default();
        let cache = AccessCache::default();
        assert_eq!(cache.byte_offset("aé中🐍", 2, &tracker).unwrap(), 3);
        assert_eq!(cache.char_length.get(), None);
        assert_eq!(cache.char_len("aé中🐍"), 4);
        assert_eq!(cache.char_length.get(), NonZeroUsize::new(4));
        assert_eq!(cache.byte_offset("aé中🐍", 4, &tracker).unwrap(), 10);
    }
}
