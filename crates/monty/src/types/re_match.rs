//! Regex match result type for the `re` module.
//!
//! `ReMatch` represents the result of a successful regex match operation.
//! It stores the matched text, capture groups, and their positions, providing
//! Python-compatible access via `.group()`, `.groups()`, `.start()`, `.end()`,
//! and `.span()` methods.
//!
//! The matched substrings are owned copies, but the subject string (`.string`)
//! is held as a single refcounted heap reference shared across every match from
//! one `finditer`/`findall` call, so `py_dec_ref_ids` releases that reference.

use std::{cell::OnceCell, cmp::Ordering, fmt::Write, mem};

use smallvec::smallvec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::StaticStrings,
    resource::ResourceTracker,
    types::{
        Dict, LazyHeapSet, PyTrait, Type, allocate_tuple,
        str::{allocate_string, string_repr_fmt},
    },
    value::{EitherStr, Value},
};

/// A regex match result, storing captured groups and positions.
///
/// Created by `re.match()`, `re.search()`, `re.fullmatch()`, and their
/// `Pattern` method equivalents. The captured substrings are owned copies, but
/// the subject (`.string`) is a refcounted heap reference (`subject`) shared
/// across every match from one call rather than copied per match — so
/// `py_dec_ref_ids` must release it, and its memory is tracked once.
///
/// The `.re` attribute (reference back to the pattern) is intentionally omitted
/// to avoid circular references between Match and Pattern objects.
///
/// # Position semantics
///
/// Offsets are stored as byte offsets (what the `regex` crate produces) and
/// converted to the character offsets CPython reports lazily, only when
/// `.start()`/`.end()`/`.span()`/`repr` read them; the full match's conversion
/// is memoized (`char_span`). ASCII input needs no conversion.
///
/// # Group Indexing
///
/// Group 0 is the full match, groups 1..N are capture groups.
/// Both integer and named group access are supported — named groups are looked
/// up via the `named_groups` mapping.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReMatch {
    /// The full matched text (equivalent to `group(0)`).
    full_match: String,
    /// Start byte position of the full match in the input string.
    start: usize,
    /// End byte position of the full match in the input string.
    end: usize,
    /// Captured group strings (index 0 = group 1). `None` for unmatched optional groups.
    groups: Vec<Option<String>>,
    /// Span byte positions per captured group (index 0 = group 1). `None` for unmatched optional groups.
    group_spans: Vec<Option<(usize, usize)>>,
    /// Named groups: maps group name → 1-based group index.
    named_groups: Vec<(String, usize)>,
    /// The searched subject (the `.string` attribute), held as a refcounted heap
    /// reference so every match from one `finditer` shares one copy — tracked
    /// once, not per match. Always a `str` `Value` (heap `Str` or interned).
    subject: Value,
    /// Whether the subject is pure ASCII, so byte offset == character offset.
    all_ascii: bool,
    /// Char-offset `(start, end)` of the full match, computed lazily by
    /// [`ReMatch::char_span`]. A pure cache — skipped by serde, rebuilt on demand.
    #[serde(skip)]
    char_span: OnceCell<(i64, i64)>,
}

impl ReMatch {
    /// Creates a `ReMatch` from a `fancy_regex::Captures` result.
    ///
    /// Stores byte offsets (converted to char offsets lazily — see the type docs).
    /// `subject` is the already-refcounted subject `Value` this match keeps alive
    /// (the caller clones it per match); `regex` supplies named-group mappings.
    pub fn from_captures(
        caps: &fancy_regex::Captures<'_>,
        subject: Value,
        all_ascii: bool,
        regex: &fancy_regex::Regex,
    ) -> Self {
        let full = caps.get(0).expect("group 0 always exists on a successful match");
        let full_match = full.as_str().to_owned();
        let start = full.start();
        let end = full.end();

        let group_count = caps.len().saturating_sub(1);
        let mut groups = Vec::with_capacity(group_count);
        let mut group_spans = Vec::with_capacity(group_count);

        for cap in caps.iter().skip(1) {
            if let Some(m) = cap {
                groups.push(Some(m.as_str().to_owned()));
                group_spans.push(Some((m.start(), m.end())));
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
            subject,
            all_ascii,
            char_span: OnceCell::new(),
        }
    }

    /// The subject `Value` this match keeps alive (its `.string`). Lets the heap
    /// walkers in `heap.rs` follow and release the shared subject reference.
    pub(crate) fn subject_ref(&self) -> &Value {
        &self.subject
    }

    /// The `(start, end)` char offsets of the full match — what `.start()`,
    /// `.end()`, `.span()`, and `repr` report. Memoized: a non-ASCII subject is
    /// scanned once (only up to the match end) on first access, O(1) after.
    fn char_span(&self, vm: &VM<'_, impl ResourceTracker>) -> (i64, i64) {
        *self.char_span.get_or_init(|| {
            let (start, end) = if self.all_ascii {
                (self.start, self.end)
            } else {
                let text = self.subject.to_str(vm).expect("subject is always a str");
                let start = byte_to_char_offset(text, self.start);
                // Count only the matched slice for `end` instead of rescanning from zero.
                (start, start + text[self.start..self.end].chars().count())
            };
            (position_i64(start), position_i64(end))
        })
    }

    /// Converts a stored byte offset into the subject to the Unicode character
    /// offset CPython reports — used for capture-group positions (the full
    /// match's span goes through the memoized [`ReMatch::char_span`] instead).
    /// ASCII subjects need no conversion; otherwise the subject text is read
    /// back from the heap for a forward scan.
    fn char_offset(&self, byte_offset: usize, vm: &VM<'_, impl ResourceTracker>) -> i64 {
        let char_offset = if self.all_ascii {
            byte_offset
        } else {
            let text = self.subject.to_str(vm).expect("subject is always a str");
            byte_to_char_offset(text, byte_offset)
        };
        position_i64(char_offset)
    }

    /// Returns the match for a given group number.
    ///
    /// Group 0 is the full match, groups 1..N are capture groups.
    /// Returns `Value::None` for unmatched optional groups.
    /// Raises `IndexError` for invalid group numbers.
    fn get_group(&self, n: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => Ok(allocate_string(self.full_match.as_str(), heap)?),
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.groups.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                match &self.groups[idx] {
                    Some(s) => Ok(allocate_string(s.as_str(), heap)?),
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
    fn get_groupdict(&self, default: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        let this = self.get(vm.heap);
        let mut pairs = Vec::with_capacity(this.named_groups.len());
        for (name, idx) in &this.named_groups {
            let key = allocate_string(name.as_str(), vm.heap)?;
            // idx is 1-based, groups vec is 0-based (index 0 = group 1)
            let value = if *idx > 0 && (*idx - 1) < this.groups.len() {
                match &this.groups[*idx - 1] {
                    Some(s) => allocate_string(s.as_str(), vm.heap)?,
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
                Some(s) => elements.push(allocate_string(s.as_str(), heap)?),
                None => elements.push(Value::None),
            }
        }
        Ok(allocate_tuple(elements, heap)?)
    }

    /// Returns the start character position for a given group.
    ///
    /// Group 0 is the full match. Returns -1 for unmatched optional groups
    fn get_start(&self, n: i64, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => Ok(Value::Int(self.char_span(vm).0)),
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.group_spans.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                match &self.group_spans[idx] {
                    Some((s, _)) => Ok(Value::Int(self.char_offset(*s, vm))),
                    None => Ok(Value::Int(-1)),
                }
            }
        }
    }

    /// Returns the end character position for a given group.
    ///
    /// Group 0 is the full match. Returns -1 for unmatched optional groups
    fn get_end(&self, n: i64, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => Ok(Value::Int(self.char_span(vm).1)),
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.group_spans.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                match &self.group_spans[idx] {
                    Some((_, e)) => Ok(Value::Int(self.char_offset(*e, vm))),
                    None => Ok(Value::Int(-1)),
                }
            }
        }
    }

    /// Returns a `(start, end)` tuple for a given group.
    ///
    /// Group 0 is the full match. Returns `(-1, -1)` for unmatched optional groups
    fn get_span(&self, n: i64, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
        match n.cmp(&0) {
            Ordering::Equal => {
                let (start, end) = self.char_span(vm);
                Ok(allocate_tuple(smallvec![Value::Int(start), Value::Int(end)], vm.heap)?)
            }
            Ordering::Less => Err(ExcType::re_match_group_index_error()),
            Ordering::Greater => {
                let idx = group_index(n);
                if idx >= self.group_spans.len() {
                    return Err(ExcType::re_match_group_index_error());
                }
                let (s, e) = match &self.group_spans[idx] {
                    Some((s, e)) => (self.char_offset(*s, vm), self.char_offset(*e, vm)),
                    None => (-1, -1),
                };
                Ok(allocate_tuple(smallvec![Value::Int(s), Value::Int(e)], vm.heap)?)
            }
        }
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, ReMatch> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::ReMatch
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        // Match objects use identity equality (handled before the heap read).
        Ok(None)
    }

    fn py_bool(&self, _vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        // Match objects are always truthy
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        let m = self.get(vm.heap);
        let (start, end) = m.char_span(vm);
        write!(f, "<re.Match object; span=({start}, {end}), match=")?;
        string_repr_fmt(&m.full_match, f)?;
        Ok(f.write_char('>')?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        match attr.static_string() {
            Some(StaticStrings::StringAttr) => {
                // Hand back the same subject object (CPython's `m.string is s`),
                // bumping its refcount rather than allocating a fresh copy.
                let v = self.get(vm.heap).subject.clone_with_heap(vm);
                Ok(Some(CallResult::Value(v)))
            }
            _ => Err(ExcType::attribute_error(Type::ReMatch, attr.as_str(vm.interns))),
        }
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
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
                let GroupdictArgs { default } = GroupdictArgs::from_args(args, vm)?;
                let default = default.unwrap_or(Value::None);
                let result = self.get_groupdict(&default, vm)?;
                default.drop_with_heap(vm);
                result
            }
            Some(StaticStrings::Start) => {
                let n = extract_optional_group_arg(args, "re.Match.start", 0, vm.heap)?;
                self.get(vm.heap).get_start(n, vm)?
            }
            Some(StaticStrings::End) => {
                let n = extract_optional_group_arg(args, "re.Match.end", 0, vm.heap)?;
                self.get(vm.heap).get_end(n, vm)?
            }
            Some(StaticStrings::Span) => {
                let n = extract_optional_group_arg(args, "re.Match.span", 0, vm.heap)?;
                self.get(vm.heap).get_span(n, vm)?
            }
            _ => {
                return Err(ExcType::attribute_error(Type::ReMatch, attr.as_str(vm.interns)));
            }
        };
        Ok(CallResult::Value(result))
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
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
        // The subject is NOT counted here: it is a shared refcounted heap `Str`
        // charged once as its own entry, so counting it per match would inflate
        // a `finditer` of N matches to N× the subject size (see `subject`).
        mem::size_of::<Self>()
            + self.full_match.len()
            + self
                .groups
                .iter()
                .map(|g| g.as_ref().map_or(0, String::len))
                .sum::<usize>()
            + self
                .named_groups
                .iter()
                .map(|(name, _)| name.len() + mem::size_of::<usize>())
                .sum::<usize>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Release the shared subject reference (no-op for an interned subject).
        // `dec_ref_forget` neutralises the `Ref` so the struct's own `Drop`
        // does not trip the `memory-model-checks` leak assertion.
        if let Value::Ref(id) = &mut self.subject {
            stack.push(*id);
            #[cfg(feature = "memory-model-checks")]
            self.subject.dec_ref_forget();
        }
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
    vm: &mut VM<'h, impl ResourceTracker>,
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
fn resolve_group_arg(m: &ReMatch, val: &Value, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
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

/// Converts a position to the `i64` Python-facing APIs report, saturating on
/// the (practically impossible) overflow rather than wrapping negative.
fn position_i64(offset: usize) -> i64 {
    i64::try_from(offset).unwrap_or(i64::MAX)
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

/// Argument shape for `re.Match.groupdict(default=None)` — one optional
/// pos-or-keyword `default` value used to fill groups that didn't match.
#[derive(FromArgs)]
#[from_args(name = "groupdict", at_most_total)]
struct GroupdictArgs {
    #[from_args(default)]
    default: Option<Value>,
}
