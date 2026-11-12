//! Convert between Python code-point indices and UTF-8 byte offsets using two recent boundaries.
//! Start and end are implicit anchors; each lookup chooses the closest anchor in its input coordinates.
//! Skips use scalar iteration, 16-byte SIMD or bulk bytecount according to distance and owner density.
//! Small owners retain u16 cursors inline; larger owners allocate wider cursors on first use.
//! Clones and snapshots start without access history. This cache builds no index.

use std::{
    array,
    cell::{Cell, OnceCell, RefCell},
    fmt,
};

use ahash::AHashMap;
use monty_types::{ResourceError, ResourceTracker};
use wide::u8x16;

use crate::heap::{HeapId, StableHeap};

/// Allocation refusal disables caching; deadline errors stop the lookup.
/// Hooks must not execute Python or reenter the cache while its state is temporarily taken.
pub(crate) trait Budget {
    type Error;
    /// Preflights optional cache storage without turning refusal into a memory error.
    fn allow_allocation(&self, bytes: usize) -> bool;
    /// Polls execution limits during scans too long to wait for the next VM checkpoint.
    fn check_time(&self) -> Result<(), Self::Error>;
}

impl Budget for ResourceTracker {
    type Error = ResourceError;

    #[inline]
    fn allow_allocation(&self, bytes: usize) -> bool {
        self.check_allocation(bytes).is_ok()
    }

    #[inline]
    fn check_time(&self) -> Result<(), Self::Error> {
        Self::check_time(self)
    }
}

/// Immutable owner metadata; density decisions use the whole owner, even when walking a suffix.
#[derive(Clone, Copy)]
struct Source<'a> {
    text: &'a str,
    characters: usize,
}

/// Cursor width follows byte length, which bounds both byte offsets and code-point indices.
trait Coordinate: Copy + Default {
    fn pack(value: usize) -> Self;
    fn unpack(self) -> usize;
}

macro_rules! coordinates {
    ($($ty:ty),*) => {$(
        impl Coordinate for $ty {
            #[inline]
            fn pack(value: usize) -> Self {
                Self::try_from(value).expect("cursor width selected by owner byte length")
            }

            #[inline]
            fn unpack(self) -> usize { self as usize }
        }
    )*};
}
coordinates!(u16, u32, usize);

/// One valid UTF-8 boundary expressed in both coordinate systems.
#[derive(Clone, Copy, Default)]
struct Cursor<P> {
    character: P,
    byte: P,
}

/// A retained cursor or implicit endpoint, expanded for traversal arithmetic.
#[derive(Clone, Copy)]
struct Anchor {
    character: usize,
    byte: usize,
}

/// Two u16 cursor pairs and the enum tag fit in 16 bytes on 64-bit targets.
/// Keeping wider pairs behind a pointer avoids enlarging every heap string.
enum State {
    /// Owners below 64 KiB need no cursor allocation. Both initial cursors denote the start.
    Inline([Cursor<u16>; 2]),
    /// Owners through `u32::MAX` bytes allocate only after a nontrivial lookup.
    Wide(Box<[Cursor<u32>; 2]>),
    /// Full-width coordinates retain support for owners beyond the u32 byte range.
    Huge(Box<[Cursor<usize>; 2]>),
}

impl Default for State {
    fn default() -> Self {
        Self::Inline([Cursor::default(); 2])
    }
}

/// Retains the two most recently resolved non-endpoint boundaries, newest first.
/// Sequential reads use the newest cursor; alternating distant reads can reuse both.
/// The caller must supply the same immutable text and exact code-point count on every access.
#[derive(Default)]
pub(crate) struct AccessCache {
    // Only offsets change: shared text borrows remain valid while cursor history is updated.
    state: Cell<State>,
}

impl Clone for AccessCache {
    fn clone(&self) -> Self {
        // Access history and optional allocations belong to the original owner.
        Self::default()
    }
}

impl fmt::Debug for AccessCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessCache")
    }
}

/// The known coordinate determines how anchor distance is measured and what conversion returns.
#[derive(Clone, Copy)]
enum Target {
    /// A normalized code-point index whose byte offset is required.
    Character(usize),
    /// An in-bounds UTF-8 byte boundary whose code-point index is required.
    Byte(usize),
}

impl AccessCache {
    /// Converts from an implicit endpoint when optional interned-cache storage was refused.
    /// Uses the same deadline-aware scanners without retaining the temporary cursors.
    pub(crate) fn uncached_byte_offset<B: Budget>(
        text: &str,
        target: usize,
        characters: usize,
        budget: &B,
    ) -> Result<usize, B::Error> {
        if target == 0 {
            return Ok(0);
        }
        if target >= characters {
            return Ok(text.len());
        }
        if text.len() == characters {
            return Ok(target);
        }
        convert(
            Source { text, characters },
            Target::Character(target),
            &mut [Cursor::<usize>::default(); 2],
            budget,
        )
    }

    /// Counts from an implicit endpoint without allocating or retaining cache state.
    /// `target` must be an in-bounds UTF-8 boundary, as returned by substring search.
    pub(crate) fn uncached_char_offset<B: Budget>(
        text: &str,
        target: usize,
        characters: usize,
        budget: &B,
    ) -> Result<usize, B::Error> {
        if target == 0 {
            return Ok(0);
        }
        if target == text.len() {
            return Ok(characters);
        }
        if text.len() == characters {
            return Ok(target);
        }
        convert(
            Source { text, characters },
            Target::Byte(target),
            &mut [Cursor::<usize>::default(); 2],
            budget,
        )
    }

    /// Reads a signed Python index without allocating cache storage; out-of-range indices return `None`.
    pub(crate) fn uncached_character<B: Budget>(
        text: &str,
        index: i64,
        characters: usize,
        budget: &B,
    ) -> Result<Option<char>, B::Error> {
        let Some(target) = character_index(index, characters) else {
            return Ok(None);
        };
        let byte = Self::uncached_byte_offset(text, target, characters, budget)?;
        Ok(text[byte..].chars().next())
    }

    /// Reads a signed Python code-point index, translating negative indices from the end.
    /// Direct reads at 0/-1 and out-of-range indices do not allocate cursor state.
    pub(crate) fn character<B: Budget>(
        &self,
        text: &str,
        index: i64,
        characters: usize,
        budget: &B,
    ) -> Result<Option<char>, B::Error> {
        if index == 0 {
            return Ok(text.chars().next());
        }
        if index == -1 {
            return Ok(text.chars().next_back());
        }
        let target = character_index(index, characters);
        let Some(target) = target else {
            return Ok(None);
        };
        let byte = self.byte_offset(text, target, characters, budget)?;
        Ok(text[byte..].chars().next())
    }

    /// Converts a code-point index to a byte offset for slicing or bounded search.
    /// Indices at or beyond the character count clamp to the byte length; ASCII offsets are direct.
    pub(crate) fn byte_offset<B: Budget>(
        &self,
        text: &str,
        target: usize,
        characters: usize,
        budget: &B,
    ) -> Result<usize, B::Error> {
        if target == 0 {
            return Ok(0);
        }
        if target >= characters {
            return Ok(text.len());
        }
        if text.len() == characters {
            return Ok(target);
        }
        self.convert(Source { text, characters }, Target::Character(target), budget)
    }

    /// Converts a search result's byte offset to the code-point index Python returns.
    /// `target` must be in bounds and on a UTF-8 boundary; both endpoints bypass the cache.
    pub(crate) fn char_offset<B: Budget>(
        &self,
        text: &str,
        target: usize,
        characters: usize,
        budget: &B,
    ) -> Result<usize, B::Error> {
        debug_assert!(text.is_char_boundary(target));
        if target == 0 {
            return Ok(0);
        }
        if target == text.len() {
            return Ok(characters);
        }
        if text.len() == characters {
            return Ok(target);
        }
        self.convert(Source { text, characters }, Target::Byte(target), budget)
    }

    /// Promotes cursor width on first use, then restores the owned state even if a scan times out.
    fn convert<B: Budget>(&self, source: Source<'_>, target: Target, budget: &B) -> Result<usize, B::Error> {
        let mut state = self.state.take();
        if source.text.len() >= 65_536 && matches!(state, State::Inline(_)) {
            if u32::try_from(source.text.len()).is_ok() {
                if budget.allow_allocation(size_of::<[Cursor<u32>; 2]>()) {
                    state = State::Wide(Box::default());
                }
            } else if budget.allow_allocation(size_of::<[Cursor<usize>; 2]>()) {
                state = State::Huge(Box::default());
            }
        }
        let result = match &mut state {
            State::Inline(cursors) if source.text.len() < 65_536 => convert(source, target, cursors, budget),
            State::Wide(cursors) => convert(source, target, cursors.as_mut(), budget),
            State::Huge(cursors) => convert(source, target, cursors.as_mut(), budget),
            // Optional state must not make an otherwise valid read fail its memory limit.
            State::Inline(_) => convert(source, target, &mut [Cursor::<usize>::default(); 2], budget),
        };
        // No Python code runs while the cell is empty; deadline errors restore its state too.
        self.state.set(state);
        result
    }

    #[cfg(test)]
    pub(crate) fn has_cursor(&self) -> bool {
        let state = self.state.take();
        let populated = match &state {
            State::Inline(cursors) => cursors.iter().any(|c| c.byte != 0),
            State::Wide(_) | State::Huge(_) => true,
        };
        self.state.set(state);
        populated
    }
}

/// Normalizes Python indices without overflowing on i64::MIN or a narrower usize target.
fn character_index(index: i64, characters: usize) -> Option<usize> {
    if index < 0 {
        usize::try_from(index.unsigned_abs())
            .ok()
            .and_then(|n| characters.checked_sub(n))
    } else {
        usize::try_from(index).ok().filter(|&n| n < characters)
    }
}

/// A newly resolved target becomes most recent; exact hits refresh the order in `convert`.
#[inline]
fn remember<P: Coordinate>(cursors: &mut [Cursor<P>; 2], character: usize, byte: usize) {
    cursors[1] = cursors[0];
    cursors[0] = Cursor {
        character: P::pack(character),
        byte: P::pack(byte),
    };
}

/// Selects an anchor in the requested coordinate system, walks in either direction, and remembers the result.
/// Character targets return bytes; byte targets return code-point indices.
#[inline]
fn convert<P: Coordinate, B: Budget>(
    source: Source<'_>,
    target: Target,
    cursors: &mut [Cursor<P>; 2],
    budget: &B,
) -> Result<usize, B::Error> {
    let (position, end) = match target {
        Target::Character(n) => (n, source.characters),
        Target::Byte(n) => (n, source.text.len()),
    };
    // Endpoints cost no storage and can be closer than either cursor, especially on a cold cache.
    let mut anchor = if position <= end - position {
        Anchor { character: 0, byte: 0 }
    } else {
        Anchor {
            character: source.characters,
            byte: source.text.len(),
        }
    };
    for i in 0..2 {
        let saved = Anchor {
            character: cursors[i].character.unpack(),
            byte: cursors[i].byte.unpack(),
        };
        let (saved_position, anchor_position) = match target {
            Target::Character(_) => (saved.character, anchor.character),
            Target::Byte(_) => (saved.byte, anchor.byte),
        };
        // Refresh even a zero-walk hit so alternating positions remain the two retained anchors.
        if saved_position == position {
            if i == 1 {
                cursors.swap(0, 1);
            }
            return Ok(match target {
                Target::Character(_) => saved.byte,
                Target::Byte(_) => saved.character,
            });
        }
        // Strict comparison keeps the earlier choice on ties: endpoint, then newest cursor.
        if saved_position.abs_diff(position) < anchor_position.abs_diff(position) {
            anchor = saved;
        }
    }
    let (character, byte) = match target {
        Target::Character(n) => (n, walk(source, anchor, n, budget)?),
        Target::Byte(n) if n >= anchor.byte => {
            (anchor.character + count_range(&source.text[anchor.byte..n], budget)?, n)
        }
        Target::Byte(n) => (anchor.character - count_range(&source.text[n..anchor.byte], budget)?, n),
    };
    remember(cursors, character, byte);
    Ok(match target {
        Target::Character(_) => byte,
        Target::Byte(_) => character,
    })
}

/// Counts code points between known boundaries, polling deadlines in at most 64 KiB byte chunks.
fn count_range<B: Budget>(text: &str, budget: &B) -> Result<usize, B::Error> {
    let mut count = 0;
    // Chunk edges may split UTF-8 characters: counting leading bytes remains additive.
    for bytes in text.as_bytes().chunks(65_536) {
        if text.len() >= 65_536 {
            budget.check_time()?;
        }
        count += count_bytes(bytes);
    }
    Ok(count)
}

/// Counts leading bytes (including ASCII), rather than decoding each Unicode scalar.
#[inline]
fn count_bytes(bytes: &[u8]) -> usize {
    // bytecount enables wasm SIMD internally, even when the caller did not select simd128.
    #[cfg(all(target_arch = "wasm32", not(target_feature = "simd128")))]
    {
        bytes.iter().filter(|&&byte| byte & 0xc0 != 0x80).count()
    }

    #[cfg(not(all(target_arch = "wasm32", not(target_feature = "simd128"))))]
    {
        bytecount::num_chars(bytes)
    }
}

/// Returns an absolute byte boundary after walking from an anchor by a code-point distance.
#[inline]
fn walk<B: Budget>(source: Source<'_>, anchor: Anchor, target: usize, budget: &B) -> Result<usize, B::Error> {
    let n = anchor.character.abs_diff(target);
    if n == 0 {
        return Ok(anchor.byte);
    }
    // Sequential indexing needs only the adjacent UTF-8 boundary, avoiding scanner setup.
    if n == 1 {
        if target > anchor.character {
            let lead = source.text.as_bytes()[anchor.byte];
            return Ok(anchor.byte
                + if lead < 0x80 {
                    1
                } else if lead < 0xe0 {
                    2
                } else if lead < 0xf0 {
                    3
                } else {
                    4
                });
        }
        let bytes = source.text.as_bytes();
        let mut byte = anchor.byte - 1;
        while bytes[byte] & 0xc0 == 0x80 {
            byte -= 1;
        }
        return Ok(byte);
    }
    // Average width >= 1.5 bytes/code point selects the later bulk-scan crossover.
    let heavy = source.text.len() - source.characters >= source.characters.div_ceil(2);
    if target > anchor.character {
        Ok(anchor.byte + forward(&source.text[anchor.byte..], n, heavy, budget)?)
    } else {
        Ok(anchor.byte - backward(&source.text[..anchor.byte], n, heavy, budget)?)
    }
}

/// Returns bytes consumed from a UTF-8 suffix, choosing scalar, SIMD or bulk counting by skip distance.
/// SIMD setup is avoided below eight code points; bulk counting starts at 128, or 512 for dense text.
fn forward<B: Budget>(text: &str, n: usize, heavy: bool, budget: &B) -> Result<usize, B::Error> {
    if n < 8 {
        return Ok(chars_nth(text, n));
    }
    if n < 128 || (heavy && n < 512) {
        return Ok(wide16(text, n));
    }
    let (mut remaining, mut byte, mut checked) = (n, 0, 0);
    if n >= 65_536 {
        budget.check_time()?;
    }
    // Scanning at most `remaining` bytes cannot consume too many code points: each needs >= one byte.
    // Align each chunk's end so the next iteration still starts at a valid UTF-8 boundary.
    while remaining >= 16 && byte < text.len() {
        let mut end = byte + remaining.min(text.len() - byte).min(65_536);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == byte {
            break;
        }
        remaining -= count_bytes(&text.as_bytes()[byte..end]);
        byte = end;
        // Boundary rounding can trim three bytes from a full 64 KiB chunk.
        if byte - checked >= 65_532 {
            budget.check_time()?;
            checked = byte;
        }
    }
    // Finish the small remainder without another bulk-count call.
    Ok(byte + wide16(&text[byte..], remaining))
}

/// Returns bytes removed from a UTF-8 prefix, using the same skip thresholds as forward traversal.
/// The caller subtracts this distance from its anchor's absolute byte offset.
fn backward<B: Budget>(text: &str, n: usize, heavy: bool, budget: &B) -> Result<usize, B::Error> {
    if n < 8 {
        return Ok(chars_nth_back(text, n));
    }
    if n < 128 || (heavy && n < 512) {
        return Ok(wide16_back(text, n));
    }
    let (mut remaining, mut end, mut checked) = (n, text.len(), text.len());
    if n >= 65_536 {
        budget.check_time()?;
    }
    // Move the lower edge forward to a boundary; the retained prefix then remains valid UTF-8.
    while remaining >= 16 && end > 0 {
        let mut start = end - remaining.min(end).min(65_536);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        if start == end {
            break;
        }
        remaining -= count_bytes(&text.as_bytes()[start..end]);
        end = start;
        if checked - end >= 65_532 {
            budget.check_time()?;
            checked = end;
        }
    }
    Ok(text.len() - end + wide16_back(&text[..end], remaining))
}

/// Consumes preceding code points while leaving the requested boundary at the remaining suffix.
fn chars_nth(text: &str, target: usize) -> usize {
    if target == 0 {
        return 0;
    }
    let mut chars = text.chars();
    chars.nth(target - 1);
    text.len() - chars.as_str().len()
}

/// Finds a forward boundary by ranking leading bytes in 16-byte blocks.
fn wide16(text: &str, mut remaining: usize) -> usize {
    let bytes = text.as_bytes();
    let mut offset = 0;
    while bytes.len() - offset >= 16 {
        let lanes = u8x16::from(<[u8; 16]>::try_from(&bytes[offset..offset + 16]).unwrap());
        // Continuation bytes have prefix 10xxxxxx; every other byte starts a code point.
        let leaders = (lanes & u8x16::splat(0xc0)).simd_ne(u8x16::splat(0x80));
        let mut mask = leaders.to_bitmask();
        let count = mask.count_ones() as usize;
        if remaining < count {
            // When every byte is a leading byte, character and byte ranks coincide.
            if count == 16 {
                return offset + remaining;
            }
            // Clearing the lower ranks leaves the requested leading byte as the lowest set bit.
            for _ in 0..remaining {
                mask &= mask - 1;
            }
            return offset + mask.trailing_zeros() as usize;
        }
        remaining -= count;
        offset += 16;
    }
    for (relative, &value) in bytes[offset..].iter().enumerate() {
        if value & 0xc0 != 0x80 {
            if remaining == 0 {
                return offset + relative;
            }
            remaining -= 1;
        }
    }
    bytes.len()
}

/// Consumes code points from the end and returns the removed byte count.
fn chars_nth_back(text: &str, target: usize) -> usize {
    if target == 0 {
        return 0;
    }
    let mut chars = text.chars();
    chars.nth_back(target - 1);
    text.len() - chars.as_str().len()
}

/// Finds a reverse boundary by selecting leading bytes from the high end of each 16-byte block.
fn wide16_back(text: &str, mut remaining: usize) -> usize {
    if remaining == 0 {
        return 0;
    }
    let bytes = text.as_bytes();
    let mut end = bytes.len();
    while end >= 16 {
        let start = end - 16;
        let lanes = u8x16::from(<[u8; 16]>::try_from(&bytes[start..end]).unwrap());
        let mut mask = (lanes & u8x16::splat(0xc0)).simd_ne(u8x16::splat(0x80)).to_bitmask();
        let count = mask.count_ones() as usize;
        // Reverse walking consumes the selected character; equality lands within this block.
        if remaining <= count {
            if count == 16 {
                return bytes.len() - end + remaining;
            }
            // Discard later leading bytes until the requested reverse rank is the highest bit.
            for _ in 1..remaining {
                mask ^= 1 << mask.ilog2();
            }
            let byte = start + mask.ilog2() as usize;
            return bytes.len() - byte;
        }
        remaining -= count;
        end = start;
    }
    // A SIMD block may split a character; scan bytes until its leading byte is reached.
    for byte in (0..end).rev() {
        if bytes[byte] & 0xc0 != 0x80 {
            remaining -= 1;
            if remaining == 0 {
                return bytes.len() - byte;
            }
        }
    }
    bytes.len()
}

const PAGE_SIZE: usize = 256;

/// Lazily populated cache slots for one contiguous range of intern IDs.
/// Boxing individual caches avoids initializing their cursor state for untouched slots.
type AccessCachePage = [OnceCell<Box<AccessCache>>; PAGE_SIZE];

/// Lazy caches keyed by intern slot; ASCII-only sessions allocate no cache pages.
/// Cache addresses remain stable while runtime compilation adds strings; cloning creates an empty store.
#[derive(Debug, Default)]
pub(crate) struct AccessCacheStore(OnceCell<Box<Directory>>);

/// Sparse directory entries avoid reserving the gap before a late Unicode intern slot.
#[derive(Debug)]
struct Directory {
    /// Logical page number to its stable arena slot; absent pages allocate nothing.
    slots: RefCell<AHashMap<usize, HeapId>>,
    /// Append-only pages: no arena slot is freed while a cache may be borrowed.
    pages: StableHeap<Box<AccessCachePage>>,
    /// Last logical page and arena slot, avoiding a hash lookup for nearby intern IDs.
    last: Cell<Option<(usize, HeapId)>>,
}

impl Default for Directory {
    fn default() -> Self {
        Self {
            slots: RefCell::default(),
            pages: StableHeap::with_capacity(0),
            last: Cell::new(None),
        }
    }
}

impl Clone for AccessCacheStore {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl AccessCacheStore {
    #[cfg(test)]
    pub(crate) fn is_initialized(&self) -> bool {
        self.0.get().is_some()
    }

    /// Returns the cache for an intern slot, allocating its page and cache shell on first access.
    /// The caller filters ASCII and reserved strings; returned borrows survive later insertions.
    /// Refused allocations return `None` so the caller can scan directly without retaining state.
    pub(crate) fn get<B: Budget>(&self, index: usize, budget: &B) -> Option<&AccessCache> {
        if self.0.get().is_none() && !budget.allow_allocation(size_of::<Directory>()) {
            return None;
        }
        let table = self.0.get_or_init(Box::default);
        let page = index / PAGE_SIZE;
        // Most accesses reuse a logical page; avoid borrowing and probing the directory in that case.
        let slot = if let Some((last_page, slot)) = table.last.get()
            && last_page == page
        {
            slot
        } else {
            let mut slots = table.slots.borrow_mut();
            if !slots.contains_key(&page) {
                // Preflight the cache page, possible stable-arena page, and directory/pointer growth.
                // Conservative headroom can skip a cache early; lookup correctness never depends on it.
                let directory = slots
                    .capacity()
                    .saturating_add(1)
                    .saturating_mul(8 * size_of::<(usize, HeapId)>());
                let page_pointers = (table.pages.len() / PAGE_SIZE + 1).saturating_mul(4 * size_of::<usize>());
                let bytes = size_of::<AccessCachePage>()
                    .saturating_mul(2)
                    .saturating_add(directory)
                    .saturating_add(page_pointers)
                    .saturating_add(128);
                if !budget.allow_allocation(bytes) {
                    return None;
                }
            }
            let slot = *slots
                .entry(page)
                .or_insert_with(|| table.pages.allocate(Box::new(array::from_fn(|_| OnceCell::new()))));
            table.last.set(Some((page, slot)));
            slot
        };
        // Cache borrows come from stable pages, not the directory's temporary RefCell borrow.
        let entry = &table.pages.get(slot)[index % PAGE_SIZE];
        if entry.get().is_none() && !budget.allow_allocation(size_of::<AccessCache>()) {
            return None;
        }
        Some(entry.get_or_init(Box::default))
    }
}

#[cfg(test)]
mod store_tests {
    use std::ptr;

    use super::{test_budget::Unlimited, *};

    #[test]
    fn late_unicode_cache_allocates_only_its_page() {
        let store = AccessCacheStore::default();
        let cache = store.get(1_000_000, &Unlimited).unwrap();
        assert_eq!(cache.character("é中", 1, 2, &Unlimited), Ok(Some('中')));
        let table = store.0.get().unwrap();
        assert_eq!(table.pages.len(), 1);
        assert_eq!(table.slots.borrow().len(), 1);
        assert!(!ptr::eq(cache, store.get(1_000_001, &Unlimited).unwrap()));
        assert_eq!(table.pages.len(), 1);
    }

    #[test]
    fn cache_borrows_survive_growth_and_clone_starts_empty() {
        let store = AccessCacheStore::default();
        let original = store.get(3, &Unlimited).unwrap();
        original.byte_offset("aé中🐍", 2, 4, &Unlimited).unwrap();
        for i in 0..512 {
            store.get(i * PAGE_SIZE + 1024, &Unlimited).unwrap();
        }
        assert!(ptr::eq(original, store.get(3, &Unlimited).unwrap()));
        assert!(original.has_cursor());
        assert!(!store.clone().is_initialized());
    }

    struct NoMemory;
    impl Budget for NoMemory {
        type Error = ();
        fn allow_allocation(&self, _: usize) -> bool {
            false
        }
        fn check_time(&self) -> Result<(), ()> {
            Ok(())
        }
    }

    #[test]
    fn refused_directory_or_cache_allocation_returns_none() {
        let store = AccessCacheStore::default();
        assert!(store.get(0, &NoMemory).is_none());
        assert!(!store.is_initialized());
        store.get(0, &Unlimited).unwrap();
        assert!(store.get(1, &NoMemory).is_none());
        assert!(store.get(PAGE_SIZE, &NoMemory).is_none());
        assert!(store.get(0, &NoMemory).is_some());
    }
}

#[cfg(test)]
pub(crate) mod test_budget {
    use std::convert::Infallible;

    use super::Budget;

    pub(crate) struct Unlimited;
    impl Budget for Unlimited {
        type Error = Infallible;
        fn allow_allocation(&self, _: usize) -> bool {
            true
        }
        fn check_time(&self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{test_budget::Unlimited, *};
    use crate::{
        heap::{HeapData, HeapEntry},
        types::str::HeapStr,
    };

    fn verify(text: &str) {
        let cache = AccessCache::default();
        let boundaries: Vec<_> = text.char_indices().map(|(b, _)| b).chain([text.len()]).collect();
        let characters = boundaries.len() - 1;
        if characters == 0 {
            return;
        }
        let mut rng = 42u64;
        for _ in 0..4096 {
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let n = (rng >> 32) as usize % (characters + 1);
            assert_eq!(cache.byte_offset(text, n, characters, &Unlimited), Ok(boundaries[n]));
            assert_eq!(cache.char_offset(text, boundaries[n], characters, &Unlimited), Ok(n));
            assert_eq!(
                cache.character(text, i64::try_from(n).unwrap(), characters, &Unlimited),
                Ok(text.chars().nth(n))
            );
        }
        for (n, &byte) in boundaries.iter().enumerate().rev() {
            assert_eq!(cache.byte_offset(text, n, characters, &Unlimited), Ok(byte));
        }
    }

    #[test]
    fn random_backward_and_inverse_offsets() {
        for text in [
            String::new(),
            "ascii".repeat(100),
            "aé中🐍\0\u{301}\u{10ffff}".repeat(100),
            "a".repeat(1000) + "🐍" + &"z".repeat(1000),
        ] {
            verify(&text);
        }
        for bytes in [255, 256, 65_535, 65_536, 65_537, 131_072] {
            let mut text = "aé中🐍".repeat(bytes / 10);
            text.push_str(&"z".repeat(bytes - text.len()));
            verify(&text);
        }
    }

    #[test]
    fn endpoints_and_out_of_range_reads_keep_empty_state() {
        let cache = AccessCache::default();
        for text in ["", "aé中🐍"] {
            let chars = text.chars().count();
            assert_eq!(cache.character(text, 0, chars, &Unlimited), Ok(text.chars().next()));
            assert_eq!(
                cache.character(text, -1, chars, &Unlimited),
                Ok(text.chars().next_back())
            );
            assert_eq!(cache.character(text, i64::MIN, chars, &Unlimited), Ok(None));
            assert_eq!(cache.character(text, i64::MAX, chars, &Unlimited), Ok(None));
            assert_eq!(cache.byte_offset(text, usize::MAX, chars, &Unlimited), Ok(text.len()));
        }
        assert!(!cache.has_cursor());
    }

    #[test]
    fn exact_hits_refresh_both_cursors_and_clone_resets() {
        let text = "aé中🐍".repeat(100);
        let cache = AccessCache::default();
        for n in [100, 300, 100] {
            cache.byte_offset(&text, n, 400, &Unlimited).unwrap();
        }
        let state = cache.state.take();
        let State::Inline(cursors) = &state else {
            panic!("small cursors must remain inline");
        };
        assert_eq!(cursors[0].character, 100);
        assert_eq!(cursors[1].character, 300);
        cache.state.set(state);
        assert!(cache.has_cursor());
        let cloned = cache.clone();
        assert!(!cloned.has_cursor());
        assert_eq!(cloned.byte_offset(&text, 200, 400, &Unlimited), Ok(500));
        assert_eq!(cache.character(&text, -2, 400, &Unlimited), Ok(Some('中')));
    }

    struct NoMemory;
    impl Budget for NoMemory {
        type Error = ();
        fn allow_allocation(&self, _: usize) -> bool {
            false
        }
        fn check_time(&self) -> Result<(), ()> {
            Ok(())
        }
    }

    #[test]
    fn denied_wide_state_falls_back_to_scanning() {
        let text = "aé中🐍".repeat(20_000);
        let cache = AccessCache::default();
        for n in [1, 17, 30_000, 70_000, 4, 79_999] {
            let byte = text.char_indices().nth(n).unwrap().0;
            assert_eq!(cache.byte_offset(&text, n, 80_000, &NoMemory), Ok(byte));
            assert_eq!(cache.char_offset(&text, byte, 80_000, &NoMemory), Ok(n));
        }
        assert!(!cache.has_cursor());
    }

    struct Expired;
    impl Budget for Expired {
        type Error = ();
        fn allow_allocation(&self, _: usize) -> bool {
            true
        }
        fn check_time(&self) -> Result<(), ()> {
            Err(())
        }
    }

    #[test]
    fn interrupted_walk_restores_retained_cursors() {
        let text = "aé中🐍".repeat(100_000);
        let cache = AccessCache::default();
        let saved = cache.byte_offset(&text, 100, 400_000, &Unlimited).unwrap();
        assert_eq!(cache.byte_offset(&text, 200_000, 400_000, &Expired), Err(()));
        assert_eq!(cache.byte_offset(&text, 100, 400_000, &Expired), Ok(saved));
        assert_eq!(cache.char_offset(&text, 500_000, 400_000, &Expired), Err(()));
        assert_eq!(cache.byte_offset(&text, 200_000, 400_000, &Unlimited), Ok(500_000));
    }

    #[test]
    fn compact_cursors_keep_arena_slots_unchanged() {
        if cfg!(target_pointer_width = "64") {
            assert_eq!(size_of::<AccessCache>(), 16);
            assert_eq!(size_of::<HeapStr>(), 48);
            assert_eq!(size_of::<HeapData>(), 72);
            assert_eq!(size_of::<HeapEntry>(), 96);
            assert_eq!(size_of::<Option<HeapEntry>>(), 104);
        }
    }
}
