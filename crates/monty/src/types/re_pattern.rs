//! Compiled regex pattern type for the `re` module.
//!
//! `RePattern` wraps a compiled `regex::Regex` with the original Python pattern string
//! and flags. The Rust `regex` crate guarantees linear-time matching (DFA-based),
//! preventing catastrophic backtracking (ReDoS) attacks — critical for sandbox security.
//!
//! Custom serde serializes only the pattern string and flags, recompiling the regex
//! on deserialization. This supports Monty's snapshot/restore feature.
//!
//! # Unsupported Python regex features
//!
//! The Rust `regex` crate does not support backreferences (`\1`), lookahead/lookbehind
//! (`(?=...)`, `(?!...)`), or atomic groups. Attempting to compile patterns using these
//! features raises `re.PatternError`.

use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{List, PyTrait, ReMatch, Str, Type, allocate_tuple},
    value::{EitherStr, Value},
};

/// Python regex flag: case-insensitive matching.
const IGNORECASE: u32 = 2;
/// Python regex flag: `^` and `$` match at line boundaries.
const MULTILINE: u32 = 8;
/// Python regex flag: `.` matches newlines.
const DOTALL: u32 = 16;

/// A compiled regular expression pattern.
///
/// Wraps a Rust `regex::Regex` with the original Python pattern string and flags.
///
/// This introduces a difference from CPython's implementation.
/// Switch to `fancy_regex` crate would be needed to support backreferences,
/// but given the security implications of backtracking this is not currently an option.
///
/// Custom serde serializes only the pattern string and flags, recompiling the
/// regex on deserialization. This supports Monty's snapshot/restore feature.
///
/// # Serialization Behavior
///
/// When serialized, only the original pattern string and flags are stored.
/// Upon deserialization, the regex is recompiled.
#[derive(Debug)]
pub(crate) struct RePattern {
    /// The original Python regex pattern string.
    pattern: String,
    /// Python regex flags bitmask (IGNORECASE=2, MULTILINE=8, DOTALL=16).
    flags: u32,
    /// The compiled Rust regex for `search` / `findall` / `sub` (unanchored).
    compiled: Regex,
    /// Compiled regex anchored at start for `match` operations.
    compiled_match: Regex,
    /// Compiled regex anchored at both ends for `fullmatch` operations.
    compiled_fullmatch: Regex,
}

impl RePattern {
    /// Creates a compiled pattern from a Python regex string and flags.
    ///
    /// Translates Python flag constants into inline regex flag prefixes and compiles
    /// the pattern. Also pre-compiles anchored variants for `match` and `fullmatch`.
    ///
    /// # Errors
    ///
    /// Returns `re.PatternError` if the pattern is invalid or uses features not supported
    /// by the Rust regex engine.
    pub fn compile(pattern: String, flags: u32) -> RunResult<Self> {
        let compiled = compile_regex(&pattern, flags)?;
        let compiled_match = compile_regex(&anchor_start(&pattern), flags)?;
        let compiled_fullmatch = compile_regex(&anchor_full(&pattern), flags)?;
        Ok(Self {
            pattern,
            flags,
            compiled,
            compiled_match,
            compiled_fullmatch,
        })
    }

    /// `pattern.search(string)` — find first match anywhere in the string.
    ///
    /// Returns a `ReMatch` heap object on success, or `Value::None` if no match.
    pub fn search(&self, text: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match self.compiled.captures(text) {
            Some(caps) => {
                let m = ReMatch::from_captures(&caps, text, &self.pattern);
                Ok(Value::Ref(heap.allocate(HeapData::ReMatch(m))?))
            }
            None => Ok(Value::None),
        }
    }

    /// `pattern.match(string)` — match anchored at the start of the string.
    ///
    /// Equivalent to `re.match(pattern, string)`. Only matches at position 0.
    /// Returns a `ReMatch` heap object on success, or `Value::None` if no match.
    pub fn match_start(&self, text: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match self.compiled_match.captures(text) {
            Some(caps) => {
                let m = ReMatch::from_captures(&caps, text, &self.pattern);
                Ok(Value::Ref(heap.allocate(HeapData::ReMatch(m))?))
            }
            None => Ok(Value::None),
        }
    }

    /// `pattern.fullmatch(string)` — match the entire string.
    ///
    /// Returns a `ReMatch` heap object on success, or `Value::None` if no match.
    pub fn fullmatch(&self, text: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match self.compiled_fullmatch.captures(text) {
            Some(caps) => {
                let m = ReMatch::from_captures(&caps, text, &self.pattern);
                Ok(Value::Ref(heap.allocate(HeapData::ReMatch(m))?))
            }
            None => Ok(Value::None),
        }
    }

    /// `pattern.findall(string)` — return all non-overlapping matches.
    ///
    /// Follows CPython's semantics:
    /// - No capture groups: returns a list of matched strings
    /// - One capture group: returns a list of the group's matched strings
    /// - Multiple capture groups: returns a list of tuples of matched strings
    pub fn findall(&self, text: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        let cap_count = self.compiled.captures_len();
        let mut results = Vec::new();

        if cap_count <= 1 {
            // No capture groups — return list of full match strings
            for m in self.compiled.find_iter(text) {
                let s = Str::new(m.as_str().to_owned());
                results.push(Value::Ref(heap.allocate(HeapData::Str(s))?));
            }
        } else if cap_count == 2 {
            // One capture group — return list of the group's strings
            for caps in self.compiled.captures_iter(text) {
                let val = if let Some(m) = caps.get(1) {
                    let s = Str::new(m.as_str().to_owned());
                    Value::Ref(heap.allocate(HeapData::Str(s))?)
                } else {
                    let s = Str::new(String::new());
                    Value::Ref(heap.allocate(HeapData::Str(s))?)
                };
                results.push(val);
            }
        } else {
            // Multiple capture groups — return list of tuples
            for caps in self.compiled.captures_iter(text) {
                let mut elements: SmallVec<[Value; 3]> = SmallVec::with_capacity(cap_count - 1);
                for i in 1..cap_count {
                    let val = if let Some(m) = caps.get(i) {
                        let s = Str::new(m.as_str().to_owned());
                        Value::Ref(heap.allocate(HeapData::Str(s))?)
                    } else {
                        let s = Str::new(String::new());
                        Value::Ref(heap.allocate(HeapData::Str(s))?)
                    };
                    elements.push(val);
                }
                results.push(allocate_tuple(elements, heap)?);
            }
        }

        let list = List::new(results);
        Ok(Value::Ref(heap.allocate(HeapData::List(list))?))
    }

    /// `pattern.sub(repl, string, count=0)` — substitute matches with a replacement.
    ///
    /// When `count` is 0, all matches are replaced. Otherwise, at most `count`
    /// replacements are made. The replacement string supports `$1`, `$2`, etc.
    /// for backreferences to captured groups.
    pub fn sub(&self, repl: &str, text: &str, count: usize, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        // Translate Python-style backreferences (\1, \2) to regex crate style ($1, $2)
        let rust_repl = translate_replacement(repl);
        let result = if count == 0 {
            self.compiled.replace_all(text, rust_repl.as_ref())
        } else {
            self.compiled.replacen(text, count, rust_repl.as_ref())
        };
        let s = Str::new(result.into_owned());
        Ok(Value::Ref(heap.allocate(HeapData::Str(s))?))
    }
}

impl PyTrait for RePattern {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::RePattern
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        None
    }

    fn py_eq(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<bool, ResourceError> {
        Ok(self.pattern == other.pattern && self.flags == other.flags)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No heap references — all data is owned.
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        // Pattern objects are always truthy (matching CPython).
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
        let escaped = escape_for_repr(&self.pattern);
        if self.flags == 0 {
            write!(f, "re.compile('{escaped}')")
        } else {
            let mut flag_parts = Vec::new();
            if self.flags & IGNORECASE != 0 {
                flag_parts.push("re.IGNORECASE");
            }
            if self.flags & MULTILINE != 0 {
                flag_parts.push("re.MULTILINE");
            }
            if self.flags & DOTALL != 0 {
                flag_parts.push("re.DOTALL");
            }
            write!(f, "re.compile('{escaped}', {})", flag_parts.join("|"))
        }
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.pattern.len()
    }

    fn py_getattr(
        &self,
        attr_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<super::AttrCallResult>> {
        match StaticStrings::from_string_id(attr_id) {
            Some(StaticStrings::PatternAttr) => {
                let s = Str::new(self.pattern.clone());
                let v = Value::Ref(heap.allocate(HeapData::Str(s))?);
                Ok(Some(super::AttrCallResult::Value(v)))
            }
            Some(StaticStrings::Flags) => Ok(Some(super::AttrCallResult::Value(Value::Int(i64::from(self.flags))))),
            _ => Err(ExcType::attribute_error(Type::RePattern, interns.get_str(attr_id))),
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
            Some(StaticStrings::Search) => {
                let arg = args.get_one_arg("Pattern.search", heap)?;
                defer_drop!(arg, heap);
                let text = value_to_str(arg, heap, interns)?.into_owned();
                self.search(&text, heap)
            }
            Some(StaticStrings::Match) => {
                let arg = args.get_one_arg("Pattern.match", heap)?;
                defer_drop!(arg, heap);
                let text = value_to_str(arg, heap, interns)?.into_owned();
                self.match_start(&text, heap)
            }
            Some(StaticStrings::Fullmatch) => {
                let arg = args.get_one_arg("Pattern.fullmatch", heap)?;
                defer_drop!(arg, heap);
                let text = value_to_str(arg, heap, interns)?.into_owned();
                self.fullmatch(&text, heap)
            }
            Some(StaticStrings::Findall) => {
                let arg = args.get_one_arg("Pattern.findall", heap)?;
                defer_drop!(arg, heap);
                let text = value_to_str(arg, heap, interns)?.into_owned();
                self.findall(&text, heap)
            }
            Some(StaticStrings::Sub) => call_pattern_sub(self, args, heap, interns),
            _ => Err(ExcType::attribute_error(Type::RePattern, attr.as_str(interns))),
        }
    }
}

/// Handles `pattern.sub(repl, string, count=0)` argument extraction and dispatch.
///
/// Separated from the main `py_call_attr` match to keep the borrow checker happy —
/// extracting multiple string arguments requires careful ordering of borrows.
fn call_pattern_sub(
    pattern: &RePattern,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let pos = args.into_pos_only("Pattern.sub", heap)?;
    defer_drop_mut!(pos, heap);

    let Some(repl_val) = pos.next() else {
        return Err(ExcType::type_error("Pattern.sub() missing required argument: 'repl'"));
    };
    defer_drop!(repl_val, heap);

    let Some(string_val) = pos.next() else {
        return Err(ExcType::type_error("Pattern.sub() missing required argument: 'string'"));
    };
    defer_drop!(string_val, heap);

    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "n is checked non-negative above"
    )]
    let count = match pos.next() {
        Some(Value::Int(n)) if n >= 0 => n as usize,
        Some(Value::Int(_)) => {
            return Err(ExcType::type_error("count must be a non-negative integer"));
        }
        Some(other) => {
            let t = other.py_type(heap);
            other.drop_with_heap(heap);
            return Err(ExcType::type_error(format!("expected int for count, not {t}")));
        }
        None => 0,
    };

    if pos.next().is_some() {
        return Err(ExcType::type_error(
            "Pattern.sub() takes at most 3 positional arguments",
        ));
    }

    let repl = value_to_str(repl_val, heap, interns)?.into_owned();
    let text = value_to_str(string_val, heap, interns)?.into_owned();
    pattern.sub(&repl, &text, count, heap)
}

/// Compiles a Python regex pattern string with flags into a Rust `Regex`.
///
/// Translates Python flag constants into inline regex flag prefixes:
/// - `re.IGNORECASE` (2) → `(?i)` prefix
/// - `re.MULTILINE` (8) → `(?m)` prefix
/// - `re.DOTALL` (16) → `(?s)` prefix
///
/// # Errors
///
/// Returns `re.PatternError(...)` if the pattern is invalid or uses features not supported
/// by the Rust regex engine (backreferences, lookahead, etc.).
pub(crate) fn compile_regex(pattern: &str, flags: u32) -> RunResult<Regex> {
    let mut prefix = String::new();
    if flags & IGNORECASE != 0 {
        prefix.push_str("(?i)");
    }
    if flags & MULTILINE != 0 {
        prefix.push_str("(?m)");
    }
    if flags & DOTALL != 0 {
        prefix.push_str("(?s)");
    }

    let full_pattern = if prefix.is_empty() {
        pattern.to_owned()
    } else {
        format!("{prefix}{pattern}")
    };

    Regex::new(&full_pattern).map_err(ExcType::re_pattern_error)
}

/// Escapes a string for inclusion in a `repr()` single-quoted string literal.
///
/// Doubles backslashes and escapes single quotes so the output is valid
/// Python source when wrapped in single quotes. Matches CPython's behavior
/// for `repr(re.compile(r'\d+'))` → `re.compile('\\d+')`.
fn escape_for_repr(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// Wraps a pattern to anchor at the start of the string (for `re.match()`).
///
/// Prepends `\A(?:...)` so the pattern only matches at position 0.
/// The non-capturing group `(?:...)` prevents altering the group numbering
/// of the original pattern.
fn anchor_start(pattern: &str) -> String {
    format!("\\A(?:{pattern})")
}

/// Wraps a pattern to anchor at both ends (for `re.fullmatch()`).
///
/// Wraps as `\A(?:...)\z` so the pattern must match the entire string.
fn anchor_full(pattern: &str) -> String {
    format!("\\A(?:{pattern})\\z")
}

/// Translates Python-style replacement backreferences to Rust regex syntax.
///
/// Python uses `\1`, `\2`, etc. for backreferences in replacement strings.
/// The Rust `regex` crate uses `$1`, `$2`, etc. This function converts
/// the Python style to Rust style.
///
/// Returns a `Cow` to avoid allocation when no translation is needed.
fn translate_replacement(repl: &str) -> Cow<'_, str> {
    if !repl.contains('\\') {
        return Cow::Borrowed(repl);
    }

    let mut result = String::with_capacity(repl.len());
    let mut chars = repl.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(&d) if d.is_ascii_digit() => {
                    result.push('$');
                    result.push(d);
                    chars.next();
                }
                Some(&'\\') => {
                    result.push('\\');
                    chars.next();
                }
                _ => {
                    result.push('\\');
                }
            }
        } else {
            result.push(c);
        }
    }

    Cow::Owned(result)
}

/// Extracts a string from a `Value`, supporting both interned and heap strings.
///
/// Returns a `Cow<str>` to avoid unnecessary copies for interned strings.
pub(crate) fn value_to_str<'a>(
    val: &'a Value,
    heap: &'a Heap<impl ResourceTracker>,
    interns: &'a Interns,
) -> RunResult<Cow<'a, str>> {
    match val {
        Value::InternString(string_id) => Ok(Cow::Borrowed(interns.get_str(*string_id))),
        Value::Ref(heap_id) => match heap.get(*heap_id) {
            HeapData::Str(s) => Ok(Cow::Borrowed(s.as_str())),
            other => Err(ExcType::type_error(format!(
                "expected string, not {}",
                other.py_type(heap)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "expected string, not {}",
            val.py_type(heap)
        ))),
    }
}

impl Serialize for RePattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialize only pattern string and flags; regex is recompiled on deserialize.
        (&self.pattern, self.flags).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RePattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (pattern, flags): (String, u32) = Deserialize::deserialize(deserializer)?;
        Self::compile(pattern, flags).map_err(|e| serde::de::Error::custom(format!("{e:?}")))
    }
}
