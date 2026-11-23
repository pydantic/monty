//! Convert between Python code-point indices and UTF-8 byte offsets using two recent boundaries.
//! Start and end are implicit anchors; each lookup chooses the closest anchor in its input coordinates.
//! Skips use scalar iteration, 16-byte SIMD or bulk bytecount according to distance and owner density.
//! Strings below 64 KiB store u16 cursors inline. Larger strings can allocate usize cursors on first use.
//! If the memory budget refuses the allocation, lookups scan without caching.
//! Clones and snapshots start without access history. This cache builds no index.

use std::{cell::Cell, fmt};

use monty_types::{ResourceError, ResourceTracker};

use super::fast_offsets::{self, Anchor};

/// Allocation refusal disables caching; deadline errors stop the lookup.
/// Hooks must not execute Python or reenter the cache while its state is temporarily taken.
pub(super) trait Budget {
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

/// One valid UTF-8 boundary expressed in both coordinate systems.
#[derive(Clone, Copy, Default)]
struct Cursor<P> {
    character: P,
    byte: P,
}

/// Two u16 cursor pairs and the enum tag fit in 16 bytes on 64-bit targets.
/// Keeping wider pairs behind a pointer avoids enlarging every heap string.
enum State {
    /// Owners below 64 KiB need no cursor allocation. Both initial cursors denote the start.
    Inline([Cursor<u16>; 2]),
    /// Full-width offsets cover every larger string with one allocation.
    Wide(Box<[Cursor<usize>; 2]>),
}

impl Default for State {
    fn default() -> Self {
        Self::Inline([Cursor::default(); 2])
    }
}

/// Retains the two most recently resolved non-endpoint boundaries, newest first.
/// Sequential reads use the newest cursor; alternating distant reads can reuse both.
/// Each cache belongs to a single immutable string.
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
    Character { index: usize, characters: usize },
    /// A byte boundary. The end anchor is available only if the owner already knows its length in code points.
    Byte { offset: usize, characters: Option<usize> },
}

impl Target {
    fn position(self) -> usize {
        match self {
            Self::Character { index, .. } => index,
            Self::Byte { offset, .. } => offset,
        }
    }

    fn coordinate(self, anchor: Anchor) -> usize {
        match self {
            Self::Character { .. } => anchor.character,
            Self::Byte { .. } => anchor.byte,
        }
    }

    fn characters(self) -> Option<usize> {
        match self {
            Self::Character { characters, .. } => Some(characters),
            Self::Byte { characters, .. } => characters,
        }
    }
}

impl AccessCache {
    /// Converts a code-point index to a byte offset for slicing or bounded search.
    /// Indices at or beyond the character count clamp to the byte length; ASCII offsets are direct.
    pub(super) fn byte_offset<B: Budget>(
        &self,
        text: &str,
        target: usize,
        characters: usize,
        hint: Option<Anchor>,
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
        self.convert(
            text,
            Target::Character {
                index: target,
                characters,
            },
            hint,
            budget,
        )
    }

    /// Converts a search result's byte offset to the code-point index Python returns.
    /// `target` must be in bounds and on a UTF-8 boundary; a known end needs no scan.
    pub(super) fn char_offset<B: Budget>(
        &self,
        text: &str,
        target: usize,
        characters: Option<usize>,
        hint: Option<Anchor>,
        budget: &B,
    ) -> Result<usize, B::Error> {
        debug_assert!(text.is_char_boundary(target));
        if target == 0 {
            return Ok(0);
        }
        if target == text.len()
            && let Some(characters) = characters
        {
            return Ok(characters);
        }
        if Some(text.len()) == characters {
            return Ok(target);
        }
        self.convert(
            text,
            Target::Byte {
                offset: target,
                characters,
            },
            hint,
            budget,
        )
    }

    /// Promotes cursor width on first use, then restores the owned state even if a scan times out.
    fn convert<B: Budget>(
        &self,
        text: &str,
        target: Target,
        hint: Option<Anchor>,
        budget: &B,
    ) -> Result<usize, B::Error> {
        let mut state = self.state.take();
        if text.len() >= 65_536
            && matches!(state, State::Inline(_))
            && budget.allow_allocation(size_of::<[Cursor<usize>; 2]>())
        {
            state = State::Wide(Box::default());
        }
        let result = match &mut state {
            State::Inline(cursors) if text.len() < 65_536 => convert(text, target, hint, cursors, budget),
            State::Wide(cursors) => convert(text, target, hint, cursors.as_mut(), budget),
            // Optional state must not make an otherwise valid read fail its memory limit.
            State::Inline(_) => convert(text, target, hint, &mut [Cursor::<usize>::default(); 2], budget),
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
            State::Wide(_) => true,
        };
        self.state.set(state);
        populated
    }
}

/// Selects an anchor in the requested coordinate system, walks in either direction, and remembers the result.
/// Character targets return bytes; byte targets return code-point indices.
#[inline]
fn convert<P, B: Budget>(
    text: &str,
    target: Target,
    hint: Option<Anchor>,
    cursors: &mut [Cursor<P>; 2],
    budget: &B,
) -> Result<usize, B::Error>
where
    P: Copy + Into<usize> + TryFrom<usize>,
    P::Error: fmt::Debug,
{
    let position = target.position();
    let mut anchor = Anchor { character: 0, byte: 0 };
    if let Some(characters) = target.characters() {
        let end = Anchor {
            character: characters,
            byte: text.len(),
        };
        if target.coordinate(end) - position < position {
            anchor = end;
        }
    }
    if let Some(hint) = hint
        && target.coordinate(hint).abs_diff(position) < target.coordinate(anchor).abs_diff(position)
    {
        anchor = hint;
    }
    for i in 0..2 {
        let saved = Anchor {
            character: cursors[i].character.into(),
            byte: cursors[i].byte.into(),
        };
        let saved_position = target.coordinate(saved);
        // Refresh even a zero-walk hit so alternating positions remain the two retained anchors.
        if saved_position == position {
            if i == 1 {
                cursors.swap(0, 1);
            }
            return Ok(match target {
                Target::Character { .. } => saved.byte,
                Target::Byte { .. } => saved.character,
            });
        }
        // Strict comparison keeps the earlier choice on ties: endpoint, then newest cursor.
        if saved_position.abs_diff(position) < target.coordinate(anchor).abs_diff(position) {
            anchor = saved;
        }
    }
    let (character, byte) = match target {
        Target::Character { index: n, characters } => (
            n,
            fast_offsets::byte_offset(text, characters, Some(anchor), n, || budget.check_time())?,
        ),
        Target::Byte { offset: n, .. } if n >= anchor.byte => (
            anchor.character + fast_offsets::count_chars(&text[anchor.byte..n], || budget.check_time())?,
            n,
        ),
        Target::Byte { offset: n, .. } => (
            anchor.character - fast_offsets::count_chars(&text[n..anchor.byte], || budget.check_time())?,
            n,
        ),
    };
    cursors[1] = cursors[0];
    cursors[0] = Cursor {
        character: P::try_from(character).expect("cursor width covers the string"),
        byte: P::try_from(byte).expect("cursor width covers the string"),
    };
    Ok(match target {
        Target::Character { .. } => byte,
        Target::Byte { .. } => character,
    })
}

#[cfg(test)]
pub(crate) mod test_budget {
    use std::convert::Infallible;

    use super::Budget;

    pub(super) struct AllocationBudget(pub(super) usize);
    impl Budget for AllocationBudget {
        type Error = ();
        fn allow_allocation(&self, bytes: usize) -> bool {
            bytes <= self.0
        }
        fn check_time(&self) -> Result<(), ()> {
            Ok(())
        }
    }

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
    use super::{
        test_budget::{AllocationBudget, Unlimited},
        *,
    };
    #[cfg(target_pointer_width = "64")]
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
            assert_eq!(
                cache.byte_offset(text, n, characters, None, &Unlimited),
                Ok(boundaries[n])
            );
            assert_eq!(
                cache.char_offset(text, boundaries[n], Some(characters), None, &Unlimited),
                Ok(n)
            );
        }
        for (n, &byte) in boundaries.iter().enumerate().rev() {
            assert_eq!(cache.byte_offset(text, n, characters, None, &Unlimited), Ok(byte));
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
            assert_eq!(cache.byte_offset(text, 0, chars, None, &Unlimited), Ok(0));
            assert_eq!(
                cache.char_offset(text, text.len(), Some(chars), None, &Unlimited),
                Ok(chars)
            );
            assert_eq!(
                cache.byte_offset(text, usize::MAX, chars, None, &Unlimited),
                Ok(text.len())
            );
        }
        assert!(!cache.has_cursor());
    }

    #[test]
    fn exact_hits_refresh_both_cursors_and_clone_resets() {
        let text = "aé中🐍".repeat(100);
        let cache = AccessCache::default();
        for n in [100, 300, 100] {
            cache.byte_offset(&text, n, 400, None, &Unlimited).unwrap();
        }
        let state = cache.state.take();
        assert!(matches!(
            &state,
            State::Inline([Cursor { character: 100, .. }, Cursor { character: 300, .. }])
        ));
        cache.state.set(state);
        assert_eq!(format!("{cache:?}"), "AccessCache");
        let cloned = cache.clone();
        assert!(cache.has_cursor());
        assert!(!cloned.has_cursor());
        assert_eq!(cloned.byte_offset(&text, 200, 400, None, &Unlimited), Ok(500));
    }

    #[test]
    fn refused_wide_allocation_scans_and_can_be_retried() {
        let text = "aé中🐍".repeat(20_000);
        let cache = AccessCache::default();
        let cursor_bytes = 4 * size_of::<usize>();
        let too_small = AllocationBudget(cursor_bytes - 1);
        for n in [1, 17, 30_000, 70_000, 4, 79_999] {
            let byte = text.char_indices().nth(n).unwrap().0;
            assert_eq!(cache.byte_offset(&text, n, 80_000, None, &too_small), Ok(byte));
            assert_eq!(cache.char_offset(&text, byte, Some(80_000), None, &too_small), Ok(n));
        }
        assert!(!cache.has_cursor());
        assert_eq!(
            cache.byte_offset(&text, 70_000, 80_000, None, &AllocationBudget(cursor_bytes)),
            Ok(175_000)
        );
        assert!(cache.has_cursor());
        assert_eq!(
            cache.char_offset(&text, 175_000, Some(80_000), None, &AllocationBudget(0)),
            Ok(70_000)
        );
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
        assert_eq!(cache.byte_offset(&text, 200_000, 400_000, None, &Expired), Err(()));
        let saved = cache.byte_offset(&text, 100, 400_000, None, &Unlimited).unwrap();
        assert_eq!(cache.byte_offset(&text, 200_000, 400_000, None, &Expired), Err(()));
        assert_eq!(cache.byte_offset(&text, 100, 400_000, None, &Expired), Ok(saved));
        assert_eq!(
            cache.char_offset(&text, 500_000, Some(400_000), None, &Expired),
            Err(())
        );
        assert_eq!(
            cache.byte_offset(&text, 200_000, 400_000, None, &Unlimited),
            Ok(500_000)
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn compact_cursors_keep_arena_slots_unchanged() {
        assert_eq!(size_of::<AccessCache>(), 16);
        assert_eq!(size_of::<HeapStr>(), 48);
        assert_eq!(size_of::<HeapData>(), 72);
        assert_eq!(size_of::<HeapEntry>(), 96);
        assert_eq!(size_of::<Option<HeapEntry>>(), 104);
    }
}
