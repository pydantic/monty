//! Implementation of the `re` module.
//!
//! Provides regular expression matching operations modeled after Python's `re` module.
//! Uses the Rust `regex` crate, which guarantees linear-time matching (DFA-based),
//! preventing catastrophic backtracking (ReDoS) attacks — critical for sandbox security.
//!
//! # Supported module-level functions
//!
//! - `re.compile(pattern, flags=0)` → `re.Pattern`
//! - `re.search(pattern, string)` → `re.Match` or `None`
//! - `re.match(pattern, string)` → `re.Match` or `None`
//! - `re.fullmatch(pattern, string)` → `re.Match` or `None`
//! - `re.findall(pattern, string)` → `list`
//! - `re.sub(pattern, repl, string, count=0)` → `str`
//!
//! # Module attributes
//!
//! - `re.IGNORECASE` / `re.I` — case-insensitive matching (value: 2)
//! - `re.MULTILINE` / `re.M` — `^`/`$` match at line boundaries (value: 8)
//! - `re.DOTALL` / `re.S` — `.` matches newlines (value: 16)
//! - `re.PatternError` — exception type for invalid patterns
//!
//! # Unsupported Python regex features
//!
//! The Rust `regex` crate does not support backreferences (`\1`), lookahead/lookbehind
//! (`(?=...)`, `(?!...)`), or atomic groups. Attempting to compile patterns using these
//! features raises `re.PatternError`.

use std::borrow::Cow;

use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Module, PyTrait, RePattern, re_pattern::value_to_str},
    value::Value,
};

/// Python regex flag: case-insensitive matching.
const IGNORECASE: u8 = 2;
/// Python regex flag: `^` and `$` match at line boundaries.
const MULTILINE: u8 = 8;
/// Python regex flag: `.` matches newlines.
const DOTALL: u8 = 16;

/// Functions exposed by the `re` module.
///
/// Each variant corresponds to a module-level function that can be called directly
/// (e.g., `re.search(pattern, string)`). These are convenience wrappers that compile
/// the pattern on each call — for repeated use, `re.compile()` avoids recompilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum ReFunctions {
    /// `re.compile(pattern, flags=0)` — compile a pattern into a `re.Pattern` object.
    Compile,
    /// `re.search(pattern, string)` — find first match anywhere in the string.
    Search,
    /// `re.match(pattern, string)` — match anchored at the start.
    #[strum(serialize = "match")]
    Match,
    /// `re.fullmatch(pattern, string)` — match the entire string.
    Fullmatch,
    /// `re.findall(pattern, string)` — return all non-overlapping matches.
    Findall,
    /// `re.sub(pattern, repl, string, count=0)` — substitute matches.
    Sub,
}

/// Creates the `re` module and allocates it on the heap.
///
/// The module provides regex functions (`compile`, `search`, `match`, `fullmatch`,
/// `findall`, `sub`) and flag constants (`IGNORECASE`, `MULTILINE`, `DOTALL`).
///
/// # Returns
/// A `HeapId` pointing to the newly allocated module.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Re);

    // Functions
    module.set_attr(
        StaticStrings::Compile,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Compile)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Search,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Search)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Match,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Match)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Fullmatch,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Fullmatch)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Findall,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Findall)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Sub,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Sub)),
        heap,
        interns,
    );

    // Flag constants
    module.set_attr(
        StaticStrings::Ignorecase,
        Value::Int(i64::from(IGNORECASE)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::MultilineFlag,
        Value::Int(i64::from(MULTILINE)),
        heap,
        interns,
    );
    module.set_attr(StaticStrings::DotallFlag, Value::Int(i64::from(DOTALL)), heap, interns);

    heap.allocate(HeapData::Module(module))
}

/// Dispatches a call to a `re` module function.
///
/// Extracts arguments, compiles patterns as needed, and delegates to the appropriate
/// `RePattern` method. All functions return `AttrCallResult::Value` since regex
/// operations don't need host involvement.
pub(super) fn call(
    heap: &mut Heap<impl ResourceTracker>,
    function: ReFunctions,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<AttrCallResult> {
    match function {
        ReFunctions::Compile => call_compile(heap, args, interns).map(AttrCallResult::Value),
        ReFunctions::Search => call_search(heap, args, interns).map(AttrCallResult::Value),
        ReFunctions::Match => call_match(heap, args, interns).map(AttrCallResult::Value),
        ReFunctions::Fullmatch => call_fullmatch(heap, args, interns).map(AttrCallResult::Value),
        ReFunctions::Findall => call_findall(heap, args, interns).map(AttrCallResult::Value),
        ReFunctions::Sub => call_sub(heap, args, interns).map(AttrCallResult::Value),
    }
}

/// `re.compile(pattern, flags=0)` — compile a regular expression pattern.
///
/// Returns a `re.Pattern` object that can be reused for multiple match operations.
/// The pattern is compiled once and stored, avoiding recompilation overhead.
fn call_compile(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern_val, flags) = extract_pattern_and_flags(args, "re.compile", heap, interns)?;
    let compiled = RePattern::compile(pattern_val, flags)?;
    Ok(Value::Ref(heap.allocate(HeapData::RePattern(compiled))?))
}

/// `re.search(pattern, string)` — scan through string looking for a match.
///
/// Compiles the pattern, then delegates to `RePattern::search`. Returns a `re.Match`
/// object on success, or `None` if no position in the string matches.
fn call_search(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text) = extract_pattern_and_string(args, "re.search", heap, interns)?;
    let compiled = RePattern::compile(pattern, 0)?;
    compiled.search(&text, heap)
}

/// `re.match(pattern, string)` — match at the beginning of the string.
///
/// Compiles the pattern, then delegates to `RePattern::match_start`. Returns a `re.Match`
/// object if the pattern matches at position 0, or `None` otherwise.
fn call_match(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text) = extract_pattern_and_string(args, "re.match", heap, interns)?;
    let compiled = RePattern::compile(pattern, 0)?;
    compiled.match_start(&text, heap)
}

/// `re.fullmatch(pattern, string)` — match the entire string.
///
/// Compiles the pattern, then delegates to `RePattern::fullmatch`. Returns a `re.Match`
/// object if the pattern matches the whole string, or `None` otherwise.
fn call_fullmatch(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text) = extract_pattern_and_string(args, "re.fullmatch", heap, interns)?;
    let compiled = RePattern::compile(pattern, 0)?;
    compiled.fullmatch(&text, heap)
}

/// `re.findall(pattern, string)` — find all non-overlapping matches.
///
/// Compiles the pattern, then delegates to `RePattern::findall`. Returns a list of
/// strings or tuples depending on the number of capture groups (matching CPython semantics).
fn call_findall(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text) = extract_pattern_and_string(args, "re.findall", heap, interns)?;
    let compiled = RePattern::compile(pattern, 0)?;
    compiled.findall(&text, heap)
}

/// `re.sub(pattern, repl, string, count=0)` — substitute matches with a replacement.
///
/// Compiles the pattern, then delegates to `RePattern::sub`. Replaces occurrences of the
/// pattern with the replacement string. When `count` is 0, all matches are replaced.
fn call_sub(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let mut pos = args.into_pos_only("re.sub", heap)?;

    let Some(pattern_val) = pos.next() else {
        return Err(ExcType::type_error("re.sub() missing required argument: 'pattern'"));
    };
    defer_drop!(pattern_val, heap);

    let Some(repl_val) = pos.next() else {
        return Err(ExcType::type_error("re.sub() missing required argument: 'repl'"));
    };
    defer_drop!(repl_val, heap);

    let Some(string_val) = pos.next() else {
        return Err(ExcType::type_error("re.sub() missing required argument: 'string'"));
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
        return Err(ExcType::type_error("re.sub() takes at most 4 positional arguments"));
    }

    let pattern = value_to_str(pattern_val, heap, interns)?.into_owned();
    let repl = value_to_str(repl_val, heap, interns)?.into_owned();
    let text = value_to_str(string_val, heap, interns)?.into_owned();

    let compiled = RePattern::compile(pattern, 0)?;
    compiled.sub(&repl, &text, count, heap)
}

/// Extracts pattern string and optional flags from arguments for `re.compile()`.
///
/// Accepts 1 or 2 positional arguments: `(pattern)` or `(pattern, flags)`.
/// The pattern must be a string, and flags must be a non-negative integer.
fn extract_pattern_and_flags(
    args: ArgValues,
    func_name: &str,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(String, u8)> {
    let (pattern_val, flags_val) = args.get_one_two_args(func_name, heap)?;
    defer_drop!(pattern_val, heap);

    let pattern = value_to_str(pattern_val, heap, interns)?.into_owned();

    let flags = match flags_val {
        Some(Value::Int(n)) => {
            u8::try_from(n).map_err(|_| ExcType::type_error("flags must be a non-negative integer"))?
        }
        Some(other) => {
            let t = other.py_type(heap);
            other.drop_with_heap(heap);
            return Err(ExcType::type_error(format!("expected int for flags, not {t}")));
        }
        None => 0,
    };

    Ok((pattern, flags))
}

/// Extracts pattern and string arguments for two-argument `re` functions.
///
/// Used by `re.search()`, `re.match()`, `re.fullmatch()`, and `re.findall()`,
/// all of which take exactly `(pattern, string)`.
fn extract_pattern_and_string(
    args: ArgValues,
    func_name: &str,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(String, Cow<'static, str>)> {
    let (pattern_val, string_val) = args.get_two_args(func_name, heap)?;
    defer_drop!(pattern_val, heap);
    defer_drop!(string_val, heap);

    let pattern = value_to_str(pattern_val, heap, interns)?.into_owned();
    let text = value_to_str(string_val, heap, interns)?.into_owned();

    Ok((pattern, Cow::Owned(text)))
}
