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

use std::{borrow::Cow, cell::OnceCell, cmp::Ordering, fmt::Write, iter, mem, str};

use fancy_regex::{CompileError, Error as RegexError, Regex, RegexBuilder};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    modules::re::{ASCII, DOTALL, IGNORECASE, MULTILINE},
    resource_checks::check_estimated_size,
    types::{
        LazyHeapSet, List, PyTrait, ReMatch, Type, allocate_tuple,
        str::{allocate_string, string_repr_fmt},
        tuple::TupleVec,
    },
    value::{EitherStr, VALUE_SIZE, Value},
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
    /// The regex anchored with `\A(?:...)` for `match()`, compiled lazily on first
    /// use (most patterns are only ever `search`/`split`/`sub`ed).
    ///
    /// Uses `\A` (absolute start anchor) instead of `^` so the MULTILINE flag
    /// doesn't cause it to match at line boundaries. This correctly handles
    /// alternations — e.g. `match('b|ab', 'ab')` must match `ab`, not fail
    /// because the engine found only `b` starting at position 1.
    compiled_match: OnceCell<Regex>,
    /// The regex anchored with `\A(?:...)\z` for `fullmatch()`, compiled lazily on
    /// first use (see `compiled_match`).
    ///
    /// Uses `\A`/`\z` (absolute anchors) instead of `^`/`$` so the MULTILINE flag
    /// doesn't cause them to match at line boundaries. This correctly handles
    /// alternations — e.g. `fullmatch('a|ab', 'ab')` must match `ab`, not fail
    /// because the engine found `a` first.
    compiled_fullmatch: OnceCell<Regex>,
    /// The `delegate_size_limit` the plain regex was compiled with, forwarded to
    /// the anchored variants above so a *cached* entry's total retained compiled
    /// size stays bounded (the anchors add only O(1) bytes, so a pattern that fit
    /// unanchored still fits anchored). `None` = the engine's default limit. Not
    /// serialized — restored patterns recompile at the default limit.
    delegate_size_limit: Option<usize>,
}

impl PartialEq for RePattern {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern && self.flags == other.flags
    }
}

/// Failure of [`RePattern::compile_bounded`], separating "valid pattern whose
/// compiled form exceeds the size cap" (the caller retries uncached at default
/// limits) from a genuine pattern error (reported to the user).
pub(crate) enum BoundedCompileError {
    /// The compiled regex exceeded the requested `delegate_size_limit`.
    TooBig,
    /// The pattern itself is invalid, already converted to `re.PatternError`.
    Invalid(RunError),
}

impl RePattern {
    /// Creates a compiled pattern from a Python regex string and flags.
    ///
    /// Translates Python flag constants into inline regex flag prefixes and compiles
    /// the unanchored pattern. The anchored variants used by `match`/`fullmatch` are
    /// compiled lazily on first use (see [`RePattern::match_regex`]).
    ///
    /// # Errors
    ///
    /// Returns `re.PatternError` if the pattern is invalid.
    pub fn compile(pattern: String, flags: u16) -> RunResult<Self> {
        Self::compile_inner(pattern, flags, None).map_err(ExcType::re_pattern_error)
    }

    /// As [`RePattern::compile`], but caps the compiled size of the delegated regex
    /// (`RegexBuilder::delegate_size_limit`) so the `re` module's pattern cache can
    /// retain entries with a hard per-entry memory ceiling. The limit is retained
    /// and applied to the lazily-compiled anchored `match`/`fullmatch` variants
    /// too, so a cached entry cannot pin large regexes via `.match()`/`.fullmatch()`.
    pub(crate) fn compile_bounded(
        pattern: String,
        flags: u16,
        delegate_size_limit: usize,
    ) -> Result<Self, BoundedCompileError> {
        Self::compile_inner(pattern, flags, Some(delegate_size_limit)).map_err(|err| {
            if is_size_limit_error(&err) {
                BoundedCompileError::TooBig
            } else {
                BoundedCompileError::Invalid(ExcType::re_pattern_error(err))
            }
        })
    }

    /// True when this was compiled from exactly `(pattern, flags)`.
    ///
    /// Lets the `re` module's pattern cache confirm a slot on a hash hit without
    /// storing its own copy of the key — the compiled pattern already owns both,
    /// and duplicating the source text is what makes a cached entry expensive.
    pub(crate) fn is_compiled_from(&self, pattern: &str, flags: u16) -> bool {
        self.flags == flags && self.pattern == pattern
    }

    /// Shared constructor for [`RePattern::compile`] / [`RePattern::compile_bounded`].
    fn compile_inner(pattern: String, flags: u16, delegate_size_limit: Option<usize>) -> Result<Self, RegexError> {
        let compiled = compile_regex_limited(&pattern, flags, delegate_size_limit)?;
        Ok(Self {
            pattern,
            flags,
            compiled,
            compiled_match: OnceCell::new(),
            compiled_fullmatch: OnceCell::new(),
            delegate_size_limit,
        })
    }

    /// Returns the `\A(?:pattern)` regex for `match()`, compiling it on first use.
    ///
    /// Wrapping a pattern that already compiled essentially never fails, so any
    /// error surfaces (as `re.PatternError`) at `match()` rather than `re.compile()`.
    fn match_regex(&self) -> RunResult<&Regex> {
        if let Some(regex) = self.compiled_match.get() {
            return Ok(regex);
        }
        let compiled = compile_regex_limited(
            &format!("\\A(?:{})", self.pattern),
            self.flags,
            self.delegate_size_limit,
        )
        .map_err(ExcType::re_pattern_error)?;
        // `set` only fails on a concurrent init, impossible on the single-threaded VM.
        let _ = self.compiled_match.set(compiled);
        Ok(self.compiled_match.get().expect("cell was just initialised"))
    }

    /// Returns the `\A(?:pattern)\z` regex for `fullmatch()`, compiling on first use.
    fn fullmatch_regex(&self) -> RunResult<&Regex> {
        if let Some(regex) = self.compiled_fullmatch.get() {
            return Ok(regex);
        }
        let compiled = compile_regex_limited(
            &format!("\\A(?:{})\\z", self.pattern),
            self.flags,
            self.delegate_size_limit,
        )
        .map_err(ExcType::re_pattern_error)?;
        let _ = self.compiled_fullmatch.set(compiled);
        Ok(self.compiled_fullmatch.get().expect("cell was just initialised"))
    }

    /// Builds a single `ReMatch` heap value from a capture result, keeping the
    /// subject alive by refcount (`subject.clone_with_heap`) rather than copying
    /// its text. `all_ascii` is precomputed by the caller (once per `finditer`).
    fn build_match(&self, caps: &fancy_regex::Captures<'_>, subject: &Value, all_ascii: bool, heap: &Heap) -> Value {
        let m = ReMatch::from_captures(caps, subject.clone_with_heap(heap), all_ascii, &self.compiled);
        Value::Ref(heap.allocate(HeapData::ReMatch(Box::new(m))))
    }

    /// `pattern.search(string)` — find first match anywhere in the string.
    ///
    /// `subject` is the subject `Value` (stored by the match); `text` is its
    /// borrowed contents. Returns a `ReMatch` heap object, or `Value::None`.
    pub fn search(&self, subject: &Value, text: &str, heap: &Heap) -> RunResult<Value> {
        match self.compiled.captures(text) {
            Ok(Some(caps)) => Ok(self.build_match(&caps, subject, text.is_ascii(), heap)),
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
    pub fn match_start(&self, subject: &Value, text: &str, heap: &Heap) -> RunResult<Value> {
        match self.match_regex()?.captures(text) {
            Ok(Some(caps)) => Ok(self.build_match(&caps, subject, text.is_ascii(), heap)),
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
    pub fn fullmatch(&self, subject: &Value, text: &str, heap: &Heap) -> RunResult<Value> {
        match self.fullmatch_regex()?.captures(text) {
            Ok(Some(caps)) => Ok(self.build_match(&caps, subject, text.is_ascii(), heap)),
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
    ///
    /// The scan collects borrowed `&str` slices and a second pass allocates them,
    /// so only the scan can fail: a scan abandoned half way through a list of heap
    /// values could not release them, since `dec_ref` needs `&mut Heap` while the
    /// match iterator borrows the compiled pattern out of it.
    pub fn findall(&self, text: &str, heap: &Heap) -> RunResult<Value> {
        let cap_count = self.compiled.captures_len();

        match cap_count {
            // No capture groups — return list of full match strings
            0 | 1 => {
                let mut matches = Vec::new();
                for m in self.compiled.find_iter(text) {
                    check_slice_growth(&matches, heap)?;
                    matches.push(m.map_err(ExcType::re_pattern_error)?.as_str());
                }
                allocate_str_list(&matches, heap)
            }
            // One capture group — return list of the group's strings
            2 => {
                let mut matches = Vec::new();
                for caps in self.compiled.captures_iter(text) {
                    check_slice_growth(&matches, heap)?;
                    let caps = caps.map_err(ExcType::re_pattern_error)?;
                    matches.push(caps.get(1).map_or("", |m| m.as_str()));
                }
                allocate_str_list(&matches, heap)
            }
            // Multiple capture groups — return list of tuples, collected flat
            // and cut back into rows of `groups` once the scan has succeeded.
            _ => {
                let groups = cap_count - 1;
                let mut flat = Vec::new();
                for caps in self.compiled.captures_iter(text) {
                    let caps = caps.map_err(ExcType::re_pattern_error)?;
                    for cap in caps.iter().skip(1) {
                        check_slice_growth(&flat, heap)?;
                        flat.push(cap.map_or("", |m| m.as_str()));
                    }
                }
                allocate_tuple_list(&flat, groups, heap)
            }
        }
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
    pub fn sub(&self, repl: &str, text: &str, count: usize, heap: &Heap) -> RunResult<Value> {
        // Translate Python-style backreferences (\1, \2) to regex crate style ($1, $2)
        let rust_repl = translate_replacement(repl)?;
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
            check_estimated_size(result.len() + (text.len() - last_end), &heap.tracker)?;
        }

        result.push_str(&text[last_end..]);
        Ok(allocate_string(result, heap))
    }

    /// `pattern.split(string, maxsplit=0)` — split string by pattern occurrences.
    ///
    /// Returns a list of strings. If `maxsplit` is positive, at most `maxsplit`
    /// splits occur and the remainder of the string is returned as the final
    /// element; if it is negative, no splits occur at all (CPython's split loop
    /// runs zero times), returning the whole subject as a single element.
    pub fn split(&self, text: &str, maxsplit: i64, heap: &Heap) -> RunResult<Value> {
        let pieces = match maxsplit.cmp(&0) {
            Ordering::Less => vec![text],
            Ordering::Equal => collect_slices(self.compiled.split(text), heap)?,
            Ordering::Greater => {
                // `maxsplit + 1` pieces = at most `maxsplit` splits; saturate
                // for absurdly large limits (splitn caps at the piece count).
                let limit = usize::try_from(maxsplit).unwrap_or(usize::MAX).saturating_add(1);
                collect_slices(self.compiled.splitn(text, limit), heap)?
            }
        };

        allocate_str_list(&pieces, heap)
    }

    /// `pattern.finditer(string)` — return all matches as a list.
    ///
    /// Eagerly collects all match objects into a list. This differs from CPython's
    /// lazy iterator but produces the same results when iterated. The VM's `GetIter`
    /// opcode handles iteration over the returned list.
    pub fn finditer(&self, subject: &Value, text: &str, heap: &Heap) -> RunResult<Value> {
        // Every match shares one refcounted subject reference, not a copy each.
        let all_ascii = text.is_ascii();

        let mut results = Vec::new();
        for caps in self.compiled.captures_iter(text) {
            check_results_growth(&results, heap)?;
            let caps = caps.map_err(ExcType::re_pattern_error)?;
            results.push(self.build_match(&caps, subject, all_ascii, heap));
        }

        let list = List::new(results);
        Ok(Value::Ref(heap.allocate(HeapData::List(list))))
    }
}

/// Preflights the growth one more match result would cause.
///
/// A match list grows as long as the subject allows with no instruction
/// checkpoint in between, so without this the buffer's doubling can clear the
/// allocator's hard-limit headroom and kill the worker.
fn check_results_growth(results: &Vec<Value>, heap: &Heap) -> RunResult<()> {
    Ok(heap
        .tracker
        .check_growth(results.len(), results.capacity(), VALUE_SIZE)?)
}

/// [`check_results_growth`] for a buffer of borrowed match slices.
fn check_slice_growth(slices: &Vec<&str>, heap: &Heap) -> RunResult<()> {
    Ok(heap
        .tracker
        .check_growth(slices.len(), slices.capacity(), mem::size_of::<&str>())?)
}

/// Collects the pieces an iterator yields, preflighting the buffer as it grows.
///
/// A split's piece buffer is bounded only by the subject and is filled before the
/// result list exists, so the check on that list comes too late.
fn collect_slices<'t>(
    pieces: impl Iterator<Item = Result<&'t str, RegexError>>,
    heap: &Heap,
) -> RunResult<Vec<&'t str>> {
    let mut collected = Vec::new();
    for piece in pieces {
        check_slice_growth(&collected, heap)?;
        collected.push(piece.map_err(ExcType::re_pattern_error)?);
    }
    Ok(collected)
}

/// Allocates one heap string per slice and returns them as a list.
///
/// Fallible only before the first allocation, so no half-built list of heap
/// values can be stranded by a refused one.
fn allocate_str_list(slices: &[&str], heap: &Heap) -> RunResult<Value> {
    heap.tracker.check_allocation(slices.len().saturating_mul(VALUE_SIZE))?;
    let results = slices.iter().map(|s| allocate_string(*s, heap)).collect();
    Ok(Value::Ref(heap.allocate(HeapData::List(List::new(results)))))
}

/// [`allocate_str_list`] for rows of `groups` slices, one tuple per row.
fn allocate_tuple_list(flat: &[&str], groups: usize, heap: &Heap) -> RunResult<Value> {
    let rows = flat.len() / groups;
    heap.tracker.check_allocation(rows.saturating_mul(VALUE_SIZE))?;
    let results = flat
        .chunks_exact(groups)
        .map(|row| {
            let elements: TupleVec = row.iter().map(|s| allocate_string(*s, heap)).collect();
            allocate_tuple(elements, heap)
        })
        .collect();
    Ok(Value::Ref(heap.allocate(HeapData::List(List::new(results)))))
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, RePattern> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::RePattern
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::RePattern(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(self.get(vm.heap) == other.get(vm.heap)))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        // Pattern objects are always truthy (matching CPython).
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
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

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        match attr.static_string(vm.interns) {
            Some(StaticStrings::PatternAttr) => {
                let v = allocate_string(self.get(vm.heap).pattern.as_str(), vm.heap);
                Ok(Some(CallResult::Value(v)))
            }
            Some(StaticStrings::Flags) => Ok(Some(CallResult::Value(Value::Int(i64::from(self.get(vm.heap).flags))))),
            _ => Err(ExcType::attribute_error(Type::RePattern, attr.as_str(vm.interns))),
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let result = match attr.static_string(vm.interns) {
            Some(StaticStrings::Search) => {
                let arg = args.get_one_arg("Pattern.search", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?;
                self.get(vm.heap).search(arg, text, vm.heap)
            }
            Some(StaticStrings::Match) => {
                let arg = args.get_one_arg("Pattern.match", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?;
                self.get(vm.heap).match_start(arg, text, vm.heap)
            }
            Some(StaticStrings::Fullmatch) => {
                let arg = args.get_one_arg("Pattern.fullmatch", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?;
                self.get(vm.heap).fullmatch(arg, text, vm.heap)
            }
            Some(StaticStrings::Findall) => {
                let arg = args.get_one_arg("Pattern.findall", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?;
                self.get(vm.heap).findall(text, vm.heap)
            }
            Some(StaticStrings::Sub) => call_pattern_sub(self, args, vm),
            Some(StaticStrings::Split) => call_pattern_split(self, args, vm),
            Some(StaticStrings::Finditer) => {
                let arg = args.get_one_arg("Pattern.finditer", vm.heap)?;
                defer_drop!(arg, vm);
                let text = arg.to_str(vm)?;
                self.get(vm.heap).finditer(arg, text, vm.heap)
            }
            _ => return Err(ExcType::attribute_error_method(Type::RePattern, attr, args, vm)),
        }?;
        Ok(CallResult::Value(result))
    }
}

impl HeapItem for RePattern {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No heap references — all data is owned.
    }
}

/// Handles `pattern.sub(repl, string, count=0)` argument extraction and dispatch.
///
/// Separated from the main `py_call_attr` match to keep the borrow checker happy —
/// extracting multiple string arguments requires careful ordering of borrows.
/// Supports `count` as either positional or keyword argument.
fn call_pattern_sub<'h>(pattern: &HeapRead<'h, RePattern>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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
    let Ok(repl) = repl_val.to_str(vm) else {
        return Err(ExcType::type_error(
            "callable replacement is not yet supported in re.sub()",
        ));
    };
    let Some(count) = count else {
        // Validate octal escapes even when the negative count skips matching.
        translate_replacement(repl)?;
        // Negative count — Pattern.sub returns the input string unchanged.
        // The subject is still type-checked (`to_str` raises this method's
        // `expected string, not {t}` wording) before the refcount bump; no
        // need to re-allocate.
        let _ = string_val.to_str(vm)?;
        return Ok(string_val.clone_with_heap(vm.heap));
    };

    let text = string_val.to_str(vm)?;
    pattern.get(vm.heap).sub(repl, text, count, vm.heap)
}

/// Handles `pattern.split(string, maxsplit=0)` argument extraction and dispatch.
///
/// Supports `maxsplit` as either positional or keyword argument.
fn call_pattern_split<'h>(pattern: &HeapRead<'h, RePattern>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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
pub(crate) fn extract_maxsplit(val: Option<Value>, vm: &mut VM<'_>) -> RunResult<i64> {
    match val {
        None => Ok(0),
        Some(Value::Int(n)) => Ok(n),
        Some(Value::Bool(b)) => Ok(i64::from(b)),
        Some(other) => {
            let t = other.py_type_name(vm);
            other.drop_with(vm);

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
pub(crate) fn extract_count(val: Option<Value>, vm: &mut VM<'_>) -> RunResult<Option<usize>> {
    match val {
        None => Ok(Some(0)),
        // Saturate rather than `as`-cast: on 32-bit targets (wasm) a count
        // above usize::MAX would otherwise truncate — e.g. 2**32 to 0, which
        // means "replace all" instead of an unreachably large cap.
        Some(Value::Int(n)) if n >= 0 => Ok(Some(usize::try_from(n).unwrap_or(usize::MAX))),
        Some(Value::Bool(b)) => Ok(Some(usize::from(b))),
        Some(Value::Int(_)) => Ok(None),
        Some(other) => {
            let t = other.py_type_name(vm);
            other.drop_with(vm);

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
/// `delegate_size_limit` optionally caps the compiled size of the delegated
/// regex (`RegexBuilder::delegate_size_limit`) — used to bound cached patterns;
/// `None` uses the engine's default limit.
///
/// # Errors
///
/// Returns the raw `fancy_regex` error so callers can distinguish a size-limit
/// overflow from an invalid pattern (see [`is_size_limit_error`]) before
/// converting to `re.PatternError`.
fn compile_regex_limited(pattern: &str, flags: u16, delegate_size_limit: Option<usize>) -> Result<Regex, RegexError> {
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

    let mut builder = RegexBuilder::new(&full_pattern);
    if let Some(limit) = delegate_size_limit {
        builder.delegate_size_limit(limit);
    }
    builder.build()
}

/// True when `err` is the delegated engine's exceeded-size-limit error: the
/// pattern is valid, its compiled form just doesn't fit the requested cap.
fn is_size_limit_error(err: &RegexError) -> bool {
    match err {
        RegexError::CompileError(compile_error) => match &**compile_error {
            CompileError::InnerError(inner) => inner.size_limit().is_some(),
            _ => false,
        },
        _ => false,
    }
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
pub(crate) fn translate_replacement(repl: &str) -> RunResult<Cow<'_, str>> {
    // Fast path: no backslashes and no literal `$` means nothing to translate or escape.
    if !repl.contains('\\') && !repl.contains('$') {
        return Ok(Cow::Borrowed(repl));
    }

    let mut result = String::with_capacity(repl.len());
    let mut chars = repl.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(&d) if d.is_ascii_digit() => {
                    // A leading zero or three octal digits makes this an octal escape.
                    let octal_len = chars
                        .clone()
                        .take_while(|digit| matches!(digit, '0'..='7'))
                        .take(3)
                        .count();
                    if d == '0' || octal_len == 3 {
                        let mut octal = 0;
                        for _ in 0..octal_len {
                            let digit = chars.next().expect("octal digits were checked");
                            octal = octal * 8 + digit.to_digit(8).expect("octal digit");
                        }
                        if octal > 0o377 {
                            // The iterator has consumed the backslash and octal digits.
                            let position = repl.chars().count() - chars.clone().count() - 1 - octal_len;
                            return Err(ExcType::re_pattern_error(format!(
                                "octal escape value \\{octal:03o} outside of range 0-0o377 at position {position}"
                            )));
                        }
                        let decoded = char::from_u32(octal).expect("validated octal escape is a valid character");
                        if decoded == '$' {
                            // Keep an octal-escaped dollar literal in the regex replacement syntax.
                            result.push_str("$$");
                        } else {
                            result.push(decoded);
                        }
                    } else {
                        // TODO: This only handles single-digit backrefs (\1–\9).
                        // Multi-digit like \10 should be ${10} when group 10 exists,
                        // but that requires knowing the group count. See docstring.
                        result.push('$');
                        result.push(d);
                        chars.next();
                    }
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

    Ok(Cow::Owned(result))
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
