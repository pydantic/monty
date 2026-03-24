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
    bytecode::{CallResult, VM},
    defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Dict, PyTrait, Str, Type, allocate_tuple, str::string_repr_fmt},
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
/// Group 0 is the full match, groups 1..N are capture groups.
/// Both integer and named group access are supported — named groups are looked
/// up via the `named_groups` mapping.
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
    /// Named groups: maps group name → 1-based group index.
    named_groups: Vec<(String, usize)>,
    /// Owned copy of the input string (returned by `.string` attribute).
    input_string: String,
    /// The original pattern string (used in repr output).
    pattern_string: String,
}

impl ReMatch {
    /// Creates a `ReMatch` from a `fancy_regex::Captures` result.
    ///
    /// Converts byte offsets from the regex crate into character offsets to match
    /// CPython's behavior. The full match (group 0) is always present when captures
    /// are successful.
    ///
    /// # Arguments
    /// * `caps` - The successful capture result from the regex engine
    /// * `input` - The full input string that was searched
    /// * `pattern` - The original pattern string (for repr)
    /// * `regex` - The compiled regex, used to extract named group mappings
    pub fn from_captures(
        caps: &fancy_regex::Captures<'_>,
        input: &str,
        pattern: &str,
        regex: &fancy_regex::Regex,
    ) -> Self {
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

        // Extract named group name→index mappings from the regex
        let mut named_groups = Vec::new();
        for (idx, name) in regex.capture_names().enumerate() {
            if let Some(name) = name {
                named_groups.push((name.to_owned(), idx));
            }
        }

        Self {
            full_match,
            start,
            end,
            groups,
            group_spans,
            named_groups,
            input_string: input.to_owned(),
            pattern_string: pattern.to_owned(),
        }
    }

    /// Returns the match for a given group number.
    ///
    /// Group 0 is the full match, groups 1..N are capture groups.
    /// Returns `Value::None` for unmatched optional groups.
    /// Raises `IndexError` for invalid group numbers.
    fn get_group(&self, n: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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

    /// Returns the match for a named group.
    ///
    /// Looks up the group name in `named_groups` and delegates to `get_group`.
    /// Raises `IndexError` if the name is not found.
    fn get_group_by_name(&self, name: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        for (group_name, idx) in &self.named_groups {
            if group_name == name {
                #[expect(clippy::cast_possible_wrap, reason = "group indices are always small")]
                return self.get_group(*idx as i64, heap);
            }
        }
        Err(ExcType::re_match_group_index_error())
    }
}

impl<'h> HeapRead<'h, ReMatch> {
    /// Returns a dict mapping named group names to their matched strings.
    ///
    /// Groups that didn't participate in the match have the `default` value
    /// (typically `None`).
    fn get_groupdict(&self, default: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let this = self.get(vm.heap);
        let mut pairs = Vec::with_capacity(this.named_groups.len());
        for (name, idx) in &this.named_groups {
            let key_str = Str::new(name.clone());
            let key = Value::Ref(vm.heap.allocate(HeapData::Str(key_str))?);
            // idx is 1-based, groups vec is 0-based (index 0 = group 1)
            let value = if *idx > 0 && (*idx - 1) < this.groups.len() {
                match &this.groups[*idx - 1] {
                    Some(s) => {
                        let s = Str::new(s.clone());
                        Value::Ref(vm.heap.allocate(HeapData::Str(s))?)
                    }
                    None => default.clone_with_heap(vm),
                }
            } else {
                default.clone_with_heap(vm)
            };
            pairs.push((key, value));
        }
        let dict = Dict::from_pairs(pairs, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))?))
    }
}

impl ReMatch {
    /// Returns a tuple of all capture group strings.
    ///
    /// Unmatched optional groups appear as `None`.
    fn get_groups(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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
    fn get_span(&self, n: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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

impl ReMatch {
    /// Formats this match for `repr()` output.
    ///
    /// Kept as an inherent method so `HeapData` can call it without going
    /// through a `HeapRead` handle.
    pub fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _vm: &VM<'_, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        write!(f, "<re.Match object; span=({}, {}), match=", self.start, self.end)?;
        string_repr_fmt(&self.full_match, f)?;
        Ok(f.write_char('>')?)
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, ReMatch> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::ReMatch
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, _other: &Self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        // Match objects are not comparable
        Ok(false)
    }

    fn py_bool(&self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        // Match objects are always truthy
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        self.get(vm.heap).py_repr_fmt(f, vm, heap_ids)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        match attr.static_string() {
            Some(StaticStrings::StringAttr) => {
                let s = Str::new(self.get(vm.heap).input_string.clone());
                let v = Value::Ref(vm.heap.allocate(HeapData::Str(s))?);
                Ok(Some(CallResult::Value(v)))
            }
            _ => Err(ExcType::attribute_error(Type::ReMatch, attr.as_str(vm.interns))),
        }
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let result = match attr.static_string() {
            Some(StaticStrings::Group) => call_group(self, args, vm)?,
            Some(StaticStrings::Groups) => {
                args.check_zero_args("re.Match.groups", vm.heap)?;
                self.get(vm.heap).get_groups(vm.heap)?
            }
            Some(StaticStrings::Groupdict) => {
                let default = args.get_zero_one_arg("re.Match.groupdict", vm.heap)?;
                let default = default.unwrap_or(Value::None);
                let result = self.get_groupdict(&default, vm)?;
                default.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::Start) => {
                let n = extract_optional_group_arg(args, "re.Match.start", 0, vm.heap)?;
                self.get(vm.heap).get_start(n)?
            }
            Some(StaticStrings::End) => {
                let n = extract_optional_group_arg(args, "re.Match.end", 0, vm.heap)?;
                self.get(vm.heap).get_end(n)?
            }
            Some(StaticStrings::Span) => {
                let n = extract_optional_group_arg(args, "re.Match.span", 0, vm.heap)?;
                self.get(vm.heap).get_span(n, vm.heap)?
            }
            _ => return Err(ExcType::attribute_error(Type::ReMatch, attr.as_str(vm.interns))),
        };
        Ok(CallResult::Value(result))
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        match key {
            Value::Int(n) => self.get(vm.heap).get_group(*n, vm.heap),
            Value::Bool(b) => self.get(vm.heap).get_group(i64::from(*b), vm.heap),
            Value::InternString(id) => {
                let name = vm.interns.get_str(*id);
                self.get(vm.heap).get_group_by_name(name, vm.heap)
            }
            Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
                HeapData::Str(s) => {
                    let name = s.as_str().to_owned();
                    self.get(vm.heap).get_group_by_name(&name, vm.heap)
                }
                _ => Err(ExcType::re_match_group_index_error()),
            },
            _ => Err(ExcType::re_match_group_index_error()),
        }
    }
}

impl HeapItem for ReMatch {
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
            + self
                .named_groups
                .iter()
                .map(|(name, _)| name.len() + std::mem::size_of::<usize>())
                .sum::<usize>()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No heap references — all data is owned strings and integers.
    }
}

/// Handles `m.group(...)` calls, supporting zero, one, or multiple arguments.
///
/// - `m.group()` → equivalent to `m.group(0)`, returns full match string
/// - `m.group(n)` → returns the nth group (integer or named string)
/// - `m.group(n1, n2, ...)` → returns a tuple of groups
fn call_group<'h>(
    m: &HeapRead<'h, ReMatch>,
    args: ArgValues,
    vm: &mut VM<'h, '_, impl ResourceTracker>,
) -> RunResult<Value> {
    match args {
        ArgValues::Empty => m.get(vm.heap).get_group(0, vm.heap),
        ArgValues::One(v) => {
            let result = resolve_group_arg(m.get(vm.heap), &v, vm);
            v.drop_with_heap(vm);
            result
        }
        other => {
            let pos = other.into_pos_only("re.Match.group", vm.heap)?;
            defer_drop_mut!(pos, vm);
            let mut elements = smallvec::smallvec![];
            for val in pos.as_slice() {
                let result = resolve_group_arg(m.get(vm.heap), val, vm);
                if result.is_err() {
                    // Drop already-allocated elements
                    for elem in elements {
                        Value::drop_with_heap(elem, vm);
                    }
                    return result;
                }
                elements.push(result?);
            }
            Ok(allocate_tuple(elements, vm.heap)?)
        }
    }
}

/// Resolves a single group argument — integer, bool, or string (named group).
fn resolve_group_arg(m: &ReMatch, val: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
    match val {
        Value::Int(n) => m.get_group(*n, vm.heap),
        Value::Bool(b) => m.get_group(i64::from(*b), vm.heap),
        Value::InternString(id) => {
            let name = vm.interns.get_str(*id);
            m.get_group_by_name(name, vm.heap)
        }
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(s) => {
                let name = s.as_str().to_owned();
                m.get_group_by_name(&name, vm.heap)
            }
            _ => Err(ExcType::re_match_group_index_error()),
        },
        _ => Err(ExcType::re_match_group_index_error()),
    }
}

/// Extracts an optional integer argument for group-related methods.
///
/// Many `re.Match` methods accept an optional group number that defaults to 0.
/// This helper extracts the argument, validates it is an integer (or string for
/// named groups), and returns the group number.
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
        // CPython treats bool as int subclass: True=1, False=0.
        Some(Value::Bool(b)) => Ok(i64::from(b)),
        // String group names are not valid for start/end/span — they take integers only
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
