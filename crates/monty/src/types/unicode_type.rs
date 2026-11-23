//! Unicode character properties for `str`, backed by the CPython-generated tables in
//! `unicode_type_data.rs` (CPython's `unicodectype.c` in miniature) so results, and the Unicode
//! version, follow the target CPython rather than Rust std.

use monty_types::{LARGE_RESULT_THRESHOLD, ResourceError, ResourceTracker};

use super::unicode_type_data::{
    ALPHA, CASE_IGNORABLE, CASED, DECIMAL, DIGIT, EXTENDED, EXTENDED_CASE, ID_CONTINUE, ID_START, INDEX1, INDEX2,
    LOWER, NUMERIC, PRINTABLE, RECORDS, SHIFT, SPACE, TITLE, TypeRecord, UPPER,
};
use crate::resource_checks::check_estimated_size;

/// Full lowercase of `s` with `Final_Sigma` applied; ASCII input takes the bulk path.
pub(super) fn lowercase(s: &str, tracker: &ResourceTracker) -> Result<String, ResourceError> {
    if s.is_ascii() {
        check_estimated_size(s.len(), tracker)?;
        Ok(s.to_ascii_lowercase())
    } else {
        map_case(s, tracker, |out, i, c| type_record(c).push_lower(out, s, i, c))
    }
}

/// Full uppercase of `s`; ASCII input takes the bulk path.
pub(super) fn uppercase(s: &str, tracker: &ResourceTracker) -> Result<String, ResourceError> {
    if s.is_ascii() {
        check_estimated_size(s.len(), tracker)?;
        Ok(s.to_ascii_uppercase())
    } else {
        map_case(s, tracker, |out, _, c| type_record(c).push_upper(out, c))
    }
}

/// Full default case folding of `s`, without normalization or locale tailoring; ASCII input takes the bulk path.
pub(super) fn casefold(s: &str, tracker: &ResourceTracker) -> Result<String, ResourceError> {
    if s.is_ascii() {
        check_estimated_size(s.len(), tracker)?;
        Ok(s.to_ascii_lowercase())
    } else {
        map_case(s, tracker, |out, _, c| type_record(c).push_fold(out, c))
    }
}

/// Most bytes one char's full case mapping can append: at most three code points, each at most 4 bytes.
const MAX_MAPPING_LEN: usize = 12;

/// Builds a case mapping of `s`, calling `push(out, byte_offset, char)` for each char.
///
/// A mapping can outgrow its input (`'ɐ'.upper()` is 3 bytes from 2), so for large inputs each
/// growth of the buffer is checked against `tracker` before it happens. The buffer starts with
/// room for one extra mapping, so a mapping that keeps the input's length never grows it. Inputs
/// below `LARGE_RESULT_THRESHOLD / MAX_MAPPING_LEN` bytes, a conservative bound, skip the checks.
pub(super) fn map_case(
    s: &str,
    tracker: &ResourceTracker,
    mut push: impl FnMut(&mut String, usize, char),
) -> Result<String, ResourceError> {
    if s.len().saturating_mul(MAX_MAPPING_LEN) <= LARGE_RESULT_THRESHOLD {
        let mut out = String::with_capacity(s.len());
        for (i, c) in s.char_indices() {
            push(&mut out, i, c);
        }
        Ok(out)
    } else {
        let capacity = s.len().saturating_add(MAX_MAPPING_LEN);
        tracker.check_allocation(capacity)?;
        let mut out = String::with_capacity(capacity);
        let mut start = 0;
        while start < s.len() {
            let room = out.capacity() - out.len();
            if room < MAX_MAPPING_LEN {
                // Room for the rest at 1:1, but at least an eighth more so that expanding input
                // reallocates only a few times. The allocator charges a reallocation for its growth.
                let rest = s.len() - start;
                let additional = rest
                    .max(out.capacity() / 8)
                    .min(rest.saturating_mul(MAX_MAPPING_LEN))
                    .saturating_add(MAX_MAPPING_LEN);
                tracker.check_allocation(additional)?;
                out.reserve_exact(additional);
                continue;
            }
            // Map in chunks that fit without reallocating: no more chars start in the chunk than it
            // has bytes, and widening it to a char boundary only completes a char that starts inside.
            let mut end = (start + room / MAX_MAPPING_LEN).min(s.len());
            while !s.is_char_boundary(end) {
                end += 1;
            }
            for (j, c) in s[start..end].char_indices() {
                push(&mut out, start + j, c);
            }
            start = end;
        }
        Ok(out)
    }
}

/// Whitespace as `str.isspace()`, `split()` and `strip()` define it, which unlike `char::is_whitespace`
/// includes the ASCII separators `\x1c`–`\x1f`.
pub(super) fn is_space(c: char) -> bool {
    type_record(c).is_space()
}

/// The value of a Unicode decimal digit (`unicodedata.decimal`), `None` for other characters.
///
/// Decimal digits come in contiguous runs of ten starting at zero, so the value is the
/// distance from the start of the run, modulo ten for blocks that stack several runs.
pub(super) fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        Some(u32::from(c) - u32::from('0'))
    } else if type_record(c).is_decimal() {
        let cp = u32::from(c);
        let mut start = cp;
        while start > 0 && char::from_u32(start - 1).is_some_and(|prev| type_record(prev).is_decimal()) {
            start -= 1;
        }
        Some((cp - start) % 10)
    } else {
        None
    }
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
