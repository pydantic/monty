//! Implementation of the `re` module.
//!
//! Provides regular expression matching operations.
//! Uses the Rust `fancy-regex` crate.
//!
//! # Supported module-level functions
//!
//! - `re.compile(pattern, flags=0)` → `re.Pattern`
//! - `re.search(pattern, string, flags=0)` → `re.Match` or `None`
//! - `re.match(pattern, string, flags=0)` → `re.Match` or `None`
//! - `re.fullmatch(pattern, string, flags=0)` → `re.Match` or `None`
//! - `re.findall(pattern, string, flags=0)` → `list`
//! - `re.sub(pattern, repl, string, count=0, flags=0)` → `str`
//! - `re.split(pattern, string, maxsplit=0, flags=0)` → `list`
//! - `re.finditer(pattern, string, flags=0)` → iterator of `re.Match`
//! - `re.escape(pattern)` → `str`
//!
//! # Module attributes
//!
//! - `re.NOFLAG` - no flag (value: 0)
//! - `re.IGNORECASE` / `re.I` — case-insensitive matching (value: 2)
//! - `re.MULTILINE` / `re.M` — `^`/`$` match at line boundaries (value: 8)
//! - `re.DOTALL` / `re.S` — `.` matches newlines (value: 16)
//! - `re.ASCII` / `re.A` — ASCII-only matching for `\w`, `\d`, `\s` (value: 256)
//! - `re.PatternError` / `re.error` — exception type for invalid patterns

use std::borrow::Cow;

use crate::{
    args::ArgValues,
    builtins::Builtins,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Module, PyTrait, RePattern, Str, Type, re_pattern::value_to_str},
    value::Value,
};

/// Python regex flag: no flag being applied.
pub(crate) const NOFLAG: u16 = 0;
/// Python regex flag: case-insensitive matching.
pub(crate) const IGNORECASE: u16 = 2;
/// Python regex flag: `^` and `$` match at line boundaries.
pub(crate) const MULTILINE: u16 = 8;
/// Python regex flag: `.` matches newlines.
pub(crate) const DOTALL: u16 = 16;
/// Python regex flag: ASCII-only matching for `\w`, `\b`, `\d`, `\s`.
pub(crate) const ASCII: u16 = 256;

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
    /// `re.search(pattern, string, flags=0)` — find first match anywhere in the string.
    Search,
    /// `re.match(pattern, string, flags=0)` — match anchored at the start.
    Match,
    /// `re.fullmatch(pattern, string, flags=0)` — match the entire string.
    Fullmatch,
    /// `re.findall(pattern, string, flags=0)` — return all non-overlapping matches.
    Findall,
    /// `re.sub(pattern, repl, string, count=0, flags=0)` — substitute matches.
    Sub,
    /// `re.split(pattern, string, maxsplit=0, flags=0)` — split string by pattern.
    Split,
    /// `re.finditer(pattern, string, flags=0)` — return iterator over all matches.
    Finditer,
    /// `re.escape(pattern)` — escape all non-alphanumeric characters in pattern.
    Escape,
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
    module.set_attr(
        StaticStrings::Split,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Split)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Finditer,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Finditer)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Escape,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Escape)),
        heap,
        interns,
    );

    // Flag constants
    module.set_attr(StaticStrings::NoFlag, Value::Int(i64::from(NOFLAG)), heap, interns);
    module.set_attr(
        StaticStrings::Ignorecase,
        Value::Int(i64::from(IGNORECASE)),
        heap,
        interns,
    );
    module.set_attr(StaticStrings::I, Value::Int(i64::from(IGNORECASE)), heap, interns);
    module.set_attr(
        StaticStrings::MultilineFlag,
        Value::Int(i64::from(MULTILINE)),
        heap,
        interns,
    );
    module.set_attr(StaticStrings::M, Value::Int(i64::from(MULTILINE)), heap, interns);
    module.set_attr(StaticStrings::DotallFlag, Value::Int(i64::from(DOTALL)), heap, interns);
    module.set_attr(StaticStrings::S, Value::Int(i64::from(DOTALL)), heap, interns);
    module.set_attr(StaticStrings::AsciiFlag, Value::Int(i64::from(ASCII)), heap, interns);
    module.set_attr(StaticStrings::A, Value::Int(i64::from(ASCII)), heap, interns);

    // Exception types
    module.set_attr(
        StaticStrings::PatternError,
        Value::Builtin(Builtins::ExcType(ExcType::RePatternError)),
        heap,
        interns,
    );
    // `re.error` is the historical alias for `re.PatternError` (still widely used)
    module.set_attr(
        StaticStrings::Error,
        Value::Builtin(Builtins::ExcType(ExcType::RePatternError)),
        heap,
        interns,
    );

    // Constructed types
    module.set_attr(
        StaticStrings::PatternClass,
        Value::Builtin(Builtins::Type(Type::RePattern)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::MatchClass,
        Value::Builtin(Builtins::Type(Type::ReMatch)),
        heap,
        interns,
    );

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
        ReFunctions::Split => call_split(heap, args, interns).map(AttrCallResult::Value),
        ReFunctions::Finditer => call_finditer(heap, args, interns).map(AttrCallResult::Value),
        ReFunctions::Escape => call_escape(heap, args, interns).map(AttrCallResult::Value),
    }
}

/// `re.compile(pattern, flags=0)` — compile a regular expression pattern.
///
/// Returns a `re.Pattern` object that can be reused for multiple match operations.
/// The pattern is compiled once and stored, avoiding recompilation overhead.
fn call_compile(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern_val, flags) = extract_pattern_and_flags(args, "re.compile", heap, interns)?;
    let compiled = RePattern::compile(pattern_val, flags)?;
    Ok(Value::Ref(heap.allocate(HeapData::RePattern(Box::new(compiled)))?))
}

/// `re.search(pattern, string, flags=0)` — scan through string looking for a match.
///
/// Compiles the pattern, then delegates to `RePattern::search`. Returns a `re.Match`
/// object on success, or `None` if no position in the string matches.
fn call_search(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text, flags) = extract_pattern_string_flags(args, "re.search", heap, interns)?;
    let compiled = RePattern::compile(pattern, flags)?;
    compiled.search(&text, heap)
}

/// `re.match(pattern, string, flags=0)` — match at the beginning of the string.
///
/// Compiles the pattern, then delegates to `RePattern::match_start`. Returns a `re.Match`
/// object if the pattern matches at position 0, or `None` otherwise.
fn call_match(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text, flags) = extract_pattern_string_flags(args, "re.match", heap, interns)?;
    let compiled = RePattern::compile(pattern, flags)?;
    compiled.match_start(&text, heap)
}

/// `re.fullmatch(pattern, string, flags=0)` — match the entire string.
///
/// Compiles the pattern, then delegates to `RePattern::fullmatch`. Returns a `re.Match`
/// object if the pattern matches the whole string, or `None` otherwise.
fn call_fullmatch(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text, flags) = extract_pattern_string_flags(args, "re.fullmatch", heap, interns)?;
    let compiled = RePattern::compile(pattern, flags)?;
    compiled.fullmatch(&text, heap)
}

/// `re.findall(pattern, string, flags=0)` — find all non-overlapping matches.
///
/// Compiles the pattern, then delegates to `RePattern::findall`. Returns a list of
/// strings or tuples depending on the number of capture groups (matching CPython semantics).
fn call_findall(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text, flags) = extract_pattern_string_flags(args, "re.findall", heap, interns)?;
    let compiled = RePattern::compile(pattern, flags)?;
    compiled.findall(&text, heap)
}

/// `re.sub(pattern, repl, string, count=0, flags=0)` — substitute matches with a replacement.
///
/// Compiles the pattern, then delegates to `RePattern::sub`. Replaces occurrences of the
/// pattern with the replacement string. When `count` is 0, all matches are replaced.
/// Supports both positional and keyword arguments for `count` and `flags`.
fn call_sub(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

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

    // Extract count and flags from remaining positional args
    let pos_count = pos.next();
    let pos_flags = pos.next();

    if let Some(extra) = pos.next() {
        extra.drop_with_heap(heap);
        return Err(ExcType::type_error("re.sub() takes at most 5 positional arguments"));
    }

    // Extract count and flags from kwargs (if not given positionally)
    let (mut kw_count, mut kw_flags): (Option<Value>, Option<Value>) = (None, None);
    for (key, value) in kwargs {
        defer_drop!(key, heap);
        let Some(keyword_name) = key.as_either_str(heap) else {
            value.drop_with_heap(heap);
            return Err(ExcType::type_error("keywords must be strings"));
        };
        let key_str = keyword_name.as_str(interns);
        match key_str {
            "count" => {
                if pos_count.is_some() {
                    value.drop_with_heap(heap);
                    return Err(ExcType::type_error("re.sub() got multiple values for argument 'count'"));
                }
                kw_count.replace(value).drop_with_heap(heap);
            }
            "flags" => {
                if pos_flags.is_some() {
                    value.drop_with_heap(heap);
                    return Err(ExcType::type_error("re.sub() got multiple values for argument 'flags'"));
                }
                kw_flags.replace(value).drop_with_heap(heap);
            }
            _ => {
                value.drop_with_heap(heap);
                return Err(ExcType::type_error(format!(
                    "'{key_str}' is an invalid keyword argument for re.sub()"
                )));
            }
        }
    }

    let count_val = pos_count.or(kw_count);
    let flags_val = pos_flags.or(kw_flags);

    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "n is checked non-negative above"
    )]
    let count = match count_val {
        Some(Value::Int(n)) if n >= 0 => n as usize,
        Some(Value::Bool(b)) => usize::from(b),
        Some(Value::Int(_)) => {
            // Negative count — return original string unchanged
            let _flags = extract_flags(flags_val, heap)?;
            let text = value_to_str(string_val, heap, interns)?.into_owned();
            let s = Str::new(text);
            return Ok(Value::Ref(heap.allocate(HeapData::Str(s))?));
        }
        Some(other) => {
            let t = other.py_type(heap);
            other.drop_with_heap(heap);
            return Err(ExcType::type_error(format!(
                "'{t}' object cannot be interpreted as an integer for 'count' argument"
            )));
        }
        None => 0,
    };

    let flags = extract_flags(flags_val, heap)?;

    let pattern = value_to_str(pattern_val, heap, interns)?.into_owned();

    // Check that repl is a string — callable replacement is not supported
    if !repl_val.is_str(heap) {
        return Err(ExcType::type_error(
            "callable replacement is not yet supported in re.sub()",
        ));
    }
    let repl = value_to_str(repl_val, heap, interns)?.into_owned();
    let text = value_to_str(string_val, heap, interns)?.into_owned();

    let compiled = RePattern::compile(pattern, flags)?;
    compiled.sub(&repl, &text, count, heap)
}

/// `re.split(pattern, string, maxsplit=0, flags=0)` — split string by pattern occurrences.
///
/// Returns a list of strings. If `maxsplit` is non-zero, at most `maxsplit` splits occur
/// and the remainder of the string is returned as the final list element.
/// Supports both positional and keyword arguments for `maxsplit` and `flags`.
fn call_split(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let Some(pattern_val) = pos.next() else {
        return Err(ExcType::type_error("re.split() missing required argument: 'pattern'"));
    };
    defer_drop!(pattern_val, heap);

    let Some(string_val) = pos.next() else {
        return Err(ExcType::type_error("re.split() missing required argument: 'string'"));
    };
    defer_drop!(string_val, heap);

    let pos_maxsplit = pos.next();
    let pos_flags = pos.next();

    if let Some(extra) = pos.next() {
        extra.drop_with_heap(heap);
        return Err(ExcType::type_error("re.split() takes at most 4 positional arguments"));
    }

    let (mut kw_maxsplit, mut kw_flags): (Option<Value>, Option<Value>) = (None, None);
    for (key, value) in kwargs {
        defer_drop!(key, heap);
        let Some(keyword_name) = key.as_either_str(heap) else {
            value.drop_with_heap(heap);
            return Err(ExcType::type_error("keywords must be strings"));
        };
        let key_str = keyword_name.as_str(interns);
        match key_str {
            "maxsplit" => {
                if pos_maxsplit.is_some() {
                    value.drop_with_heap(heap);
                    return Err(ExcType::type_error(
                        "re.split() got multiple values for argument 'maxsplit'",
                    ));
                }
                kw_maxsplit.replace(value).drop_with_heap(heap);
            }
            "flags" => {
                if pos_flags.is_some() {
                    value.drop_with_heap(heap);
                    return Err(ExcType::type_error(
                        "re.split() got multiple values for argument 'flags'",
                    ));
                }
                kw_flags.replace(value).drop_with_heap(heap);
            }
            _ => {
                value.drop_with_heap(heap);
                return Err(ExcType::type_error(format!(
                    "'{key_str}' is an invalid keyword argument for re.split()"
                )));
            }
        }
    }

    let maxsplit = extract_maxsplit(pos_maxsplit.or(kw_maxsplit), heap)?;
    let flags = extract_flags(pos_flags.or(kw_flags), heap)?;

    let pattern = value_to_str(pattern_val, heap, interns)?.into_owned();
    let text = value_to_str(string_val, heap, interns)?.into_owned();

    let compiled = RePattern::compile(pattern, flags)?;
    compiled.split(&text, maxsplit, heap)
}

/// `re.finditer(pattern, string, flags=0)` — return all matches as a list.
///
/// Eagerly collects all match objects into a list. When the user iterates with
/// `for m in re.finditer(...)`, the VM's `GetIter` opcode handles iteration
/// over the returned list automatically.
fn call_finditer(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pattern, text, flags) = extract_pattern_string_flags(args, "re.finditer", heap, interns)?;
    let compiled = RePattern::compile(pattern, flags)?;
    compiled.finditer(&text, heap)
}

/// `re.escape(pattern)` — escape special regex characters in a string.
///
/// Returns a string with all regex metacharacters and whitespace prefixed with
/// a backslash. Only characters that have special meaning in regex patterns are
/// escaped, matching CPython 3.7+ behavior.
///
/// Escaped characters: `\t \n \v \f \r   # $ & ( ) * + - . ? [ \ ] ^ { | } ~`
fn call_escape(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let arg = args.get_one_arg("re.escape", heap)?;
    defer_drop!(arg, heap);
    let text = value_to_str(arg, heap, interns)?.into_owned();

    let mut result = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if should_escape(c) {
            result.push('\\');
        }
        result.push(c);
    }

    let s = Str::new(result);
    Ok(Value::Ref(heap.allocate(HeapData::Str(s))?))
}

/// Returns whether a character should be escaped by `re.escape()`.
///
/// Matches CPython's `_special_chars_map` — only regex metacharacters and whitespace.
fn should_escape(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\x0b'
            | '\x0c'
            | '\r'
            | ' '
            | '#'
            | '$'
            | '&'
            | '('
            | ')'
            | '*'
            | '+'
            | '-'
            | '.'
            | '?'
            | '['
            | '\\'
            | ']'
            | '^'
            | '{'
            | '|'
            | '}'
            | '~'
    )
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

/// Extracts pattern string and optional flags from arguments for `re.compile()`.
///
/// Accepts 1 or 2 positional arguments: `(pattern)` or `(pattern, flags)`.
/// The pattern must be a string, and flags must be a non-negative integer.
fn extract_pattern_and_flags(
    args: ArgValues,
    func_name: &str,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(String, u16)> {
    let (pattern_val, flags_val) = args.get_one_two_args(func_name, heap)?;
    defer_drop!(pattern_val, heap);

    let pattern = value_to_str(pattern_val, heap, interns)?.into_owned();
    let flags = extract_flags(flags_val, heap)?;

    Ok((pattern, flags))
}

/// Extracts a flags value from an optional `Value`, validating it is a non-negative integer
/// that fits in a `u16`.
fn extract_flags(flags_val: Option<Value>, heap: &mut Heap<impl ResourceTracker>) -> RunResult<u16> {
    match flags_val {
        Some(Value::Int(n)) => {
            u16::try_from(n).map_err(|_| ExcType::type_error("flags must be a non-negative integer"))
        }
        // CPython treats bool as int subclass: True=1, False=0.
        Some(Value::Bool(b)) => Ok(u16::from(b)),
        Some(other) => {
            let t = other.py_type(heap);
            other.drop_with_heap(heap);
            Err(ExcType::type_error(format!("expected int for flags, not {t}")))
        }
        None => Ok(0),
    }
}

/// Extracts pattern, string, and optional flags for `re.search()`, `re.match()`,
/// `re.fullmatch()`, and `re.findall()`.
///
/// Accepts 2 or 3 positional arguments: `(pattern, string)` or `(pattern, string, flags)`.
fn extract_pattern_string_flags(
    args: ArgValues,
    func_name: &str,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(String, Cow<'static, str>, u16)> {
    let pos = args.into_pos_only(func_name, heap)?;
    defer_drop_mut!(pos, heap);

    let Some(pattern_val) = pos.next() else {
        return Err(ExcType::type_error(format!(
            "{func_name}() missing required argument: 'pattern'"
        )));
    };
    defer_drop!(pattern_val, heap);

    let Some(string_val) = pos.next() else {
        return Err(ExcType::type_error(format!(
            "{func_name}() missing required argument: 'string'"
        )));
    };
    defer_drop!(string_val, heap);

    let flags = extract_flags(pos.next(), heap)?;

    if let Some(extra) = pos.next() {
        extra.drop_with_heap(heap);
        return Err(ExcType::type_error(format!(
            "{func_name}() takes at most 3 positional arguments"
        )));
    }

    let pattern = value_to_str(pattern_val, heap, interns)?.into_owned();
    let text = value_to_str(string_val, heap, interns)?.into_owned();

    Ok((pattern, Cow::Owned(text), flags))
}
