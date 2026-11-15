//! Stateless conversion between Unicode code-point indices and UTF-8 byte offsets.
//! The caller supplies the character count and an optional known boundary; no cursor state is retained.
//! Short skips use scalar iteration, medium skips use 16-byte SIMD, and longer skips use bytecount.
//! Long scans poll a caller-provided deadline callback, without depending on interpreter resource types.

use wide::u8x16;

/// A known UTF-8 boundary; both coordinates must refer to the same boundary in the owner.
#[derive(Clone, Copy)]
pub(super) struct Anchor {
    pub(super) character: usize,
    pub(super) byte: usize,
}

/// Counts code points between known boundaries, polling deadlines in at most 64 KiB byte chunks.
pub(super) fn count_chars<E>(text: &str, check_time: impl Fn() -> Result<(), E>) -> Result<usize, E> {
    let mut count = 0;
    // Chunk edges may split UTF-8 characters: counting leading bytes remains additive.
    for bytes in text.as_bytes().chunks(65_536) {
        if text.len() >= 65_536 {
            check_time()?;
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

/// Converts a code-point index to an absolute byte offset.
/// `characters` must be the exact code-point count of `text`, used to select scanning thresholds.
/// `None` selects the nearer string endpoint; `Some` starts at the supplied boundary.
/// The supplied anchor and target must belong to `text`; `target` may include the end boundary.
/// Long scans call `check_time` between bounded chunks and propagate its error unchanged.
#[inline]
pub(super) fn byte_offset<E>(
    text: &str,
    characters: usize,
    anchor: Option<Anchor>,
    target: usize,
    check_time: impl Fn() -> Result<(), E>,
) -> Result<usize, E> {
    let anchor = anchor.unwrap_or_else(|| {
        if target <= characters - target {
            Anchor { character: 0, byte: 0 }
        } else {
            Anchor {
                character: characters,
                byte: text.len(),
            }
        }
    });
    let n = anchor.character.abs_diff(target);
    if n == 0 {
        return Ok(anchor.byte);
    }
    // Sequential indexing needs only the adjacent UTF-8 boundary, avoiding scanner setup.
    if n == 1 {
        if target > anchor.character {
            let lead = text.as_bytes()[anchor.byte];
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
        let bytes = text.as_bytes();
        let mut byte = anchor.byte - 1;
        while bytes[byte] & 0xc0 == 0x80 {
            byte -= 1;
        }
        return Ok(byte);
    }
    // Average width >= 1.5 bytes/code point selects the later bulk-scan crossover.
    let heavy = text.len() - characters >= characters.div_ceil(2);
    if target > anchor.character {
        Ok(anchor.byte + forward(&text[anchor.byte..], n, heavy, &check_time)?)
    } else {
        Ok(anchor.byte - backward(&text[..anchor.byte], n, heavy, &check_time)?)
    }
}

/// Returns bytes consumed from a UTF-8 suffix, choosing scalar, SIMD or bulk counting by skip distance.
/// SIMD setup is avoided below eight code points; bulk counting starts at 128, or 512 for dense text.
fn forward<E>(text: &str, n: usize, heavy: bool, check_time: &impl Fn() -> Result<(), E>) -> Result<usize, E> {
    if n < 8 {
        return Ok(chars_nth(text, n));
    }
    if n < 128 || (heavy && n < 512) {
        return Ok(wide16(text, n));
    }
    let (mut remaining, mut byte, mut checked) = (n, 0, 0);
    if n >= 65_536 {
        check_time()?;
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
            check_time()?;
            checked = byte;
        }
    }
    // Finish the small remainder without another bulk-count call.
    Ok(byte + wide16(&text[byte..], remaining))
}

/// Returns bytes removed from a UTF-8 prefix, using the same skip thresholds as forward traversal.
/// The caller subtracts this distance from its anchor's absolute byte offset.
fn backward<E>(text: &str, n: usize, heavy: bool, check_time: &impl Fn() -> Result<(), E>) -> Result<usize, E> {
    if n < 8 {
        return Ok(chars_nth_back(text, n));
    }
    if n < 128 || (heavy && n < 512) {
        return Ok(wide16_back(text, n));
    }
    let (mut remaining, mut end, mut checked) = (n, text.len(), text.len());
    if n >= 65_536 {
        check_time()?;
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
            check_time()?;
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

#[cfg(test)]
mod tests {
    use std::{cell::Cell, convert::Infallible};

    use super::{Anchor, byte_offset, count_chars};

    fn boundaries(text: &str) -> Vec<usize> {
        text.char_indices().map(|(byte, _)| byte).chain([text.len()]).collect()
    }

    #[test]
    fn every_small_boundary_in_both_directions() {
        for text in ["", "ascii\0text", "aé中🐍e\u{301}\0\u{10ffff}"] {
            let offsets = boundaries(text);
            let characters = offsets.len() - 1;

            for (character, &byte) in offsets.iter().enumerate() {
                let anchor = Anchor { character, byte };
                for (target, &expected) in offsets.iter().enumerate() {
                    assert_eq!(
                        byte_offset(text, characters, Some(anchor), target, || Ok::<(), Infallible>(())),
                        Ok(expected)
                    );
                    assert_eq!(
                        byte_offset(text, characters, None, target, || Ok::<(), Infallible>(())),
                        Ok(expected)
                    );
                }
            }
        }
    }

    #[test]
    fn scanner_crossovers_in_both_directions() {
        let mostly_ascii = format!("{}é", "a".repeat(99)).repeat(100);
        let dense = "aé中🐍".repeat(2500);
        let ascii = "a".repeat(10_000);
        let skips = [
            0, 1, 2, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 129, 255, 256, 511, 512, 513, 4096,
        ];

        for text in [&ascii, &mostly_ascii, &dense] {
            let offsets = boundaries(text);
            let characters = offsets.len() - 1;

            for character in [0, 3, characters / 2, characters - 3, characters] {
                let anchor = Anchor {
                    character,
                    byte: offsets[character],
                };

                for skip in skips {
                    for target in [character.checked_add(skip), character.checked_sub(skip)]
                        .into_iter()
                        .flatten()
                        .filter(|&target| target <= characters)
                    {
                        assert_eq!(
                            byte_offset(text, characters, Some(anchor), target, || Ok::<(), Infallible>(())),
                            Ok(offsets[target]),
                            "anchor={character}, target={target}, bytes={}",
                            text.len()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn implicit_endpoints_avoid_distant_scans() {
        let text = "é中🐍".repeat(40_000);
        let offsets = boundaries(&text);
        let characters = offsets.len() - 1;

        // Near either end, the default anchor must avoid a long scan from the opposite end.
        for target in [0, 1, 2, characters - 2, characters - 1, characters] {
            assert_eq!(
                byte_offset(&text, characters, None, target, || Err("deadline")),
                Ok(offsets[target])
            );
        }
        for target in [characters / 2, characters / 2 + 1] {
            assert_eq!(
                byte_offset(&text, characters, None, target, || Err("deadline")),
                Err("deadline")
            );
        }
        assert_eq!(byte_offset("", 0, None, 0, || Err("deadline")), Ok(0));
    }

    #[test]
    fn counting_chunks_can_split_characters() {
        let text = "é中🐍".repeat(30_000);
        let checks = Cell::new(0);
        let count = count_chars(&text, || {
            checks.set(checks.get() + 1);
            Ok::<(), Infallible>(())
        });

        assert_eq!(count, Ok(90_000));
        assert_eq!(checks.get(), text.len().div_ceil(65_536));
        assert!(!text.is_char_boundary(65_536));
    }

    #[test]
    fn deadlines_only_interrupt_long_scans() {
        let text = "é中🐍".repeat(40_000);
        let characters = 120_000;
        let start = Anchor { character: 0, byte: 0 };
        let end = Anchor {
            character: characters,
            byte: text.len(),
        };

        assert_eq!(
            byte_offset(&text, characters, Some(start), characters, || Err("deadline")),
            Err("deadline")
        );
        assert_eq!(
            byte_offset(&text, characters, Some(end), 0, || Err("deadline")),
            Err("deadline")
        );
        assert_eq!(count_chars(&text, || Err("deadline")), Err("deadline"));

        // These skips cross a byte chunk before reaching the initial character-count check.
        assert_eq!(
            byte_offset(&text, characters, Some(start), 30_000, || Err("deadline")),
            Err("deadline")
        );
        assert_eq!(
            byte_offset(&text, characters, Some(end), end.character - 30_000, || Err("deadline")),
            Err("deadline")
        );

        // Small accesses remain usable without polling a deadline callback.
        assert_eq!(
            byte_offset(&text, characters, Some(start), 1, || Err("deadline")),
            Ok(2)
        );
        assert_eq!(
            byte_offset(&text, characters, Some(end), end.character - 1, || Err("deadline")),
            Ok(text.len() - 4)
        );
        assert_eq!(count_chars("é中🐍", || Err("deadline")), Ok(3));
    }
}
