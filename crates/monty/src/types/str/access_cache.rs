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
    ops::Range,
};

use ahash::AHashMap;
use monty_types::ResourceTracker;

use crate::{
    exception_private::RunResult,
    hash::{HashValue, hash_python_str},
};

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
    /// Search/index navigation must not be displaced by repeated view-origin lookups.
    last: Cursor,
    view_start: Cursor,
    /// UTF-8 bytes traversed by walks longer than eight characters, before checkpoints exist.
    scanned: usize,
    /// Sparse anchors built after repeated long traversals exhaust the scan budget.
    offsets: Offsets,
    view_hashes: ViewHashes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ViewRange {
    start: u32,
    len: u32,
}

#[derive(Debug, Clone, Copy)]
struct ViewHash {
    range: ViewRange,
    hash: HashValue,
}

#[derive(Debug)]
enum ViewHashes {
    Inline([Option<ViewHash>; 4]),
    Table(AHashMap<ViewRange, HashValue>),
}

impl Default for ViewHashes {
    fn default() -> Self {
        Self::Inline([None; 4])
    }
}

impl ViewHashes {
    fn get(&self, range: ViewRange) -> Option<HashValue> {
        match self {
            Self::Inline(entries) => entries
                .iter()
                .flatten()
                .find(|entry| entry.range == range)
                .map(|entry| entry.hash),
            Self::Table(table) => table.get(&range).copied(),
        }
    }

    fn insert(&mut self, range: ViewRange, hash: HashValue, tracker: &ResourceTracker) -> RunResult<()> {
        match self {
            Self::Inline(entries) => {
                if let Some(slot) = entries.iter_mut().find(|entry| entry.is_none()) {
                    *slot = Some(ViewHash { range, hash });
                    return Ok(());
                }
                tracker.check_allocation(8 * size_of::<(ViewRange, HashValue)>() * 2)?;
                let mut table = AHashMap::with_capacity(8);
                for entry in entries.iter().flatten() {
                    table.insert(entry.range, entry.hash);
                }
                table.insert(range, hash);
                *self = Self::Table(table);
            }
            Self::Table(table) => {
                // Hash-table buckets also contain control bytes and unused load-factor capacity.
                tracker.check_growth(table.len(), table.capacity(), size_of::<(ViewRange, HashValue)>() * 2)?;
                table.insert(range, hash);
            }
        }
        Ok(())
    }
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

impl State {
    fn locate_view_start(&mut self, text: &str, target: usize) -> (usize, usize, usize) {
        if self.view_start.byte_offset == target {
            return (self.view_start.character_index, 0, 0);
        }
        let navigation = self.last;
        let result = self.locate_byte(text, target);
        self.view_start = self.last;
        self.last = navigation;
        result
    }

    fn locate_byte(&mut self, text: &str, target: usize) -> (usize, usize, usize) {
        // A nearby backward byte boundary should not restart at a checkpoint.
        if self.last.byte_offset > target && self.last.byte_offset - target <= 8 * 4 {
            let distance = text[target..self.last.byte_offset].chars().count();
            if distance <= 8 {
                let result = self.last.character_index - distance;
                let bytes = self.last.byte_offset - target;
                self.last = Cursor {
                    character_index: result,
                    byte_offset: target,
                };
                return (result, distance, bytes);
            }
        }
        let Cursor {
            character_index: mut character,
            byte_offset: mut byte,
        } = self.offsets.anchor(target);
        if self.view_start.byte_offset <= target && self.view_start.byte_offset >= byte {
            character = self.view_start.character_index;
            byte = self.view_start.byte_offset;
        }
        if self.last.byte_offset <= target && self.last.byte_offset >= byte {
            character = self.last.character_index;
            byte = self.last.byte_offset;
        }
        let traversed = text[byte..target].chars().count();
        let result = character + traversed;
        self.last = Cursor {
            character_index: result,
            byte_offset: target,
        };
        (result, traversed, target - byte)
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
        self.prepare_index(text, tracker)?;
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
        if state.view_start.character_index <= target && state.view_start.character_index >= character {
            character = state.view_start.character_index;
            byte = state.view_start.byte_offset;
        }
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
        self.prepare_index(text, tracker)?;
        if target == 0 {
            return Ok(0);
        }
        if target == text.len() {
            return Ok(self.char_len(text));
        }
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let (result, traversed, bytes) = state.locate_byte(text, target);
        self.charge(text, state, traversed, bytes, tracker)?;
        Ok(result)
    }

    /// Keeps a view's origin separate from the most recent search/index boundary.
    pub(super) fn view_start(&self, text: &str, target: usize, tracker: &ResourceTracker) -> RunResult<usize> {
        self.prepare_index(text, tracker)?;
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let (result, characters, bytes) = state.locate_view_start(text, target);
        self.charge(text, state, characters, bytes, tracker)?;
        Ok(result)
    }

    /// Keeps four hashes in the lazy state, then promotes to a table without evicting older ranges.
    pub(super) fn view_hash(&self, text: &str, range: Range<usize>, tracker: &ResourceTracker) -> RunResult<HashValue> {
        // Inline views already check both bounds before narrowing them to u32.
        let key = ViewRange {
            start: u32::try_from(range.start).expect("view start fits u32"),
            len: u32::try_from(range.len()).expect("view length fits u32"),
        };
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        if let Some(hash) = state.view_hashes.get(key) {
            return Ok(hash);
        }
        let hash = hash_python_str(&text[range]);
        state.view_hashes.insert(key, hash, tracker)?;
        Ok(hash)
    }

    /// Counts a view from backing coordinates without allocating a sparse index.
    /// Existing checkpoints and the backing's cursor are shared by all views.
    pub(super) fn char_len_range(&self, text: &str, range: Range<usize>) -> usize {
        let start = {
            let mut state = self.state.borrow_mut();
            let state = state.get_or_insert_with(Box::default);
            let (result, characters, bytes) = state.locate_view_start(text, range.start);
            if state.offsets.len() == 0 && characters > 8 {
                state.scanned = state.scanned.saturating_add(bytes);
            }
            result
        };
        let end = self.char_offset_for_length(text, range.end);
        end - start
    }

    fn char_offset_for_length(&self, text: &str, target: usize) -> usize {
        if target == 0 {
            return 0;
        }
        if target == text.len() {
            return self.char_len(text);
        }
        let mut state = self.state.borrow_mut();
        let state = state.get_or_insert_with(Box::default);
        let (result, characters, bytes) = state.locate_byte(text, target);
        // len() is infallible; defer any sparse-index allocation to a fallible lookup.
        if state.offsets.len() == 0 && characters > 8 {
            state.scanned = state.scanned.saturating_add(bytes);
        }
        result
    }

    /// Range lengths cannot fail; fallible offset lookups build any index they made worthwhile.
    pub(super) fn prepare_index(&self, text: &str, tracker: &ResourceTracker) -> RunResult<()> {
        let mut state = self.state.borrow_mut();
        if let Some(state) = state.as_mut()
            && state.offsets.len() == 0
            && state.scanned >= text.len().saturating_mul(SCAN_BUDGET)
        {
            self.charge(text, state, 0, 0, tracker)?;
        }
        Ok(())
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
        if state.offsets.len() != 0 {
            return Ok(());
        }
        if characters > 8 {
            state.scanned = state.scanned.saturating_add(bytes);
        }
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
    fn view_origins_do_not_displace_search_boundaries() {
        let text = "aé中🐍".repeat(1024);
        let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).collect();
        let cache = AccessCache::default();
        let tracker = ResourceTracker::default();
        assert_eq!(cache.view_start(&text, boundaries[100], &tracker).unwrap(), 100);
        assert_eq!(cache.char_offset(&text, boundaries[250], &tracker).unwrap(), 250);
        for _ in 0..100 {
            assert_eq!(cache.char_len_range(&text, boundaries[100]..text.len()), 3996);
            assert_eq!(cache.view_start(&text, boundaries[100], &tracker).unwrap(), 100);
            assert_eq!(cache.state.borrow().as_ref().unwrap().last.byte_offset, boundaries[250]);
            assert_eq!(cache.byte_offset(&text, 250, &tracker).unwrap(), boundaries[250]);
        }
        assert_eq!(cache.view_start(&text, boundaries[500], &tracker).unwrap(), 500);
        assert_eq!(cache.state.borrow().as_ref().unwrap().last.byte_offset, boundaries[250]);
        assert_eq!(cache.byte_offset(&text, 501, &tracker).unwrap(), boundaries[501]);
        for start in [3000, 0, 250, 1] {
            assert_eq!(cache.view_start(&text, boundaries[start], &tracker).unwrap(), start);
            assert_eq!(
                cache.char_len_range(&text, boundaries[start]..boundaries[3500]),
                3500 - start
            );
        }
    }

    #[test]
    fn view_hashes_promote_without_eviction_and_reset_on_clone() {
        let text = "aé中🐍".repeat(1024);
        let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).collect();
        let cache = AccessCache::default();
        let tracker = ResourceTracker::default();
        for i in 0..128 {
            let range = boundaries[i]..boundaries[i + 1000];
            assert_eq!(
                cache.view_hash(&text, range.clone(), &tracker).unwrap(),
                hash_python_str(&text[range])
            );
            let state = cache.state.borrow();
            match &state.as_ref().unwrap().view_hashes {
                ViewHashes::Inline(_) => assert!(i < 4),
                ViewHashes::Table(table) => assert_eq!(table.len(), i + 1),
            }
        }
        let state = cache.state.borrow();
        for i in 0..128 {
            let range = boundaries[i]..boundaries[i + 1000];
            let key = ViewRange {
                start: u32::try_from(range.start).unwrap(),
                len: u32::try_from(range.len()).unwrap(),
            };
            assert_eq!(
                state.as_ref().unwrap().view_hashes.get(key),
                Some(hash_python_str(&text[range]))
            );
        }
        assert!(cache.clone().state.borrow().is_none());
    }

    #[test]
    fn view_hash_promotion_and_growth_fail_without_losing_cached_entries() {
        let text = "aé中🐍".repeat(1024);
        let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).collect();
        let cache = AccessCache::default();
        let tracker = ResourceTracker::default();
        let limited = ResourceTracker::new(ResourceLimits {
            max_memory: Some(0),
            ..ResourceLimits::default()
        });
        for i in 0..4 {
            cache
                .view_hash(&text, boundaries[i]..boundaries[i + 100], &limited)
                .unwrap();
        }
        let fifth = boundaries[4]..boundaries[104];
        assert!(cache.view_hash(&text, fifth.clone(), &limited).is_err());
        assert!(matches!(
            &cache.state.borrow().as_ref().unwrap().view_hashes,
            ViewHashes::Inline(_)
        ));
        cache.view_hash(&text, fifth, &tracker).unwrap();
        let capacity = match &cache.state.borrow().as_ref().unwrap().view_hashes {
            ViewHashes::Table(table) => table.capacity(),
            ViewHashes::Inline(_) => panic!("fifth hash promotes to a table"),
        };
        for i in 5..capacity {
            cache
                .view_hash(&text, boundaries[i]..boundaries[i + 100], &limited)
                .unwrap();
        }
        let next = boundaries[capacity]..boundaries[capacity + 100];
        assert!(cache.view_hash(&text, next.clone(), &limited).is_err());
        for i in 0..capacity {
            assert_eq!(
                cache
                    .view_hash(&text, boundaries[i]..boundaries[i + 100], &limited)
                    .unwrap(),
                hash_python_str(&text[boundaries[i]..boundaries[i + 100]])
            );
        }
        cache.view_hash(&text, next, &tracker).unwrap();
    }

    #[test]
    fn nearby_backward_byte_offsets_preserve_local_walks() {
        let text = "aé中🐍".repeat(1024);
        let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).collect();
        let mut state = State {
            last: Cursor {
                character_index: 1500,
                byte_offset: boundaries[1500],
            },
            ..State::default()
        };
        for target in (1492..1500).rev() {
            let (character, walked, _) = state.locate_byte(&text, boundaries[target]);
            assert_eq!(character, target);
            assert_eq!(walked, 1);
        }
        assert_eq!(state.offsets.len(), 0);
    }

    #[test]
    fn range_lengths_trigger_resource_checked_indexes_on_the_next_lookup() {
        let text = "aé中🐍".repeat(1024);
        let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).collect();
        let cache = AccessCache::default();
        for n in 0..512 {
            let end = if n % 2 == 0 { 3000 } else { 1 };
            assert_eq!(cache.char_len_range(&text, 0..boundaries[end]), end);
        }
        assert!(!cache.has_checkpoints());
        let limited = ResourceTracker::new(ResourceLimits {
            max_memory: Some(0),
            ..ResourceLimits::default()
        });
        // Even an endpoint lookup must honor the pending build and its allocation failure.
        assert!(cache.byte_offset(&text, 0, &limited).is_err());
        assert!(!cache.has_checkpoints());
        assert_eq!(cache.byte_offset(&text, 0, &ResourceTracker::default()).unwrap(), 0);
        assert!(cache.has_checkpoints());
        for start in [1, 41, 257, 1500] {
            assert_eq!(
                cache.char_len_range(&text, boundaries[start]..boundaries[3000]),
                3000 - start
            );
        }
    }

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
