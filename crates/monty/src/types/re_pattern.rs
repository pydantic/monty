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

use std::{borrow::Cow, cmp::Ordering, fmt::Write, iter, mem, str};

use ahash::AHashSet;
use fancy_regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    modules::re::{ASCII, DOTALL, IGNORECASE, MULTILINE},
    resource::{ResourceTracker, check_estimated_size},
    types::{
        List, PyTrait, ReMatch, Type, allocate_tuple,
        str::{allocate_string, string_repr_fmt},
    },
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
#[derive(Debug, Clone)]
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

impl PartialEq for RePattern {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern && self.flags == other.flags
    }
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
    pub fn search(&self, text: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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
    pub fn match_start(&self, text: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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
    pub fn fullmatch(&self, text: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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
    pub fn findall(&self, text: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let cap_count = self.compiled.captures_len();
        let mut results = Vec::new();

        match cap_count {
            // No capture groups — return list of full match strings
            0 | 1 => {
                for m in self.compiled.find_iter(text) {
                    let val = m.map_err(ExcType::re_pattern_error)?.as_str();
                    results.push(allocate_string(val, heap)?);
                }
            }
            // One capture group — return list of the group's strings
            2 => {
                for caps in self.compiled.captures_iter(text) {
                    let caps = caps.map_err(ExcType::re_pattern_error)?;
                    let val = caps.get(1).map_or("", |m| m.as_str());
                    results.push(allocate_string(val, heap)?);
                }
            }
            // Multiple capture groups — return list of tuples
            _ => {
                for caps in self.compiled.captures_iter(text) {
                    let caps = caps.map_err(ExcType::re_pattern_error)?;
                    let mut elements: SmallVec<[Value; 3]> = SmallVec::with_capacity(cap_count - 1);
                    for cap in caps.iter().skip(1) {
                        let val = cap.map_or("", |m| m.as_str());
                        elements.push(allocate_string(val, heap)?);
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
    pub fn sub(&self, repl: &str, text: &str, count: usize, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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
        Ok(allocate_string(result, heap)?)
    }

    /// `pattern.split(string, maxsplit=0)` — split string by pattern occurrences.
    ///
    /// Returns a list of strings. If `maxsplit` is positive, at most `maxsplit`
    /// splits occur and the remainder of the string is returned as the final
    /// element; if it is negative, no splits occur at all (CPython's split loop
    /// runs zero times), returning the whole subject as a single element.
    pub fn split(&self, text: &str, maxsplit: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
        let pieces: Vec<&str> = match maxsplit.cmp(&0) {
            Ordering::Less => vec![text],
            Ordering::Equal => self
                .compiled
                .split(text)
                .collect::<Result<Vec<_>, _>>()
                .map_err(ExcType::re_pattern_error)?,
            Ordering::Greater => {
                // `maxsplit + 1` pieces = at most `maxsplit` splits; saturate
                // for absurdly large limits (splitn caps at the piece count).
                let limit = usize::try_from(maxsplit).unwrap_or(usize::MAX).saturating_add(1);
                self.compiled
                    .splitn(text, limit)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(ExcType::re_pattern_error)?
            }
        };

        let mut results = Vec::with_capacity(pieces.len());
        for piece in pieces {
            results.push(allocate_string(piece, heap)?);
        }

        let list = List::new(results);
        Ok(Value::Ref(heap.allocate(HeapData::List(list))?))
    }

    /// `pattern.finditer(string)` — return all matches as a list.
    ///
    /// Eagerly collects all match objects into a list. This differs from CPython's
    /// lazy iterator but produces the same results when iterated. The VM's `GetIter`
    /// opcode handles iteration over the returned list.
    pub fn finditer(&self, text: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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

impl<'h> PyTrait<'h> for HeapRead<'h, RePattern> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::RePattern
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::RePattern(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(self.get(vm.heap) == other.get(vm.heap)))
    }

    fn py_bool(&self, _vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        // Pattern objects are always truthy (matching CPython).
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let this = self.get(vm.heap);
        write!(f, "re.compile(")?;
        string_repr_fmt(&this.pattern, f)?;
        if this.flags != 0 {
            let mut flag_parts = smallvec::SmallVec::<[&'static str; 4]>::new();
            if this.flags & IGNORECASE != 0 {
                flag_parts.push("re.IGNORECASE");
            }
            if this.flags & MULTILINE != 0 {
                flag_parts.push("re.MULTILINE");
            }
            if this.flags & DOTALL != 0 {
                flag_parts.push("re.DOTALL");
            }
            if this.flags & ASCII != 0 {
                flag_parts.push("re.ASCII");
            }
            write!(f, ", {}", flag_parts.join("|"))?;
        }
        Ok(write!(f, ")")?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        match attr.static_string() {
            Some(StaticStrings::PatternAttr) => {
                let v = allocate_string(self.get(vm.heap).pattern.as_str(), vm.heap)?;
                Ok(Some(CallResult::Value(v)))
            }
            Some(StaticStrings::Flags) => Ok(Some(CallResult::Value(Value::Int(i64::from(self.get(vm.heap).flags))))),
            _ => Err(ExcType::attribute_error(Type::RePattern, attr.as_str(vm.interns))),
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
            Some(StaticStrings::Search) => {
                let arg = args.get_one_arg("Pattern.search", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?.to_owned();
                self.get(vm.heap).search(&text, vm.heap)
            }
            Some(StaticStrings::Match) => {
                let arg = args.get_one_arg("Pattern.match", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?.to_owned();
                self.get(vm.heap).match_start(&text, vm.heap)
            }
            Some(StaticStrings::Fullmatch) => {
                let arg = args.get_one_arg("Pattern.fullmatch", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?.to_owned();
                self.get(vm.heap).fullmatch(&text, vm.heap)
            }
            Some(StaticStrings::Findall) => {
                let arg = args.get_one_arg("Pattern.findall", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?.to_owned();
                self.get(vm.heap).findall(&text, vm.heap)
            }
            Some(StaticStrings::Sub) => call_pattern_sub(self, args, vm),
            Some(StaticStrings::Split) => call_pattern_split(self, args, vm),
            Some(StaticStrings::Finditer) => {
                let arg = args.get_one_arg("Pattern.finditer", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?.to_owned();
                self.get(vm.heap).finditer(&text, vm.heap)
            }
            _ => return Err(ExcType::attribute_error(Type::RePattern, attr.as_str(vm.interns))),
        }?;
        Ok(CallResult::Value(result))
    }
}

impl HeapItem for RePattern {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.pattern.len()
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
fn call_pattern_sub<'h>(
    pattern: &HeapRead<'h, RePattern>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let PatternSubArgs {
        repl: repl_val,
        string: string_val,
        count: count_val,
    } = PatternSubArgs::from_args(args, vm)?;
    defer_drop!(repl_val, vm);
    defer_drop!(string_val, vm);

    let count = extract_count(count_val, vm)?;

    // Check that repl is a string — callable replacement is not supported.
    // CPython processes the replacement template *before* its match loop, so
    // this check must precede the negative-count early return below: a bad
    // repl raises even when zero substitutions will run.
    if !repl_val.is_str(vm.heap) {
        return Err(ExcType::type_error(
            "callable replacement is not yet supported in re.sub()",
        ));
    }

    let Some(count) = count else {
        // Negative count — Pattern.sub returns the input string unchanged.
        // The subject is still type-checked (`to_str` raises this method's
        // `expected string, not {t}` wording) before the refcount bump; no
        // need to re-allocate.
        let _ = string_val.to_str(vm)?;
        return Ok(string_val.clone_with_heap(vm.heap));
    };

    let repl = repl_val.to_str(vm)?.to_owned();
    let text = string_val.to_str(vm)?.to_owned();
    pattern.get(vm.heap).sub(&repl, &text, count, vm.heap)
}

/// Handles `pattern.split(string, maxsplit=0)` argument extraction and dispatch.
///
/// Supports `maxsplit` as either positional or keyword argument.
fn call_pattern_split<'h>(
    pattern: &HeapRead<'h, RePattern>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let PatternSplitArgs {
        string: string_val,
        maxsplit: maxsplit_val,
    } = PatternSplitArgs::from_args(args, vm)?;
    defer_drop!(string_val, vm);

    let maxsplit = extract_maxsplit(maxsplit_val, vm)?;
    let text = string_val.to_str(vm)?.to_owned();
    pattern.get(vm.heap).split(&text, maxsplit, vm.heap)
}

/// Argument shape for `Pattern.sub(repl, string, count=0)`.
///
/// `string` uses `static_string = "StringAttr"` because `StringAttr` is the
/// `StaticStrings` entry that interns `"string"` (the bare `String` variant
/// is taken by the `re.Pattern.string` attribute name in CPython's class
/// hierarchy).
#[derive(FromArgs)]
#[from_args(name = "sub", style = c_named, at_most_total)]
struct PatternSubArgs {
    repl: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default)]
    count: Option<Value>,
}

/// Argument shape for `Pattern.split(string, maxsplit=0)`.
///
/// See `PatternSubArgs` for why `string` uses `static_string`.
#[derive(FromArgs)]
#[from_args(name = "split", style = c_named, at_most_total)]
struct PatternSplitArgs {
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default)]
    maxsplit: Option<Value>,
}

/// Extracts a `maxsplit` value from an optional `Value` for [`RePattern::split`].
///
/// Returns 0 (split all) if not provided; negatives pass through — the split
/// loop then runs zero times, matching CPython. Non-ints get CPython's
/// argument-clinic message. Shared by `Pattern.split` and module-level
/// `re.split`.
pub(crate) fn extract_maxsplit(val: Option<Value>, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<i64> {
    match val {
        None => Ok(0),
        Some(Value::Int(n)) => Ok(n),
        Some(Value::Bool(b)) => Ok(i64::from(b)),
        Some(other) => {
            let t = other.py_type(vm);
            other.drop_with_heap(vm);
            Err(ExcType::type_error(format!(
                "'{t}' object cannot be interpreted as an integer"
            )))
        }
    }
}

/// Extracts a `count` value from an optional `Value` for [`RePattern::sub`].
///
/// Returns `Ok(None)` for a negative count, which callers turn into "return
/// the subject unchanged" (CPython's match loop runs zero times there).
/// Non-ints get CPython's argument-clinic message. Shared by `Pattern.sub`
/// and module-level `re.sub`.
pub(crate) fn extract_count(val: Option<Value>, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<usize>> {
    match val {
        None => Ok(Some(0)),
        // Saturate rather than `as`-cast: on 32-bit targets (wasm) a count
        // above usize::MAX would otherwise truncate — e.g. 2**32 to 0, which
        // means "replace all" instead of an unreachably large cap.
        Some(Value::Int(n)) if n >= 0 => Ok(Some(usize::try_from(n).unwrap_or(usize::MAX))),
        Some(Value::Bool(b)) => Ok(Some(usize::from(b))),
        Some(Value::Int(_)) => Ok(None),
        Some(other) => {
            let t = other.py_type(vm);
            other.drop_with_heap(vm);
            Err(ExcType::type_error(format!(
                "'{t}' object cannot be interpreted as an integer"
            )))
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
fn translate_g_backref(chars: &mut iter::Peekable<str::Chars<'_>>, result: &mut String) {
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

impl Serialize for RePattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialize only pattern string and flags; regex is recompiled on deserialize.
        (&self.pattern, self.flags).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RePattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (pattern, flags): (String, u16) = Deserialize::deserialize(deserializer)?;
        Self::compile(pattern, flags).map_err(|e| de::Error::custom(format!("{e:?}")))
    }
}
