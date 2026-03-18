//! Compiled regex pattern type for the `re` module.
//!
//! `RePattern` wraps a compiled `fancy_regex::Regex` with the original Python pattern
//! string and flags. The `fancy_regex` crate supports backreferences, lookahead/lookbehind,
//! and other advanced features, but uses backtracking which means patterns are susceptible
//! to ReDoS. Monty's resource limits (time and allocation budgets) are the primary defense
//! against catastrophic backtracking in untrusted patterns.
//!
//! Custom serde serializes only the pattern string and flags, recompiling the regex
//! on deserialization. This supports Monty's snapshot/restore feature.

use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;
use fancy_regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smallvec::SmallVec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId, HeapItem},
    intern::{Interns, StaticStrings},
    modules::re::{ASCII, DOTALL, IGNORECASE, MULTILINE},
    resource::{ResourceError, ResourceTracker, check_estimated_size},
    types::{List, PyTrait, ReMatch, Str, Type, allocate_tuple, str::string_repr_fmt},
    value::{EitherStr, Value},
};

/// A compiled regular expression pattern.
///
/// Wraps a `fancy_regex::Regex` with the original Python pattern string and flags.
/// The `fancy_regex` crate supports backtracking features like backreferences and
/// lookaround, but this means patterns are susceptible to ReDoS — Monty's resource
/// limits are the defense against catastrophic backtracking.
///
/// Custom serde serializes only the pattern string and flags, recompiling the
/// regex on deserialization. This supports Monty's snapshot/restore feature.
#[derive(Debug)]
pub(crate) struct RePattern {
    /// The original Python regex pattern string.
    pattern: String,
    /// Python regex flags bitmask (IGNORECASE=2, MULTILINE=8, DOTALL=16, ASCII=256).
    flags: u16,
    /// The compiled Rust regex, unanchored.
    compiled: Regex,
    /// The compiled regex anchored with `\A(?:...)` for `match()`.
    ///
    /// Uses `\A` (absolute start anchor) instead of `^` so the MULTILINE flag
    /// doesn't cause it to match at line boundaries. This correctly handles
    /// alternations — e.g. `match('b|ab', 'ab')` must match `ab`, not fail
    /// because the engine found only `b` starting at position 1.
    compiled_match: Regex,
    /// The compiled regex anchored with `\A(?:...)\z` for `fullmatch()`.
    ///
    /// Uses `\A`/`\z` (absolute anchors) instead of `^`/`$` so the MULTILINE flag
    /// doesn't cause them to match at line boundaries. This correctly handles
    /// alternations — e.g. `fullmatch('a|ab', 'ab')` must match `ab`, not fail
    /// because the engine found `a` first.
    compiled_fullmatch: Regex,
}

impl RePattern {
    /// Creates a compiled pattern from a Python regex string and flags.
    ///
    /// Translates Python flag constants into inline regex flag prefixes and compiles
    /// the pattern. Also pre-compiles anchored variants for `match` (`\A(?:pattern)`)
    /// and `fullmatch` (`\A(?:pattern)\z`) to correctly handle alternations.
    ///
    /// # Errors
    ///
    /// Returns `re.PatternError` if the pattern is invalid.
    pub fn compile(pattern: String, flags: u16) -> RunResult<Self> {
        let compiled = compile_regex(&pattern, flags)?;
        let compiled_match = compile_regex(&format!("\\A(?:{pattern})"), flags)?;
        let compiled_fullmatch = compile_regex(&format!("\\A(?:{pattern})\\z"), flags)?;
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
            Ok(Some(caps)) => {
                let m = ReMatch::from_captures(&caps, text, &self.pattern, &self.compiled);
                Ok(Value::Ref(heap.allocate(HeapData::ReMatch(m))?))
            }
            Ok(None) => Ok(Value::None),
            Err(err) => Err(ExcType::re_pattern_error(err)),
        }
    }

    /// `pattern.match(string)` — match anchored at the start of the string.
    ///
    /// Uses a pre-compiled `\A(?:pattern)` regex to correctly handle alternations.
    /// For example, `match('b|ab', 'ab')` correctly matches `ab` because the
    /// anchor forces the engine to try all alternatives at position 0.
    ///
    /// Returns a `ReMatch` heap object on success, or `Value::None` if no match.
    pub fn match_start(&self, text: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match self.compiled_match.captures(text) {
            Ok(Some(caps)) => {
                let match_obj = ReMatch::from_captures(&caps, text, &self.pattern, &self.compiled);
                Ok(Value::Ref(heap.allocate(HeapData::ReMatch(match_obj))?))
            }
            Ok(None) => Ok(Value::None),
            Err(err) => Err(ExcType::re_pattern_error(err)),
        }
    }

    /// `pattern.fullmatch(string)` — match the entire string.
    ///
    /// Uses a pre-compiled `\A(?:pattern)\z` regex to correctly handle alternations.
    /// For example, `fullmatch('a|ab', 'ab')` correctly matches `ab` because the
    /// anchors force the engine to try all alternatives for a full-string match.
    ///
    /// Returns a `ReMatch` heap object on success, or `Value::None` if no match.
    pub fn fullmatch(&self, text: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match self.compiled_fullmatch.captures(text) {
            Ok(Some(caps)) => {
                let match_obj = ReMatch::from_captures(&caps, text, &self.pattern, &self.compiled);
                Ok(Value::Ref(heap.allocate(HeapData::ReMatch(match_obj))?))
            }
            Ok(None) => Ok(Value::None),
            Err(err) => Err(ExcType::re_pattern_error(err)),
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

        match cap_count {
            // No capture groups — return list of full match strings
            0 | 1 => {
                for m in self.compiled.find_iter(text) {
                    let s = Str::new(m.map_err(ExcType::re_pattern_error)?.as_str().to_owned());
                    results.push(Value::Ref(heap.allocate(HeapData::Str(s))?));
                }
            }
            // One capture group — return list of the group's strings
            2 => {
                for caps in self.compiled.captures_iter(text) {
                    let caps = caps.map_err(ExcType::re_pattern_error)?;
                    let val = caps.get(1).map(|m| m.as_str().to_owned()).unwrap_or_default();
                    let s = Str::new(val);
                    results.push(Value::Ref(heap.allocate(HeapData::Str(s))?));
                }
            }
            // Multiple capture groups — return list of tuples
            _ => {
                for caps in self.compiled.captures_iter(text) {
                    let caps = caps.map_err(ExcType::re_pattern_error)?;
                    let mut elements: SmallVec<[Value; 3]> = SmallVec::with_capacity(cap_count - 1);
                    for cap in caps.iter().skip(1) {
                        let val = cap.map(|m| m.as_str().to_owned()).unwrap_or_default();
                        let s = Str::new(val);
                        elements.push(Value::Ref(heap.allocate(HeapData::Str(s))?));
                    }
                    results.push(allocate_tuple(elements, heap)?);
                }
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
    ///
    /// Builds the result string in a single pass by iterating matches and appending
    /// replacements directly. Checks the running output size against resource limits
    /// after each match, bailing out immediately if the budget is exceeded. This
    /// avoids both false rejections from conservative pre-estimates and untracked
    /// Rust heap allocations from delegating to `fancy_regex::replace_all()`.
    pub fn sub(&self, repl: &str, text: &str, count: usize, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        // Translate Python-style backreferences (\1, \2) to regex crate style ($1, $2)
        let rust_repl = translate_replacement(repl);
        let effective_count = if count == 0 { usize::MAX } else { count };

        let mut result = String::new();
        let mut last_end = 0;

        for caps in self.compiled.captures_iter(text).take(effective_count) {
            let caps = caps.map_err(ExcType::re_pattern_error)?;
            let m = caps.get(0).expect("capture group 0 always exists");
            result.push_str(&text[last_end..m.start()]);
            caps.expand(rust_repl.as_ref(), &mut result);
            last_end = m.end();
            // Check running size: current result + remaining unprocessed text.
            check_estimated_size(result.len() + (text.len() - last_end), heap.tracker())?;
        }

        result.push_str(&text[last_end..]);
        let s = Str::new(result);
        Ok(Value::Ref(heap.allocate(HeapData::Str(s))?))
    }

    /// `pattern.split(string, maxsplit=0)` — split string by pattern occurrences.
    ///
    /// Returns a list of strings. If `maxsplit` is non-zero, at most `maxsplit`
    /// splits occur and the remainder of the string is returned as the final element.
    pub fn split(&self, text: &str, maxsplit: usize, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        let pieces: Vec<&str> = if maxsplit == 0 {
            self.compiled
                .split(text)
                .collect::<Result<Vec<_>, _>>()
                .map_err(ExcType::re_pattern_error)?
        } else {
            self.compiled
                .splitn(text, maxsplit + 1)
                .collect::<Result<Vec<_>, _>>()
                .map_err(ExcType::re_pattern_error)?
        };

        let mut results = Vec::with_capacity(pieces.len());
        for piece in pieces {
            let s = Str::new(piece.to_owned());
            results.push(Value::Ref(heap.allocate(HeapData::Str(s))?));
        }

        let list = List::new(results);
        Ok(Value::Ref(heap.allocate(HeapData::List(list))?))
    }

    /// `pattern.finditer(string)` — return all matches as a list.
    ///
    /// Eagerly collects all match objects into a list. This differs from CPython's
    /// lazy iterator but produces the same results when iterated. The VM's `GetIter`
    /// opcode handles iteration over the returned list.
    pub fn finditer(&self, text: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        let mut results = Vec::new();
        for caps in self.compiled.captures_iter(text) {
            let caps = caps.map_err(ExcType::re_pattern_error)?;
            let m = ReMatch::from_captures(&caps, text, &self.pattern, &self.compiled);
            results.push(Value::Ref(heap.allocate(HeapData::ReMatch(m))?));
        }

        let list = List::new(results);
        Ok(Value::Ref(heap.allocate(HeapData::List(list))?))
    }
}

impl PyTrait for RePattern {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::RePattern
    }

    fn py_len(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, _vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Ok(self.pattern == other.pattern && self.flags == other.flags)
    }

    fn py_bool(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
        // Pattern objects are always truthy (matching CPython).
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _vm: &VM<'_, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        write!(f, "re.compile(")?;
        string_repr_fmt(&self.pattern, f)?;
        if self.flags != 0 {
            let mut flag_parts = smallvec::SmallVec::<[&'static str; 4]>::new();
            if self.flags & IGNORECASE != 0 {
                flag_parts.push("re.IGNORECASE");
            }
            if self.flags & MULTILINE != 0 {
                flag_parts.push("re.MULTILINE");
            }
            if self.flags & DOTALL != 0 {
                flag_parts.push("re.DOTALL");
            }
            if self.flags & ASCII != 0 {
                flag_parts.push("re.ASCII");
            }
            write!(f, ", {}", flag_parts.join("|"))?;
        }
        write!(f, ")")
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        match attr.static_string() {
            Some(StaticStrings::PatternAttr) => {
                let s = Str::new(self.pattern.clone());
                let v = Value::Ref(vm.heap.allocate(HeapData::Str(s))?);
                Ok(Some(CallResult::Value(v)))
            }
            Some(StaticStrings::Flags) => Ok(Some(CallResult::Value(Value::Int(i64::from(self.flags))))),
            _ => Err(ExcType::attribute_error(Type::RePattern, attr.as_str(vm.interns))),
        }
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let result = match attr.static_string() {
            Some(StaticStrings::Search) => {
                let arg = args.get_one_arg("Pattern.search", vm.heap)?;
                defer_drop!(arg, vm);
                let text = value_to_str(arg, vm.heap, vm.interns)?.into_owned();
                self.search(&text, vm.heap)
            }
            Some(StaticStrings::Match) => {
                let arg = args.get_one_arg("Pattern.match", vm.heap)?;
                defer_drop!(arg, vm);
                let text = value_to_str(arg, vm.heap, vm.interns)?.into_owned();
                self.match_start(&text, vm.heap)
            }
            Some(StaticStrings::Fullmatch) => {
                let arg = args.get_one_arg("Pattern.fullmatch", vm.heap)?;
                defer_drop!(arg, vm);
                let text = value_to_str(arg, vm.heap, vm.interns)?.into_owned();
                self.fullmatch(&text, vm.heap)
            }
            Some(StaticStrings::Findall) => {
                let arg = args.get_one_arg("Pattern.findall", vm.heap)?;
                defer_drop!(arg, vm);
                let text = value_to_str(arg, vm.heap, vm.interns)?.into_owned();
                self.findall(&text, vm.heap)
            }
            Some(StaticStrings::Sub) => call_pattern_sub(self, args, vm.heap, vm.interns),
            Some(StaticStrings::Split) => call_pattern_split(self, args, vm.heap, vm.interns),
            Some(StaticStrings::Finditer) => {
                let arg = args.get_one_arg("Pattern.finditer", vm.heap)?;
                defer_drop!(arg, vm);
                let text = value_to_str(arg, vm.heap, vm.interns)?.into_owned();
                self.finditer(&text, vm.heap)
            }
            _ => return Err(ExcType::attribute_error(Type::RePattern, attr.as_str(vm.interns))),
        }?;
        Ok(CallResult::Value(result))
    }
}

impl HeapItem for RePattern {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.pattern.len()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No heap references — all data is owned.
    }
}

/// Handles `pattern.sub(repl, string, count=0)` argument extraction and dispatch.
///
/// Separated from the main `py_call_attr` match to keep the borrow checker happy —
/// extracting multiple string arguments requires careful ordering of borrows.
/// Supports `count` as either positional or keyword argument.
fn call_pattern_sub(
    pattern: &RePattern,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let Some(repl_val) = pos.next() else {
        return Err(ExcType::type_error("Pattern.sub() missing required argument: 'repl'"));
    };
    defer_drop!(repl_val, heap);

    let Some(string_val) = pos.next() else {
        return Err(ExcType::type_error("Pattern.sub() missing required argument: 'string'"));
    };
    defer_drop!(string_val, heap);

    let pos_count = pos.next();

    if let Some(extra) = pos.next() {
        extra.drop_with_heap(heap);
        return Err(ExcType::type_error(
            "Pattern.sub() takes at most 3 positional arguments",
        ));
    }

    // Extract count from kwargs if not given positionally
    let mut kw_count: Option<Value> = None;
    for (key, value) in kwargs {
        defer_drop!(key, heap);
        let Some(keyword_name) = key.as_either_str(heap) else {
            value.drop_with_heap(heap);
            return Err(ExcType::type_error("keywords must be strings"));
        };
        let key_str = keyword_name.as_str(interns);
        if key_str == "count" {
            if pos_count.is_some() {
                value.drop_with_heap(heap);
                return Err(ExcType::type_error(
                    "Pattern.sub() got multiple values for argument 'count'",
                ));
            }
            kw_count.replace(value).drop_with_heap(heap);
        } else {
            value.drop_with_heap(heap);
            return Err(ExcType::type_error(format!(
                "'{key_str}' is an invalid keyword argument for Pattern.sub()"
            )));
        }
    }

    let count_val = pos_count.or(kw_count);

    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "n is checked non-negative above"
    )]
    let count = match count_val {
        Some(Value::Int(n)) if n >= 0 => n as usize,
        Some(Value::Bool(b)) => usize::from(b),
        Some(Value::Int(_)) => {
            let text = value_to_str(string_val, heap, interns)?.into_owned();
            let s = Str::new(text);
            return Ok(Value::Ref(heap.allocate(HeapData::Str(s))?));
        }
        Some(other) => {
            let t = other.py_type(heap);
            other.drop_with_heap(heap);
            return Err(ExcType::type_error(format!("expected int for count, not {t}")));
        }
        None => 0,
    };

    // Check that repl is a string — callable replacement is not supported
    if !repl_val.is_str(heap) {
        return Err(ExcType::type_error(
            "callable replacement is not yet supported in re.sub()",
        ));
    }
    let repl = value_to_str(repl_val, heap, interns)?.into_owned();
    let text = value_to_str(string_val, heap, interns)?.into_owned();
    pattern.sub(&repl, &text, count, heap)
}

/// Handles `pattern.split(string, maxsplit=0)` argument extraction and dispatch.
///
/// Supports `maxsplit` as either positional or keyword argument.
fn call_pattern_split(
    pattern: &RePattern,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let Some(string_val) = pos.next() else {
        return Err(ExcType::type_error(
            "Pattern.split() missing required argument: 'string'",
        ));
    };
    defer_drop!(string_val, heap);

    let pos_maxsplit = pos.next();

    if let Some(extra) = pos.next() {
        extra.drop_with_heap(heap);
        return Err(ExcType::type_error(
            "Pattern.split() takes at most 2 positional arguments",
        ));
    }

    let mut kw_maxsplit: Option<Value> = None;
    for (key, value) in kwargs {
        defer_drop!(key, heap);
        let Some(keyword_name) = key.as_either_str(heap) else {
            value.drop_with_heap(heap);
            return Err(ExcType::type_error("keywords must be strings"));
        };
        let key_str = keyword_name.as_str(interns);
        if key_str == "maxsplit" {
            if pos_maxsplit.is_some() {
                value.drop_with_heap(heap);
                return Err(ExcType::type_error(
                    "Pattern.split() got multiple values for argument 'maxsplit'",
                ));
            }
            kw_maxsplit.replace(value).drop_with_heap(heap);
        } else {
            value.drop_with_heap(heap);
            return Err(ExcType::type_error(format!(
                "'{key_str}' is an invalid keyword argument for Pattern.split()"
            )));
        }
    }

    let maxsplit = extract_maxsplit(pos_maxsplit.or(kw_maxsplit), heap)?;
    let text = value_to_str(string_val, heap, interns)?.into_owned();
    pattern.split(&text, maxsplit, heap)
}

/// Extracts a `maxsplit` value from an optional `Value`.
///
/// Returns 0 if not provided. Negative values are treated as 0 (split all).
fn extract_maxsplit(val: Option<Value>, heap: &mut Heap<impl ResourceTracker>) -> RunResult<usize> {
    match val {
        None => Ok(0),
        Some(Value::Int(n)) if n <= 0 => Ok(0),
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "n is checked positive above"
        )]
        Some(Value::Int(n)) => Ok(n as usize),
        Some(Value::Bool(b)) => Ok(usize::from(b)),
        Some(other) => {
            let t = other.py_type(heap);
            other.drop_with_heap(heap);
            Err(ExcType::type_error(format!("expected int for maxsplit, not {t}")))
        }
    }
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
/// Returns `re.PatternError(...)` if the pattern is invalid.
pub(crate) fn compile_regex(pattern: &str, flags: u16) -> RunResult<Regex> {
    let mut prefix = String::new();
    if flags & IGNORECASE != 0 {
        prefix.push('i');
    }
    if flags & MULTILINE != 0 {
        prefix.push('m');
    }
    if flags & DOTALL != 0 {
        prefix.push('s');
    }
    // Note: re.ASCII (256) is accepted but has no effect on the regex compilation.
    // `fancy_regex` doesn't support `(?-u)` to disable Unicode mode, so `\w`, `\d`, `\s`
    // always match Unicode characters. This is a known limitation — Python 3 defaults to
    // Unicode mode anyway, so the behavioral difference only matters for non-ASCII input.

    let full_pattern = if prefix.is_empty() {
        pattern.to_owned()
    } else {
        format!("(?{prefix}){pattern}")
    };

    Regex::new(&full_pattern).map_err(ExcType::re_pattern_error)
}

/// Translates Python-style replacement backreferences to `fancy_regex` syntax.
///
/// Python uses `\1`, `\2`, `\g<1>`, `\g<name>` for backreferences in replacement strings.
/// `fancy_regex` uses `$1`, `$2`, `${1}`, `${name}`. This function converts between them.
///
/// # Supported translations
///
/// - `\1`–`\9` → `$1`–`$9` (single-digit backreferences)
/// - `\g<N>` → `${N}` (numeric backreference with explicit syntax)
/// - `\g<name>` → `${name}` (named group backreference)
/// - `\\` → literal backslash
/// - `$` → `$$` (escape literal `$` so `fancy_regex` doesn't misinterpret it)
///
/// Returns a `Cow` to avoid allocation when no translation is needed.
///
/// # Limitations
///
/// TODO: Multi-digit backreferences like `\10` are not fully supported. CPython
/// greedily reads all digits after `\` and interprets them as a group number if
/// that group exists, otherwise falls back to octal escapes. Currently `\10` is
/// translated as `$1` followed by literal `0`, which is wrong when 10+ groups
/// exist. Fixing this requires passing the pattern's capture group count into
/// this function to disambiguate.
fn translate_replacement(repl: &str) -> Cow<'_, str> {
    // Fast path: no backslashes and no literal `$` means nothing to translate or escape.
    if !repl.contains('\\') && !repl.contains('$') {
        return Cow::Borrowed(repl);
    }

    let mut result = String::with_capacity(repl.len());
    let mut chars = repl.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(&d) if d.is_ascii_digit() => {
                    // TODO: This only handles single-digit backrefs (\1–\9).
                    // Multi-digit like \10 should be ${10} when group 10 exists,
                    // but that requires knowing the group count. See docstring.
                    result.push('$');
                    result.push(d);
                    chars.next();
                }
                Some(&'g') => {
                    chars.next(); // consume 'g'
                    translate_g_backref(&mut chars, &mut result);
                }
                Some(&'\\') => {
                    result.push('\\');
                    chars.next();
                }
                _ => {
                    result.push('\\');
                }
            }
        } else if c == '$' {
            // Escape literal `$` as `$$` so `fancy_regex` doesn't interpret `$1` etc.
            // as backreferences.
            result.push('$');
            result.push('$');
        } else {
            result.push(c);
        }
    }

    Cow::Owned(result)
}

/// Translates a `\g<...>` backreference to `fancy_regex` `${...}` syntax.
///
/// Called after `\g` has been consumed. Reads `<name_or_number>` from the iterator
/// and writes `${name_or_number}` to the result. If the syntax is malformed
/// (missing `<` or `>`), the literal characters are written through unchanged.
fn translate_g_backref(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, result: &mut String) {
    if chars.peek() != Some(&'<') {
        // Not \g<...>, just literal \g
        result.push('\\');
        result.push('g');
        return;
    }
    chars.next(); // consume '<'

    // Collect everything until '>'
    let mut name = String::new();
    loop {
        match chars.next() {
            Some('>') => break,
            Some(ch) => name.push(ch),
            None => {
                // Unterminated \g<... — emit literally
                result.push('\\');
                result.push('g');
                result.push('<');
                result.push_str(&name);
                return;
            }
        }
    }

    // Write as ${name_or_number} for fancy_regex
    result.push('$');
    result.push('{');
    result.push_str(&name);
    result.push('}');
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
        let (pattern, flags): (String, u16) = Deserialize::deserialize(deserializer)?;
        Self::compile(pattern, flags).map_err(|e| serde::de::Error::custom(format!("{e:?}")))
    }
}
