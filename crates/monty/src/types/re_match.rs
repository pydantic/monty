//! Regex match result type for the `re` module.
//!
//! `ReMatch` represents the result of a successful regex match operation.
//! It stores the matched text, capture groups, and their positions, providing
//! Python-compatible access via `.group()`, `.groups()`, `.start()`, `.end()`,
//! and `.span()` methods.
//!
//! All data is stored as owned values (no heap references), so reference counting
//! is trivial — `py_dec_ref_ids` is a no-op.

use std::{cmp::Ordering, fmt::Write};

use ahash::AHashSet;
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{PyTrait, Str, Type, allocate_tuple},
    value::{EitherStr, Value},
};

/// A regex match result, storing captured groups and positions.
///
/// Created by `re.match()`, `re.search()`, `re.fullmatch()`, and their
/// `Pattern` method equivalents. Stores all data as owned values (no heap
/// references), which simplifies reference counting — `py_dec_ref_ids` is
/// a no-op.
///
/// The `.re` attribute (reference back to the pattern) is intentionally omitted
/// to avoid circular references between Match and Pattern objects.
///
/// # Position semantics
///
/// Positions are returned as Unicode character offsets (not byte offsets) to
/// match CPython's behavior. The conversion from byte offsets (used internally
/// by the Rust `regex` crate) happens at construction time in `from_captures`.
///
/// # Group Indexing
///
/// Currently, only non-named groups are supported. Group 0 is the full match,
/// groups 1..N are capture groups. Named groups are not yet implemented.
/// If a group index other tnan integers is requested, an `TypeError` is raised.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReMatch {
    /// The full matched text (equivalent to `group(0)`).
    full_match: String,
    /// Start character position of the full match in the input string.
    start: usize,
    /// End character position of the full match in the input string.
    end: usize,
    /// Captured group strings (index 0 = group 1). `None` for unmatched optional groups.
    groups: Vec<Option<String>>,
    /// Span positions per captured group (index 0 = group 1). `None` for unmatched optional groups.
    group_spans: Vec<Option<(usize, usize)>>,
    /// Owned copy of the input string (returned by `.string` attribute).
    input_string: String,
    /// The original pattern string (used in repr output).
    pattern_string: String,
}

impl ReMatch {
    /// Creates a `ReMatch` from a `regex::Captures` result.
    ///
    /// Converts byte offsets from the regex crate into character offsets to match
    /// CPython's behavior. The full match (group 0) is always present when captures
    /// are successful.
    ///
    /// # Arguments
    /// * `caps` - The successful capture result from the regex engine
    /// * `input` - The full input string that was searched
    /// * `pattern` - The original pattern string (for repr)
    pub fn from_captures(caps: &fancy_regex::Captures<'_>, input: &str, pattern: &str) -> Self {
        let full = caps.get(0).expect("group 0 always exists on a successful match");
        let full_match = full.as_str().to_owned();
        let start = byte_to_char_offset(input, full.start());
        let end = byte_to_char_offset(input, full.end());

        let group_count = caps.len().saturating_sub(1);
        let mut groups = Vec::with_capacity(group_count);
        let mut group_spans = Vec::with_capacity(group_count);

        for cap in caps.iter().skip(1) {
            if let Some(m) = cap {
                groups.push(Some(m.as_str().to_owned()));
                group_spans.push(Some((
                    byte_to_char_offset(input, m.start()),
                    byte_to_char_offset(input, m.end()),
                )));
            } else {
                groups.push(None);
                group_spans.push(None);
            }
        }

        Self {
            full_match,
            start,
            end,
            groups,
            group_spans,
            input_string: input.to_owned(),
            pattern_string: pattern.to_owned(),
        }
    }

    /// Returns the match for a given group number.
    ///
    /// Group 0 is the full match, groups 1..N are capture groups.
    /// Returns `Value::None` for unmatched optional groups.
    /// Raises `IndexError` for invalid group numbers.
    fn get_group(&self, n: i64, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => {
                let s = Str::new(self.full_match.clone());
                Ok(Value::Ref(heap.allocate(HeapData::Str(s))?))
            }
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.groups.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                match &self.groups[idx] {
                    Some(s) => {
                        let s = Str::new(s.clone());
                        Ok(Value::Ref(heap.allocate(HeapData::Str(s))?))
                    }
                    None => Ok(Value::None),
                }
            }
        }
    }

    /// Returns a tuple of all capture group strings.
    ///
    /// Unmatched optional groups appear as `None`.
    fn get_groups(&self, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut elements = smallvec![];
        for group in &self.groups {
            match group {
                Some(s) => {
                    let s = Str::new(s.clone());
                    elements.push(Value::Ref(heap.allocate(HeapData::Str(s))?));
                }
                None => elements.push(Value::None),
            }
        }
        Ok(allocate_tuple(elements, heap)?)
    }

    /// Returns the start character position for a given group.
    ///
    /// Group 0 is the full match. Returns -1 for unmatched optional groups
    #[expect(clippy::cast_possible_wrap, reason = "positions are always small enough for i64")]
    fn get_start(&self, n: i64) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => Ok(Value::Int(self.start as i64)),
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.group_spans.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                match &self.group_spans[idx] {
                    Some((s, _)) => Ok(Value::Int(*s as i64)),
                    None => Ok(Value::Int(-1)),
                }
            }
        }
    }

    /// Returns the end character position for a given group.
    ///
    /// Group 0 is the full match. Returns -1 for unmatched optional groups
    #[expect(clippy::cast_possible_wrap, reason = "positions are always small enough for i64")]
    fn get_end(&self, n: i64) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => Ok(Value::Int(self.end as i64)),
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.group_spans.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                match &self.group_spans[idx] {
                    Some((_, e)) => Ok(Value::Int(*e as i64)),
                    None => Ok(Value::Int(-1)),
                }
            }
        }
    }

    /// Returns a `(start, end)` tuple for a given group.
    ///
    /// Group 0 is the full match. Returns `(-1, -1)` for unmatched optional groups
    #[expect(clippy::cast_possible_wrap, reason = "positions are always small enough for i64")]
    fn get_span(&self, n: i64, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => Ok(allocate_tuple(
                smallvec![Value::Int(self.start as i64), Value::Int(self.end as i64)],
                heap,
            )?),
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.group_spans.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                let (s, e) = match &self.group_spans[idx] {
                    Some((s, e)) => (*s as i64, *e as i64),
                    None => (-1, -1),
                };
                Ok(allocate_tuple(smallvec![Value::Int(s), Value::Int(e)], heap)?)
            }
        }
    }
}

impl PyTrait for ReMatch {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::ReMatch
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        None
    }

    fn py_eq(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<bool, ResourceError> {
        // Match objects are not comparable
        Ok(false)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No heap references — all data is owned strings and integers.
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        // Match objects are always truthy
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::fmt::Result {
        write!(
            f,
            "<re.Match object; span=({}, {}), match='{}'>",
            self.start, self.end, self.full_match
        )
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.full_match.len()
            + self.input_string.len()
            + self.pattern_string.len()
            + self
                .groups
                .iter()
                .map(|g| g.as_ref().map_or(0, String::len))
                .sum::<usize>()
    }

    fn py_getattr(
        &self,
        attr_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<super::AttrCallResult>> {
        match StaticStrings::from_string_id(attr_id) {
            Some(StaticStrings::StringAttr) => {
                let s = Str::new(self.input_string.clone());
                let v = Value::Ref(heap.allocate(HeapData::Str(s))?);
                Ok(Some(super::AttrCallResult::Value(v)))
            }
            _ => Err(ExcType::attribute_error(Type::ReMatch, interns.get_str(attr_id))),
        }
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        match attr.static_string() {
            Some(StaticStrings::Group) => {
                let n = extract_optional_group_arg(args, "re.Match.group", 0, heap)?;
                self.get_group(n, heap)
            }
            Some(StaticStrings::Groups) => {
                args.check_zero_args("re.Match.groups", heap)?;
                self.get_groups(heap)
            }
            Some(StaticStrings::Start) => {
                let n = extract_optional_group_arg(args, "re.Match.start", 0, heap)?;
                self.get_start(n)
            }
            Some(StaticStrings::End) => {
                let n = extract_optional_group_arg(args, "re.Match.end", 0, heap)?;
                self.get_end(n)
            }
            Some(StaticStrings::Span) => {
                let n = extract_optional_group_arg(args, "re.Match.span", 0, heap)?;
                self.get_span(n, heap)
            }
            _ => Err(ExcType::attribute_error(Type::ReMatch, attr.as_str(interns))),
        }
    }
}

/// Extracts an optional integer argument for group-related methods.
///
/// Many `re.Match` methods accept an optional group number that defaults to 0.
/// This helper extracts the argument, validates it is an integer, and returns
/// the group number.
fn extract_optional_group_arg(
    args: ArgValues,
    name: &str,
    default: i64,
    heap: &mut Heap<impl ResourceTracker>,
) -> RunResult<i64> {
    let opt = args.get_zero_one_arg(name, heap)?;
    match opt {
        None => Ok(default),
        Some(Value::Int(n)) => Ok(n),
        Some(other) => {
            other.drop_with_heap(heap);
            Err(ExcType::re_match_group_index_error())
        }
    }
}

/// Converts a byte offset in a UTF-8 string to a character (code point) offset.
///
/// The Rust `regex` crate operates on byte offsets, but Python's `re` module
/// returns character positions. For ASCII-only strings, these are identical.
/// For multi-byte UTF-8 characters, this counts actual code points up to the
/// byte position.
fn byte_to_char_offset(s: &str, byte_offset: usize) -> usize {
    s[..byte_offset].chars().count()
}

/// Converts a positive group number (1-based) to a 0-based index.
///
/// The caller must ensure `n > 0`.
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "n is always positive (checked by caller via match on Ordering::Greater)"
)]
fn group_index(n: i64) -> usize {
    (n - 1) as usize
}
