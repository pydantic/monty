//! Borrowed ASCII runs and individual non-ASCII characters, with source byte offsets.
//!
//! Mostly-ASCII agent text benefits from bulk ASCII case conversion between Unicode
//! characters. Runs are maximal to avoid repeated iterator dispatch inside long ASCII spans.
//! Retaining `Chars` also avoids restarting its decoder for each character
//! in Chinese text. Source offsets preserve context-sensitive mappings such as Greek sigma.

use std::str::Chars;

/// An ASCII slice or one decoded non-ASCII code point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsciiOrChar<'a> {
    Ascii(&'a str),
    Char(char),
}

/// Splits text into maximal ASCII runs and individual non-ASCII characters without allocating.
#[derive(Debug, Clone)]
pub(crate) struct AsciiRuns<'a> {
    chars: Chars<'a>,
    original_len: usize,
}

impl<'a> AsciiRuns<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            chars: text.chars(),
            original_len: text.len(),
        }
    }
}

impl<'a> Iterator for AsciiRuns<'a> {
    type Item = (usize, AsciiOrChar<'a>);

    // Keep decoding inside each case-conversion loop when multiple methods share this iterator.
    #[expect(
        clippy::inline_always,
        reason = "decoding must stay in case-conversion loops to avoid measured dense-Unicode regressions"
    )]
    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        let text = self.chars.as_str();
        let character = self.chars.next()?;
        let offset = self.original_len - text.len();
        let chunk = if character.is_ascii() {
            let length = 1 + ascii_prefix_len(self.chars.as_str().as_bytes());
            let (run, remaining) = text.split_at(length);
            self.chars = remaining.chars();
            AsciiOrChar::Ascii(run)
        } else {
            AsciiOrChar::Char(character)
        };

        Some((offset, chunk))
    }
}

/// Skips ASCII words before locating the boundary byte, independent of host endianness.
fn ascii_prefix_len(bytes: &[u8]) -> usize {
    const WORD_BYTES: usize = size_of::<usize>();
    // Each byte's high bit distinguishes non-ASCII UTF-8 from ASCII.
    const HIGH_BITS: usize = usize::MAX / 255 * 128;

    let mut end = 0;
    for chunk in bytes.as_chunks::<WORD_BYTES>().0 {
        let word = usize::from_ne_bytes(*chunk);
        if word & HIGH_BITS != 0 {
            break;
        }
        end += WORD_BYTES;
    }

    let tail = &bytes[end..];
    end + tail.iter().position(|byte| !byte.is_ascii()).unwrap_or(tail.len())
}

#[cfg(test)]
mod tests {
    use super::{AsciiOrChar, AsciiRuns};

    #[test]
    fn mixed_chunks_retain_source_byte_offsets() {
        use AsciiOrChar::{Ascii, Char};

        assert_eq!(
            AsciiRuns::new("aébc中🐍d").collect::<Vec<_>>(),
            vec![
                (0, Ascii("a")),
                (1, Char('é')),
                (3, Ascii("bc")),
                (5, Char('中')),
                (8, Char('🐍')),
                (12, Ascii("d"))
            ]
        );
    }

    #[test]
    fn empty_and_ascii_text() {
        let mut empty = AsciiRuns::new("");
        assert_eq!(empty.next(), None);
        assert_eq!(empty.next(), None);

        let text: String = (0..=127).map(char::from).collect();
        assert_eq!(
            AsciiRuns::new(&text).collect::<Vec<_>>(),
            vec![(0, AsciiOrChar::Ascii(&text))]
        );
    }

    #[test]
    fn cloned_iterator_preserves_decoder_position() {
        let mut runs = AsciiRuns::new("abé中🐍cd");
        assert_eq!(runs.next(), Some((0, AsciiOrChar::Ascii("ab"))));
        assert_eq!(runs.next(), Some((2, AsciiOrChar::Char('é'))));
        let cloned = runs.clone();
        assert_eq!(
            runs.collect::<Vec<_>>(),
            vec![
                (4, AsciiOrChar::Char('中')),
                (7, AsciiOrChar::Char('🐍')),
                (11, AsciiOrChar::Ascii("cd"))
            ]
        );
        assert_eq!(cloned.map(|(offset, _)| offset).collect::<Vec<_>>(), vec![4, 7, 11]);
    }

    #[test]
    fn word_boundaries_and_all_utf8_widths_round_trip() {
        for prefix_len in 0..=3 * size_of::<usize>() {
            for character in [
                '\u{80}',
                'é',
                '\u{7ff}',
                '\u{800}',
                '中',
                '\u{ffff}',
                '🐍',
                '\u{10ffff}',
            ] {
                let text = format!(
                    "{}{character}{character}{}",
                    "A".repeat(prefix_len),
                    "z".repeat(prefix_len)
                );
                let mut reconstructed = String::new();
                for (offset, chunk) in AsciiRuns::new(&text) {
                    assert_eq!(offset, reconstructed.len());
                    match chunk {
                        AsciiOrChar::Ascii(run) => {
                            assert!(!run.is_empty() && run.is_ascii());
                            let end = offset + run.len();
                            assert!(end == text.len() || !text.as_bytes()[end].is_ascii());
                            reconstructed.push_str(run);
                        }
                        AsciiOrChar::Char(c) => {
                            assert!(!c.is_ascii());
                            reconstructed.push(c);
                        }
                    }
                }
                assert_eq!(reconstructed, text);
            }
        }
    }
}
