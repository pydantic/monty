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
        let index = StringIndex::default();
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
            let index = StringIndex::default();
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
    for text in ["", "ascii", "aé中🐍", "abcdefghüijk中🐍"].map(|s| s.repeat(300)) {
        let index = StringIndex::default();
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
    let index = StringIndex::default();
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
    let index = StringIndex::default();
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
    let index = StringIndex::default();
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
    assert_eq!(cloned.length.get(), 0);
    assert!(cloned.state.borrow().is_none());
}
