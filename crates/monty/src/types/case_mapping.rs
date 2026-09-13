//! Unicode case mapping and case predicates for `str`, backed by the CPython-generated tables in
//! `case_data.rs` so results, and the Unicode version, follow the target CPython rather than Rust std.

use super::case_data::{
    CASE_IGNORABLE, CASED, CaseRecord, EXTENDED, EXTENDED_CASE, INDEX1, INDEX2, LOWER, RECORDS, SHIFT, TITLE, UPPER,
};

/// Full lowercase of `s` with `Final_Sigma` applied; ASCII input takes the bulk path.
pub(super) fn lowercase(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        let mut out = String::with_capacity(s.len());
        for (i, c) in s.char_indices() {
            case_record(c).push_lower(&mut out, s, i, c);
        }
        out
    }
}

/// Full uppercase of `s`; ASCII input takes the bulk path.
pub(super) fn uppercase(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_uppercase()
    } else {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            case_record(c).push_upper(&mut out, c);
        }
        out
    }
}

/// Full default case folding of `s`, without normalization or locale tailoring; ASCII input takes the bulk path.
pub(super) fn casefold(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            case_record(c).push_fold(&mut out, c);
        }
        out
    }
}

/// Looks up the case record of `c`: two index reads and no data-dependent branches.
pub(super) fn case_record(c: char) -> &'static CaseRecord {
    let cp = c as usize;
    let block = INDEX1[cp >> SHIFT] as usize;
    &RECORDS[INDEX2[(block << SHIFT) | (cp & ((1 << SHIFT) - 1))] as usize]
}

impl CaseRecord {
    /// `Uppercase` property, what CPython's `_PyUnicode_IsUppercase` checks.
    pub(super) fn is_upper(&self) -> bool {
        self.flags & UPPER != 0
    }

    /// `Lowercase` property, what CPython's `_PyUnicode_IsLowercase` checks.
    pub(super) fn is_lower(&self) -> bool {
        self.flags & LOWER != 0
    }

    /// General category `Lt`, which `str.istitle()` treats like an uppercase letter.
    pub(super) fn is_title(&self) -> bool {
        self.flags & TITLE != 0
    }

    /// `Cased` property, which delimits words in `str.title()` and contexts for `Final_Sigma`.
    pub(super) fn is_cased(&self) -> bool {
        self.flags & CASED != 0
    }

    /// `Case_Ignorable` property, skipped when looking for the `Final_Sigma` context.
    pub(super) fn is_case_ignorable(&self) -> bool {
        self.flags & CASE_IGNORABLE != 0
    }

    /// Appends the full lowercase of `c`, found at byte offset `i` of `s`, applying `Final_Sigma`.
    pub(super) fn push_lower(&self, out: &mut String, s: &str, i: usize, c: char) {
        if c == 'Σ' {
            out.push(if is_final_sigma(s, i) { 'ς' } else { 'σ' });
        } else {
            push_mapped(out, c, self.lower);
        }
    }

    /// Appends the full uppercase of `c`.
    pub(super) fn push_upper(&self, out: &mut String, c: char) {
        push_mapped(out, c, self.upper);
    }

    /// Appends the full titlecase of `c`.
    pub(super) fn push_title(&self, out: &mut String, c: char) {
        push_mapped(out, c, self.title);
    }

    /// Appends the full case folding of `c`.
    pub(super) fn push_fold(&self, out: &mut String, c: char) {
        push_mapped(out, c, self.fold);
    }
}

/// Appends `mapping` applied to `c`: a delta on the code point, or an `EXTENDED_CASE` string.
fn push_mapped(out: &mut String, c: char, mapping: i32) {
    if mapping >= EXTENDED {
        out.push_str(EXTENDED_CASE[(mapping - EXTENDED).cast_unsigned() as usize]);
    } else {
        // the generator only emits deltas that land on a scalar value, so the fallback never fires
        out.push(char::from_u32(u32::from(c).wrapping_add_signed(mapping)).unwrap_or(c));
    }
}

/// Whether the `Σ` at byte offset `i` of `s` is in the `Final_Sigma` context: preceded by a cased
/// character and not followed by one, skipping `Case_Ignorable` characters on both sides.
fn is_final_sigma(s: &str, i: usize) -> bool {
    cased_after_ignorables(s[..i].chars().rev()) && !cased_after_ignorables(s[i + 'Σ'.len_utf8()..].chars())
}

/// Whether the first non-`Case_Ignorable` character yielded by `chars` is cased.
fn cased_after_ignorables(chars: impl Iterator<Item = char>) -> bool {
    chars
        .map(case_record)
        .find(|record| !record.is_case_ignorable())
        .is_some_and(CaseRecord::is_cased)
}
