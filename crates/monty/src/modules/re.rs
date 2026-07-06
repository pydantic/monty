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
//! Like CPython's pure-Python `re` functions, all arguments are accepted
//! positionally or by keyword, and `pattern` may be a `str` or an
//! already-compiled `re.Pattern` (in which case non-zero `flags` raise
//! `ValueError`). Signature errors use `#[from_args(style = def)]` so their
//! wording matches CPython's `def` binding exactly.
//!
//! # Module attributes
//!
//! - `re.NOFLAG` - no flag (value: 0)
//! - `re.IGNORECASE` / `re.I` — case-insensitive matching (value: 2)
//! - `re.MULTILINE` / `re.M` — `^`/`$` match at line boundaries (value: 8)
//! - `re.DOTALL` / `re.S` — `.` matches newlines (value: 16)
//! - `re.ASCII` / `re.A` — ASCII-only matching for `\w`, `\d`, `\s` (value: 256)
//! - `re.PatternError` / `re.error` — exception type for invalid patterns

use std::rc::Rc;

use ahash::RandomState;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{ContainsHeap, DropWithHeap, Heap, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{
        BoundedCompileError, Module, RePattern, Type,
        re_pattern::{extract_count, extract_maxsplit},
        str::allocate_string,
    },
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
pub fn create_module(vm: &mut VM<'_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Re);

    // Functions
    module.set_attr(
        StaticStrings::Compile,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Compile)),
        vm,
    );
    module.set_attr(
        StaticStrings::Search,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Search)),
        vm,
    );
    module.set_attr(
        StaticStrings::Match,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Match)),
        vm,
    );
    module.set_attr(
        StaticStrings::Fullmatch,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Fullmatch)),
        vm,
    );
    module.set_attr(
        StaticStrings::Findall,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Findall)),
        vm,
    );
    module.set_attr(
        StaticStrings::Sub,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Sub)),
        vm,
    );
    module.set_attr(
        StaticStrings::Split,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Split)),
        vm,
    );
    module.set_attr(
        StaticStrings::Finditer,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Finditer)),
        vm,
    );
    module.set_attr(
        StaticStrings::Escape,
        Value::ModuleFunction(ModuleFunctions::Re(ReFunctions::Escape)),
        vm,
    );

    // Flag constants
    module.set_attr(StaticStrings::NoFlag, Value::Int(i64::from(NOFLAG)), vm);
    module.set_attr(StaticStrings::Ignorecase, Value::Int(i64::from(IGNORECASE)), vm);
    module.set_attr(StaticStrings::I, Value::Int(i64::from(IGNORECASE)), vm);
    module.set_attr(StaticStrings::MultilineFlag, Value::Int(i64::from(MULTILINE)), vm);
    module.set_attr(StaticStrings::M, Value::Int(i64::from(MULTILINE)), vm);
    module.set_attr(StaticStrings::DotallFlag, Value::Int(i64::from(DOTALL)), vm);
    module.set_attr(StaticStrings::S, Value::Int(i64::from(DOTALL)), vm);
    module.set_attr(StaticStrings::AsciiFlag, Value::Int(i64::from(ASCII)), vm);
    module.set_attr(StaticStrings::A, Value::Int(i64::from(ASCII)), vm);

    // Exception types
    module.set_attr(
        StaticStrings::PatternError,
        Value::Builtin(Builtins::ExcType(ExcType::RePatternError)),
        vm,
    );
    // `re.error` is the historical alias for `re.PatternError` (still widely used)
    module.set_attr(
        StaticStrings::Error,
        Value::Builtin(Builtins::ExcType(ExcType::RePatternError)),
        vm,
    );

    // Constructed types
    module.set_attr(
        StaticStrings::PatternClass,
        Value::Builtin(Builtins::Type(Type::RePattern)),
        vm,
    );
    module.set_attr(
        StaticStrings::MatchClass,
        Value::Builtin(Builtins::Type(Type::ReMatch)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a call to a `re` module function.
///
/// Extracts arguments, compiles patterns as needed, and delegates to the appropriate
/// `RePattern` method. All functions return `CallResult::Value` since regex
/// operations don't need host involvement.
pub(super) fn call(
    vm: &mut VM<'_, impl ResourceTracker>,
    function: ReFunctions,
    args: ArgValues,
) -> RunResult<CallResult> {
    match function {
        ReFunctions::Compile => call_compile(vm, args).map(CallResult::Value),
        ReFunctions::Search => call_search(vm, args).map(CallResult::Value),
        ReFunctions::Match => call_match(vm, args).map(CallResult::Value),
        ReFunctions::Fullmatch => call_fullmatch(vm, args).map(CallResult::Value),
        ReFunctions::Findall => call_findall(vm, args).map(CallResult::Value),
        ReFunctions::Sub => call_sub(vm, args).map(CallResult::Value),
        ReFunctions::Split => call_split(vm, args).map(CallResult::Value),
        ReFunctions::Finditer => call_finditer(vm, args).map(CallResult::Value),
        ReFunctions::Escape => call_escape(vm, args).map(CallResult::Value),
    }
}

/// `re.compile(pattern, flags=0)` — compile a regular expression pattern.
///
/// Returns a `re.Pattern` object that can be reused for multiple match operations.
/// An already-compiled pattern is returned unchanged (`re.compile(p) is p`,
/// matching CPython's `_compile` pass-through).
fn call_compile(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReCompileArgs { pattern, flags } = ReCompileArgs::from_args(args, vm)?;
    match resolve_pattern(pattern, flags, vm)? {
        // Clone out of the shared cache entry: the returned `re.Pattern` is the
        // user's own object, independent of the cache.
        ResolvedPattern::Cached(compiled) => Ok(Value::Ref(
            vm.heap.allocate(HeapData::RePattern(Box::new((*compiled).clone())))?,
        )),
        // Ownership of the extracted value transfers straight to the caller,
        // so the refcount taken at argument extraction is the caller's.
        ResolvedPattern::Heap(value) => Ok(value),
    }
}

/// `re.search(pattern, string, flags=0)` — scan through string looking for a match.
///
/// Resolves the pattern, then delegates to `RePattern::search`. Returns a `re.Match`
/// object on success, or `None` if no position in the string matches.
fn call_search(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReSearchArgs { pattern, string, flags } = ReSearchArgs::from_args(args, vm)?;
    defer_drop!(string, vm);
    let resolved = resolve_pattern(pattern, flags, vm)?;
    defer_drop!(resolved, vm);
    resolved.get(vm.heap).search(string, subject_str(string, vm)?, vm.heap)
}

/// `re.match(pattern, string, flags=0)` — match at the beginning of the string.
///
/// Resolves the pattern, then delegates to `RePattern::match_start`. Returns a `re.Match`
/// object if the pattern matches at position 0, or `None` otherwise.
fn call_match(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReMatchArgs { pattern, string, flags } = ReMatchArgs::from_args(args, vm)?;
    defer_drop!(string, vm);
    let resolved = resolve_pattern(pattern, flags, vm)?;
    defer_drop!(resolved, vm);
    resolved
        .get(vm.heap)
        .match_start(string, subject_str(string, vm)?, vm.heap)
}

/// `re.fullmatch(pattern, string, flags=0)` — match the entire string.
///
/// Resolves the pattern, then delegates to `RePattern::fullmatch`. Returns a `re.Match`
/// object if the pattern matches the whole string, or `None` otherwise.
fn call_fullmatch(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReFullmatchArgs { pattern, string, flags } = ReFullmatchArgs::from_args(args, vm)?;
    defer_drop!(string, vm);
    let resolved = resolve_pattern(pattern, flags, vm)?;
    defer_drop!(resolved, vm);
    resolved
        .get(vm.heap)
        .fullmatch(string, subject_str(string, vm)?, vm.heap)
}

/// `re.findall(pattern, string, flags=0)` — find all non-overlapping matches.
///
/// Resolves the pattern, then delegates to `RePattern::findall`. Returns a list of
/// strings or tuples depending on the number of capture groups (matching CPython semantics).
fn call_findall(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReFindallArgs { pattern, string, flags } = ReFindallArgs::from_args(args, vm)?;
    defer_drop!(string, vm);
    let resolved = resolve_pattern(pattern, flags, vm)?;
    defer_drop!(resolved, vm);
    resolved.get(vm.heap).findall(subject_str(string, vm)?, vm.heap)
}

/// `re.sub(pattern, repl, string, count=0, flags=0)` — substitute matches with a replacement.
///
/// Resolves the pattern, then delegates to `RePattern::sub`. Replaces occurrences of the
/// pattern with the replacement string. When `count` is 0, all matches are replaced.
///
/// The pattern is resolved *before* `count` is validated so a bad pattern (or
/// flags with a compiled pattern) wins over a bad count, matching CPython
/// where `_compile` runs before `Pattern.sub` parses its arguments.
fn call_sub(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReSubArgs {
        pattern,
        repl,
        string,
        count,
        flags,
    } = ReSubArgs::from_args(args, vm)?;
    defer_drop!(string, vm);
    defer_drop!(repl, vm);
    // `count` must outlive `resolve_pattern`'s `?` (pattern errors win over a
    // bad count, so it cannot be extracted yet) — guard it, then `take` it out.
    defer_drop_mut!(count, vm);
    let resolved = resolve_pattern(pattern, flags, vm)?;
    defer_drop!(resolved, vm);

    let count = extract_count(count.take(), vm)?;

    // Check that repl is a string — callable replacement is not supported.
    // CPython processes the replacement template *before* its match loop, so
    // this check must precede the negative-count early return below: a bad
    // repl raises even when zero substitutions will run.
    if !repl.is_str(vm.heap) {
        return Err(ExcType::type_error(
            "callable replacement is not yet supported in re.sub()",
        ));
    }

    let Some(count) = count else {
        // Negative count — re.sub returns the input string unchanged.
        // CPython still type-checks the subject before its (empty) match
        // loop, so validate first, then just bump the refcount; no need to
        // re-allocate (the guard drops the extraction's reference, the
        // clone is the caller's).
        let _ = subject_str(string, vm)?;
        return Ok(string.clone_with_heap(vm.heap));
    };

    resolved
        .get(vm.heap)
        .sub(repl.to_str(vm)?, subject_str(string, vm)?, count, vm.heap)
}

/// `re.split(pattern, string, maxsplit=0, flags=0)` — split string by pattern occurrences.
///
/// Returns a list of strings. If `maxsplit` is non-zero, at most `maxsplit` splits occur
/// and the remainder of the string is returned as the final list element.
fn call_split(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReSplitArgs {
        pattern,
        string,
        maxsplit,
        flags,
    } = ReSplitArgs::from_args(args, vm)?;
    defer_drop!(string, vm);
    // As in `call_sub`: `maxsplit` must survive a pattern/flags error, so
    // guard it before `resolve_pattern` and `take` it out afterwards.
    defer_drop_mut!(maxsplit, vm);
    let resolved = resolve_pattern(pattern, flags, vm)?;
    defer_drop!(resolved, vm);

    let maxsplit = extract_maxsplit(maxsplit.take(), vm)?;
    resolved.get(vm.heap).split(subject_str(string, vm)?, maxsplit, vm.heap)
}

/// Argument shape for `re.sub(pattern, repl, string, count=0, flags=0)`.
///
/// The `pattern` and `string` field names override their `StaticStrings`
/// variant via `static_string =` because `Pattern` / `String` are
/// already used by other interner entries (`re.Pattern` class name and
/// `match.string` attribute respectively). The actual interned string for
/// dispatch is still `"pattern"` / `"string"` via those repurposed variants.
///
/// Every field is a raw `Value`: like CPython's `def` binding, signature
/// binding never type-checks, so all coercion (pattern/flags via
/// [`resolve_pattern`], count, the callable-replacement check, the subject)
/// happens in the body in CPython's error order.
#[derive(FromArgs)]
#[from_args(name = "sub", style = def)]
struct ReSubArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    repl: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default)]
    count: Option<Value>,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Argument shape for `re.split(pattern, string, maxsplit=0, flags=0)`.
///
/// See `ReSubArgs` for why `pattern` / `string` use `static_string`.
#[derive(FromArgs)]
#[from_args(name = "split", style = def)]
struct ReSplitArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default)]
    maxsplit: Option<Value>,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Argument shape for `re.compile(pattern, flags=0)`.
#[derive(FromArgs)]
#[from_args(name = "compile", style = def)]
struct ReCompileArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Argument shape for `re.escape(pattern)`.
///
/// `pattern` stays a plain `Value`: CPython's `escape` is a str-only helper
/// whose non-str error (the `decoding to str: …` fallback) differs from the
/// pattern wording [`PatternArg`] produces.
#[derive(FromArgs)]
#[from_args(name = "escape", style = def)]
struct ReEscapeArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
}

/// Argument shape for `re.search(pattern, string, flags=0)`; `re.match`,
/// `re.fullmatch`, `re.findall`, and `re.finditer` share it under their own
/// names below (the function name is baked into each struct's errors).
///
/// See `ReSubArgs` for why `pattern` / `string` use `static_string`.
#[derive(FromArgs)]
#[from_args(name = "search", style = def)]
struct ReSearchArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Argument shape for `re.match(pattern, string, flags=0)` — see [`ReSearchArgs`].
#[derive(FromArgs)]
#[from_args(name = "match", style = def)]
struct ReMatchArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Argument shape for `re.fullmatch(pattern, string, flags=0)` — see [`ReSearchArgs`].
#[derive(FromArgs)]
#[from_args(name = "fullmatch", style = def)]
struct ReFullmatchArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Argument shape for `re.findall(pattern, string, flags=0)` — see [`ReSearchArgs`].
#[derive(FromArgs)]
#[from_args(name = "findall", style = def)]
struct ReFindallArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Argument shape for `re.finditer(pattern, string, flags=0)` — see [`ReSearchArgs`].
#[derive(FromArgs)]
#[from_args(name = "finditer", style = def)]
struct ReFinditerArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}

/// Validates the `string` subject argument and borrows its text from the heap.
///
/// Runs *after* signature binding and pattern/flags resolution — CPython's
/// `def` binds arguments without type checks and only the C match machinery
/// rejects a bad subject — so arity, pattern, and flags errors always win.
/// Borrowing (rather than copying) keeps the often-large subject zero-copy on
/// the heap for the duration of the match. Monty has no bytes matching, so a
/// bytes subject gets CPython's mixed-types message and anything else gets
/// the `sre` "expected string or bytes-like object" wording.
fn subject_str<'a>(value: &'a Value, vm: &'a VM<'_, impl ResourceTracker>) -> RunResult<&'a str> {
    if value.is_str(vm.heap) {
        value.to_str(vm)
    } else if value.py_type_heap(vm.heap) == Type::Bytes {
        // Monty patterns are always str, so a bytes subject is always
        // CPython's string-pattern/bytes-subject mismatch.
        Err(ExcType::type_error(
            "cannot use a string pattern on a bytes-like object",
        ))
    } else {
        // sre reports `type(x).__name__`, so `None` reads 'NoneType'.
        Err(ExcType::type_error(format!(
            "expected string or bytes-like object, got '{}'",
            value.py_type_name(vm)
        )))
    }
}

/// The `pattern` argument for the module-level `re` functions: either an
/// owned pattern string ([`RePattern::compile`] stores it), or a still-live
/// compiled `re.Pattern` heap value, which CPython's `_compile` passes
/// through unchanged. Anything else — including `bytes`, since Monty has no
/// bytes patterns — gets CPython's `first argument must be …` error.
///
/// Coerced in the function body (via [`resolve_pattern`]) rather than as a
/// `FromArgs` field type: CPython's `def` binding never type-checks, so a
/// coercion failure at extraction time would wrongly preempt arity errors
/// (`re.search(123)` must report the missing `string`, not the bad pattern).
pub(crate) enum PatternArg {
    /// A `str` pattern, copied out because compilation stores it.
    Str(String),
    /// An already-compiled `re.Pattern` heap value (ownership transferred in;
    /// dropped by [`resolve_pattern`]'s error path, `drop_with_heap`, or the
    /// eventual consumer).
    Compiled(Value),
}

impl PatternArg {
    /// Coerces the raw `pattern` value, consuming it on every path (the
    /// `Compiled` variant transfers ownership in rather than dropping).
    fn extract(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        if let Some(either) = value.as_either_str(vm.heap) {
            let pattern = either.into_string(vm.interns);
            value.drop_with_heap(vm);
            Ok(Self::Str(pattern))
        } else if value.py_type_heap(vm.heap) == Type::RePattern {
            Ok(Self::Compiled(value))
        } else {
            value.drop_with_heap(vm);
            Err(ExcType::type_error("first argument must be string or compiled pattern"))
        }
    }
}

impl DropWithHeap for PatternArg {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        if let Self::Compiled(value) = self {
            value.drop_with_heap(heap);
        }
    }
}

/// The `flags` argument: a non-negative integer fitting `u16`, with `bool`
/// accepted as 0/1 (CPython treats bool as an int subclass). Coerced in the
/// function body like [`PatternArg`] so binding-time arity errors always win.
///
/// A non-int value reports CPython's incidental `unsupported operand type(s)
/// for &` error — what its `parse` raises when it first ANDs the flags —
/// so the message matches byte-for-byte. Out-of-range ints keep Monty's
/// clearer wording (CPython raises `ValueError`/`OverflowError` there; see
/// `limitations/re.md`).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ReFlags(u16);

impl ReFlags {
    /// Coerces the raw `flags` value, consuming it on every path.
    fn extract(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let result = match value {
            Value::Int(n) => u16::try_from(n)
                .map(Self)
                .map_err(|_| ExcType::type_error("flags must be a non-negative integer")),
            Value::Bool(b) => Ok(Self(u16::from(b))),
            _ => Err(ExcType::binary_type_error(
                "&",
                value.py_type_heap(vm.heap),
                value.py_type_name(vm),
                "int",
            )),
        };
        value.drop_with_heap(vm);
        result
    }
}

impl ReFlags {
    /// The raw flags bits.
    fn get(self) -> u16 {
        self.0
    }
}

/// A pattern ready to run: shared out of the [`RePatternCache`], or a still-live
/// `re.Pattern` heap value (which callers keep alive with `defer_drop!`).
enum ResolvedPattern {
    /// Compiled from a `str` pattern and shared (`Rc`) with the pattern cache
    /// (or uncached when its compiled form exceeds [`CACHED_SIZE_LIMIT`]).
    Cached(Rc<RePattern>),
    /// A live `re.Pattern` heap value (always a `Value::Ref` to
    /// `HeapData::RePattern`, guaranteed by [`PatternArg::extract`]).
    Heap(Value),
}

impl ResolvedPattern {
    /// Borrows the compiled pattern (from the heap for the `Heap` variant).
    fn get<'a>(&'a self, heap: &'a Heap<impl ResourceTracker>) -> &'a RePattern {
        match self {
            Self::Cached(pattern) => pattern,
            Self::Heap(value) => {
                let Value::Ref(heap_id) = value else {
                    unreachable!("ResolvedPattern::Heap always holds a heap ref")
                };
                match heap.get(*heap_id) {
                    HeapData::RePattern(pattern) => pattern,
                    _ => unreachable!("ResolvedPattern::Heap always points at a re.Pattern"),
                }
            }
        }
    }
}

impl DropWithHeap for ResolvedPattern {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        if let Self::Heap(value) = self {
            value.drop_with_heap(heap);
        }
        // `Cached` holds only an `Rc<RePattern>` (no heap references); it drops here.
    }
}

/// One slot of [`RePatternCache`], holding the key hash, the pattern text, the
/// flags (text + flags are rechecked on a hash hit to rule out collisions), and
/// the compiled pattern shared with callers.
type ReCacheEntry = Option<(u64, Box<str>, u16, Rc<RePattern>)>;

/// Slot count of [`RePatternCache`] — power of two so the index modulo is a mask;
/// 256 (vs CPython's 512 `re._MAXCACHE`) to halve the untracked worst-case retained
/// memory of `CACHE_CAPACITY × CACHED_SIZE_LIMIT`.
const CACHE_CAPACITY: usize = 256;

/// `delegate_size_limit` for compiles that will be retained in the cache. The cache
/// is invisible to the `ResourceTracker`, so this caps its worst-case footprint at
/// ~`CACHE_CAPACITY × CACHED_SIZE_LIMIT`; patterns whose compiled form is bigger
/// still work — they are recompiled per call at default limits and never retained.
const CACHED_SIZE_LIMIT: usize = 64 * 1024;

/// Fixed-size, per-run cache of compiled patterns for module-level `re.*` calls,
/// keyed on `(pattern, flags)` — so `re.split(r'\s+', text)` in a loop compiles
/// once, not per call. Direct-mapped with 5-slot linear probing and
/// LRU-on-collision (jiter's `py_string_cache` design), so retained
/// compiled-regex memory is hard-bounded by `CACHE_CAPACITY` entries of at most
/// [`CACHED_SIZE_LIMIT`] each. Mirrors CPython's `re._cache`; not snapshotted
/// (rebuilt on demand).
///
/// The `CACHE_CAPACITY`-slot backing store is allocated lazily on first use:
/// every VM constructs a `RePatternCache`, but the vast majority never touch
/// `re`, and eagerly zeroing ~`CACHE_CAPACITY × size_of::<ReCacheEntry>()` bytes
/// per VM measurably regressed setup-bound benchmarks (`add_two`, `func_call_*`).
#[derive(Default)]
pub(crate) struct RePatternCache(Option<(Box<[ReCacheEntry]>, RandomState)>);

impl RePatternCache {
    /// Returns the compiled pattern for `(pattern, flags)`, compiling and caching
    /// on a miss. Compile errors propagate and nothing is cached; patterns whose
    /// compiled form exceeds [`CACHED_SIZE_LIMIT`] are returned uncached. The
    /// backing store is allocated here on the first call.
    fn get_or_compile(&mut self, pattern: &str, flags: u16) -> RunResult<Rc<RePattern>> {
        let (entries, hash_builder) = self
            .0
            .get_or_insert_with(|| (vec![None; CACHE_CAPACITY].into_boxed_slice(), RandomState::default()));

        let hash = hash_builder.hash_one((pattern, flags));
        // `hash % CACHE_CAPACITY` is < CACHE_CAPACITY, so it always fits usize.
        let hash_index = usize::try_from(hash % CACHE_CAPACITY as u64).expect("index < CACHE_CAPACITY");

        // Probe up to 5 contiguous slots for a match, remembering the first
        // empty slot to fill on a miss.
        let mut empty_slot = None;
        for index in hash_index..hash_index + 5 {
            match entries.get(index) {
                Some(Some((entry_hash, entry_pattern, entry_flags, compiled))) => {
                    if *entry_hash == hash && *entry_flags == flags && &**entry_pattern == pattern {
                        return Ok(Rc::clone(compiled));
                    }
                }
                // First empty slot — a miss lands here.
                Some(None) => {
                    empty_slot = Some(index);
                    break;
                }
                // Ran past the end of the array.
                None => break,
            }
        }

        // Miss: compile size-bounded so a retained entry can never pin a huge
        // compiled regex. A valid pattern that compiles past the cap is
        // recompiled at default limits and returned uncached — it still works,
        // just recompiled per call.
        let compiled = match RePattern::compile_bounded(pattern.to_owned(), flags, CACHED_SIZE_LIMIT) {
            Ok(compiled) => compiled,
            Err(BoundedCompileError::TooBig) => {
                return Ok(Rc::new(RePattern::compile(pattern.to_owned(), flags)?));
            }
            Err(BoundedCompileError::Invalid(err)) => return Err(err),
        };

        // Fill the empty slot, else evict `hash_index` (LRU-on-collision) when
        // every probed slot was occupied.
        let compiled = Rc::new(compiled);
        let slot = empty_slot.unwrap_or(hash_index);
        entries[slot] = Some((hash, Box::from(pattern), flags, Rc::clone(&compiled)));
        Ok(compiled)
    }
}

/// Coerces the raw `pattern` / `flags` argument values and applies CPython's
/// `_compile` rules: string patterns compile with the given flags;
/// already-compiled patterns pass through but reject non-zero flags with
/// CPython's `ValueError`.
///
/// Coercion happens here — after `from_args` has fully bound the signature —
/// and in CPython's order: pattern type first, then flags type, then the
/// compiled-pattern flags rejection, then pattern compilation.
fn resolve_pattern(pattern: Value, flags: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<ResolvedPattern> {
    // Sequential hand-rolled cleanup: each coercion consumes its value, so on
    // failure only the *other*, not-yet-consumed value needs dropping.
    let pattern = match PatternArg::extract(pattern, vm) {
        Ok(pattern) => pattern,
        Err(e) => {
            flags.drop_with_heap(vm);
            return Err(e);
        }
    };
    let flags = match ReFlags::extract(flags, vm) {
        Ok(flags) => flags,
        Err(e) => {
            pattern.drop_with_heap(vm);
            return Err(e);
        }
    };
    match pattern {
        PatternArg::Str(pattern) => Ok(ResolvedPattern::Cached(
            vm.re_pattern_cache.get_or_compile(&pattern, flags.get())?,
        )),
        PatternArg::Compiled(value) => {
            if flags.get() == 0 {
                Ok(ResolvedPattern::Heap(value))
            } else {
                value.drop_with_heap(vm);
                Err(ExcType::value_error(
                    "cannot process flags argument with a compiled pattern",
                ))
            }
        }
    }
}

/// `re.finditer(pattern, string, flags=0)` — return all matches as a list.
///
/// Eagerly collects all match objects into a list. When the user iterates with
/// `for m in re.finditer(...)`, the VM's `GetIter` opcode handles iteration
/// over the returned list automatically.
fn call_finditer(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReFinditerArgs { pattern, string, flags } = ReFinditerArgs::from_args(args, vm)?;
    defer_drop!(string, vm);
    let resolved = resolve_pattern(pattern, flags, vm)?;
    defer_drop!(resolved, vm);
    resolved
        .get(vm.heap)
        .finditer(string, subject_str(string, vm)?, vm.heap)
}

/// `re.escape(pattern)` — escape special regex characters in a string.
///
/// Returns a string with all regex metacharacters and whitespace prefixed with
/// a backslash. Only characters that have special meaning in regex patterns are
/// escaped, matching CPython 3.7+ behavior.
///
/// Escaped characters: `\t \n \v \f \r   # $ & ( ) * + - . ? [ \ ] ^ { | } ~`
fn call_escape(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReEscapeArgs { pattern } = ReEscapeArgs::from_args(args, vm)?;
    defer_drop!(pattern, vm);
    let Ok(text) = pattern.to_str(vm) else {
        // CPython's escape() falls back to `str(pattern, 'latin1')` for
        // non-str input, so this is the (incidental) message it raises.
        let t = pattern.py_type_name(vm);
        return Err(ExcType::type_error(format!(
            "decoding to str: need a bytes-like object, {t} found"
        )));
    };

    let mut result = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if should_escape(c) {
            result.push('\\');
        }
        result.push(c);
    }

    Ok(allocate_string(result, vm.heap)?)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts the occupied slots in the cache (0 while unallocated).
    fn occupied(cache: &RePatternCache) -> usize {
        cache
            .0
            .as_ref()
            .map_or(0, |(entries, _)| entries.iter().filter(|e| e.is_some()).count())
    }

    #[test]
    fn hit_shares_one_entry() {
        let mut cache = RePatternCache::default();
        let first = cache.get_or_compile(r"\s+", 0).unwrap();
        let second = cache.get_or_compile(r"\s+", 0).unwrap();
        // A hit returns the *same* compiled pattern, not a recompile.
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(occupied(&cache), 1);
    }

    #[test]
    fn flags_key_the_entry() {
        let mut cache = RePatternCache::default();
        let plain = cache.get_or_compile("abc", 0).unwrap();
        let ignorecase = cache.get_or_compile("abc", IGNORECASE).unwrap();
        // Same pattern text but different flags are distinct entries.
        assert!(!Rc::ptr_eq(&plain, &ignorecase));
        assert_eq!(occupied(&cache), 2);
    }

    #[test]
    fn compile_error_caches_nothing() {
        let mut cache = RePatternCache::default();
        // Unbalanced parenthesis fails to compile.
        assert!(cache.get_or_compile("(", 0).is_err());
        assert_eq!(occupied(&cache), 0);
    }

    /// A counted repeat that expands far past [`CACHED_SIZE_LIMIT`] in the
    /// delegated regex compiler while still compiling fine at default limits.
    const OVERSIZE_PATTERN: &str = "a{5000}";

    #[test]
    fn oversize_pattern_compiles_but_is_not_retained() {
        let mut cache = RePatternCache::default();
        let first = cache.get_or_compile(OVERSIZE_PATTERN, 0).unwrap();
        let second = cache.get_or_compile(OVERSIZE_PATTERN, 0).unwrap();
        // Both calls succeed but nothing is retained: each call recompiles.
        assert!(!Rc::ptr_eq(&first, &second));
        assert_eq!(occupied(&cache), 0);
    }

    #[test]
    fn oversize_pattern_does_not_disturb_cached_entries() {
        let mut cache = RePatternCache::default();
        let small = cache.get_or_compile("abc", 0).unwrap();
        cache.get_or_compile(OVERSIZE_PATTERN, 0).unwrap();
        let again = cache.get_or_compile("abc", 0).unwrap();
        // The small pattern's entry survives the uncached oversize compile.
        assert!(Rc::ptr_eq(&small, &again));
        assert_eq!(occupied(&cache), 1);
    }

    #[test]
    fn occupancy_is_bounded_by_capacity() {
        let mut cache = RePatternCache::default();
        // Far more distinct patterns than slots: LRU-on-collision must keep
        // the occupied count hard-bounded rather than growing unboundedly.
        for i in 0..CACHE_CAPACITY * 4 {
            cache.get_or_compile(&format!("pat{i}"), 0).unwrap();
        }
        assert!(occupied(&cache) <= CACHE_CAPACITY);
    }
}
