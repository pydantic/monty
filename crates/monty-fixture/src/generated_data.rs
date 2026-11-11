use serde::Deserialize;

/// A file path or a specification for benchmark inputs prepared before timing.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum BenchData {
    File(String),
    Generated(GeneratedData),
}

/// Text and optional random offsets passed to a benchmark as `DATA` and `OFFSETS`.
#[derive(Debug, PartialEq, Eq)]
pub struct BenchInput {
    pub text: String,
    pub offsets: Vec<i64>,
}

/// Generate a reproducible document and optional lookup offsets outside the timed loop.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedData {
    pub corpus: Corpus,
    pub chars: usize,
    #[serde(default)]
    pub offsets: usize,
}

/// Vocabulary used by the generated document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corpus {
    MostlyAscii,
    Multilingual,
}

impl GeneratedData {
    #[must_use]
    pub fn generate(&self) -> BenchInput {
        let mut rng = Rng(42);
        let mut text = String::with_capacity(self.chars);
        let mut chars = 0;
        let mut entry = 0;

        while chars < self.chars {
            let paragraph = paragraph(&mut rng, entry, self.corpus);
            chars += paragraph.chars().count();
            text.push_str(&paragraph);
            entry += 1;
        }

        let end = text.char_indices().nth(self.chars).map_or(text.len(), |(byte, _)| byte);
        text.truncate(end);

        // Use a separate stream so lookup positions stay the same across corpora and sizes.
        let mut rng = Rng(123);
        let offsets = (0..self.offsets).map(|_| i64::from(rng.next())).collect();
        BenchInput { text, offsets }
    }
}

fn paragraph(rng: &mut Rng, entry: usize, corpus: Corpus) -> String {
    let names = [
        "Zoë", "André", "Müller", "Renée", "Björn", "Lucía", "José", "Chloë", "Maël", "Anaïs",
    ];
    let english = [
        "The group checked the equipment and updated the delivery schedule.",
        "Visitors can collect the revised timetable and a map at reception.",
        "Record the materials, quantities and contact details before placing the order.",
        "The next workshop will include time for questions and practical exercises.",
        "Keep the meeting notes with the drawings so corrections can be checked later.",
        "Allow extra time to cross the bridge and return through the public garden.",
    ];
    let translated = [
        "Die nächste Führung beginnt am Haupteingang. Bitte bringen Sie eine Jacke mit.",
        "La bibliothèque prépare une soirée de lecture. Chacun peut proposer une question.",
        "请记录材料的数量和交付日期。如果地址有变化，请在寄出包裹前通知工作人员。",
        "駅の東口から川に沿って歩いてください。受付では予約番号をお知らせください。",
        "The booking is confirmed ✓ and the finished work is ready for collection 🎉.",
        "The reading room opens at nine; please return borrowed books to the front desk.",
    ];
    let first = names[rng.index(names.len())];
    let second = names[rng.index(names.len())];
    let opening = english[rng.index(english.len())];
    let closing = match corpus {
        Corpus::MostlyAscii => english[rng.index(english.len())],
        Corpus::Multilingual => translated[rng.index(translated.len())],
    };

    match rng.index(3) {
        0 => format!("{first} recorded entry {entry}. {opening} {second} will follow up. {closing}\n\n"),
        1 => format!("{opening} Entry {entry} was checked by {first} and {second}. {closing}\n\n"),
        _ => format!("{opening} {closing} Follow-up for entry {entry}: {first} and {second}.\n\n"),
    }
}

/// A fixed-seed LCG; discard its low bits, which have short cycles.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0 >> 8
    }

    fn index(&mut self, length: usize) -> usize {
        self.next() as usize % length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_sizes_and_prefixes() {
        for corpus in [Corpus::MostlyAscii, Corpus::Multilingual] {
            let full = GeneratedData {
                corpus,
                chars: 131_072,
                offsets: 0,
            }
            .generate();
            for chars in [0, 1, 17, 512, 2048, 6472] {
                let input = GeneratedData {
                    corpus,
                    chars,
                    offsets: 0,
                }
                .generate();
                assert_eq!(input.text.chars().count(), chars);
                assert!(full.text.starts_with(&input.text));
                assert_eq!(input.offsets, [] as [i64; 0]);
            }
            assert_eq!(full.text.chars().count(), 131_072);
        }
    }

    #[test]
    fn corpus_character_distribution() {
        let ascii = GeneratedData {
            corpus: Corpus::MostlyAscii,
            chars: 131_072,
            offsets: 0,
        }
        .generate();
        let ascii_basis_points = ascii.text.chars().filter(char::is_ascii).count() * 10_000 / 131_072;
        assert!((9850..9950).contains(&ascii_basis_points), "{ascii_basis_points}");

        let multilingual = GeneratedData {
            corpus: Corpus::Multilingual,
            chars: 6472,
            offsets: 0,
        }
        .generate();
        for width in 1..=4 {
            assert!(multilingual.text.chars().any(|c| c.len_utf8() == width));
        }
    }

    #[test]
    fn lookup_offsets_are_reproducible_and_independent_of_text() {
        let spec = GeneratedData {
            corpus: Corpus::MostlyAscii,
            chars: 6472,
            offsets: 256,
        };
        let input = spec.generate();
        assert_eq!(input, spec.generate());
        let other = GeneratedData {
            corpus: Corpus::Multilingual,
            chars: 4653,
            offsets: 256,
        }
        .generate();
        assert_eq!(input.offsets, other.offsets);
        assert_eq!(input.offsets.len(), 256);
        assert!(input.offsets.iter().all(|offset| (0..1 << 24).contains(offset)));
        let mut unique = input.offsets;
        unique.sort_unstable();
        unique.dedup();
        assert!(unique.len() > 250);
    }
}
