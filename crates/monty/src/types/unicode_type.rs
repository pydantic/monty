//! Unicode character properties for `str`, backed by the CPython-generated tables in
//! `unicode_type_data.rs` (CPython's `unicodectype.c` in miniature) so results, and the Unicode
//! version, follow the target CPython rather than Rust std.

use super::unicode_type_data::{
    ALPHA, CASE_IGNORABLE, CASED, DECIMAL, DIGIT, EXTENDED, EXTENDED_CASE, ID_CONTINUE, ID_START, INDEX1, INDEX2,
    LOWER, NUMERIC, PRINTABLE, RECORDS, SHIFT, SPACE, TITLE, TypeRecord, UPPER,
};

/// Full lowercase of `s` with `Final_Sigma` applied; ASCII input takes the bulk path.
pub(super) fn lowercase(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        let mut out = String::with_capacity(s.len());
        for (i, c) in s.char_indices() {
            type_record(c).push_lower(&mut out, s, i, c);
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
            type_record(c).push_upper(&mut out, c);
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
            type_record(c).push_fold(&mut out, c);
        }
        out
    }
}

/// Whitespace as `str.isspace()`, `split()` and `strip()` define it, which unlike `char::is_whitespace`
/// includes the ASCII separators `\x1c`–`\x1f`.
pub(super) fn is_space(c: char) -> bool {
    type_record(c).is_space()
}

/// Looks up the record of `c`: two index reads and no data-dependent branches.
pub(super) fn type_record(c: char) -> &'static TypeRecord {
    let cp = c as usize;
    let block = INDEX1[cp >> SHIFT] as usize;
    &RECORDS[INDEX2[(block << SHIFT) | (cp & ((1 << SHIFT) - 1))] as usize]
}

impl TypeRecord {
    /// `Uppercase` property, what CPython's `_PyUnicode_IsUppercase` checks.
    pub(super) fn is_upper(&self) -> bool {
        self.has(UPPER)
    }

    /// `Lowercase` property, what CPython's `_PyUnicode_IsLowercase` checks.
    pub(super) fn is_lower(&self) -> bool {
        self.has(LOWER)
    }

    /// General category `Lt`, which `str.istitle()` treats like an uppercase letter.
    pub(super) fn is_title(&self) -> bool {
        self.has(TITLE)
    }

    /// `Cased` property, which delimits words in `str.title()` and contexts for `Final_Sigma`.
    pub(super) fn is_cased(&self) -> bool {
        self.has(CASED)
    }

    /// `Case_Ignorable` property, skipped when looking for the `Final_Sigma` context.
    pub(super) fn is_case_ignorable(&self) -> bool {
        self.has(CASE_IGNORABLE)
    }

    /// `str.isalpha()`: general categories `Lu`, `Ll`, `Lt`, `Lm` and `Lo`.
    pub(super) fn is_alpha(&self) -> bool {
        self.has(ALPHA)
    }

    /// `str.isdecimal()`: general category `Nd`.
    pub(super) fn is_decimal(&self) -> bool {
        self.has(DECIMAL)
    }

    /// `str.isdigit()`: `Numeric_Type` of `Decimal` or `Digit`.
    pub(super) fn is_digit(&self) -> bool {
        self.has(DIGIT)
    }

    /// `str.isnumeric()`: any `Numeric_Type`, including CJK numeric ideographs.
    pub(super) fn is_numeric(&self) -> bool {
        self.has(NUMERIC)
    }

    /// `str.isspace()`: bidirectional class `WS`, `B` or `S`, or general category `Zs`.
    pub(super) fn is_space(&self) -> bool {
        self.has(SPACE)
    }

    /// `str.isprintable()`: everything except `Cc`, `Cf`, `Cs`, `Co`, `Cn`, `Zl`, `Zp` and `Zs` other than space.
    pub(super) fn is_printable(&self) -> bool {
        self.has(PRINTABLE)
    }

    /// Valid first character of an identifier: `XID_Start` or `_`.
    pub(super) fn is_id_start(&self) -> bool {
        self.has(ID_START)
    }

    /// Valid later character of an identifier: `XID_Continue`.
    pub(super) fn is_id_continue(&self) -> bool {
        self.has(ID_CONTINUE)
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

    fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
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
        .map(type_record)
        .find(|record| !record.is_case_ignorable())
        .is_some_and(TypeRecord::is_cased)
}
