//! The `fnmatch` module: shell-style pattern matching.
//!
//! A pattern is compiled once per call into [`Token`]s by the algorithm of
//! CPython's `fnmatch.translate()`, so the odd corners of its character-class
//! handling (`[a--]` never matches, `[!b-a]` matches any character) carry over.
//! Names are matched against the tokens directly rather than through `re`;
//! `translate()` renders the same tokens as the regex text CPython 3.14 writes.
//!
//! The sandbox's `os.path` is `posixpath`, whose `normcase` is the identity, so
//! `fnmatch()` and `fnmatchcase()` differ only in how they convert their arguments.

use monty_types::{ResourceError, ResourceTracker};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os_dispatch::{value_as_bytes, value_as_path_str},
    types::{List, Module, str::allocate_string},
    value::Value,
};

/// The module's four functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum FnmatchFunctions {
    Fnmatch,
    Fnmatchcase,
    Filter,
    Translate,
}

/// Creates the `fnmatch` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Fnmatch, vm.interns);
    for (name, function) in [
        (StaticStrings::Fnmatch, FnmatchFunctions::Fnmatch),
        (StaticStrings::Fnmatchcase, FnmatchFunctions::Fnmatchcase),
        (StaticStrings::Filter, FnmatchFunctions::Filter),
        (StaticStrings::Translate, FnmatchFunctions::Translate),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Fnmatch(function)), vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to one of the module's functions.
pub(super) fn call(vm: &mut VM<'_>, function: FnmatchFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        FnmatchFunctions::Fnmatch => {
            let FnmatchArgs { name, pat } = FnmatchArgs::from_args(args, vm)?;
            defer_drop!(name, vm);
            defer_drop!(pat, vm);
            // `normcase` runs `os.fspath` on both, so a `Path` is accepted and a bad type gets its wording.
            let name = Text::from_path_like(name, vm)?;
            let pat = Text::from_path_like(pat, vm)?;
            Ok(Value::Bool(fnmatchcase(&name, &pat, &vm.heap.tracker)?))
        }
        FnmatchFunctions::Fnmatchcase => {
            let FnmatchcaseArgs { name, pat } = FnmatchcaseArgs::from_args(args, vm)?;
            defer_drop!(name, vm);
            defer_drop!(pat, vm);
            // The pattern compiles first, then the subject gets `re`'s wording.
            let pat = Text::from_pattern(pat, vm)?;
            let name = Text::from_subject(name, vm)?;
            Ok(Value::Bool(fnmatchcase(&name, &pat, &vm.heap.tracker)?))
        }
        FnmatchFunctions::Filter => {
            let FilterArgs { names, pat } = FilterArgs::from_args(args, vm)?;
            filter(names, pat, vm)
        }
        FnmatchFunctions::Translate => {
            let TranslateArgs { pat } = TranslateArgs::from_args(args, vm)?;
            defer_drop!(pat, vm);
            let pat = Text::from_pattern_str(pat, vm)?;
            Ok(allocate_string(translate(&compile(&pat)), vm.heap))
        }
    }
}

/// Argument shape for `fnmatch(name, pat)`.
#[derive(FromArgs)]
#[from_args(name = "fnmatch", style = def)]
struct FnmatchArgs {
    name: Value,
    pat: Value,
}

/// Argument shape for `fnmatchcase(name, pat)`.
#[derive(FromArgs)]
#[from_args(name = "fnmatchcase", style = def)]
struct FnmatchcaseArgs {
    name: Value,
    pat: Value,
}

/// Argument shape for `filter(names, pat)`.
#[derive(FromArgs)]
#[from_args(name = "filter", style = def)]
struct FilterArgs {
    names: Value,
    pat: Value,
}

/// Argument shape for `translate(pat)`.
#[derive(FromArgs)]
#[from_args(name = "translate", style = def)]
struct TranslateArgs {
    pat: Value,
}

/// `filter(names, pat)`: the members of `names` that match, in order and unchanged.
fn filter(names: Value, pat: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    // `names` is only consumed once the pattern binds, so a bad pattern wins over a bad iterable.
    let mut names_guard = DropGuard::new(names, vm);
    let (_, vm) = names_guard.as_parts_mut();
    let pat = {
        defer_drop!(pat, vm);
        Text::from_path_like(pat, vm)?
    };
    let (names, vm) = names_guard.into_parts();
    let tokens = compile(&pat.chars);
    let iter = names.into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    let mut out_guard = DropGuard::new(Vec::new(), vm);
    let (out, vm) = out_guard.as_parts_mut();
    let mut seen = 0;
    while let Some(item) = iter.py_next(vm)? {
        let mut item_guard = DropGuard::new(item, vm);
        let (item, vm) = item_guard.as_parts_mut();
        // The source may yield without running bytecode, and every name is copied,
        // so the drain polls the limits itself as `list()` does.
        vm.heap.tracker.check_memory_time_every(seen)?;
        seen += 1;
        let name = Text::from_subject(item, vm)?;
        if matches(&tokens, &pattern_subject(&name, &pat)?.chars, &vm.heap.tracker)? {
            out.push(item_guard.into_inner());
        }
    }
    let (out, vm) = out_guard.into_parts();
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(out)))))
}

/// `fnmatchcase(name, pat)` on converted arguments.
fn fnmatchcase(name: &Text, pat: &Text, tracker: &ResourceTracker) -> RunResult<bool> {
    let name = pattern_subject(name, pat)?;
    Ok(matches(&compile(&pat.chars), &name.chars, tracker)?)
}

/// Checks that `name` and `pat` are both `str` or both `bytes`, raising `re`'s
/// mixed-types errors otherwise, and lends `name` back.
fn pattern_subject<'a>(name: &'a Text, pat: &Text) -> RunResult<&'a Text> {
    match (name.is_bytes, pat.is_bytes) {
        (true, false) => Err(ExcType::type_error(
            "cannot use a string pattern on a bytes-like object",
        )),
        (false, true) => Err(ExcType::type_error(
            "cannot use a bytes pattern on a string-like object",
        )),
        _ => Ok(name),
    }
}

/// A `str` or `bytes` argument as code points: bytes are read as Latin-1, as
/// CPython's `fnmatch` decodes them, so one matcher serves both.
struct Text {
    chars: Vec<char>,
    is_bytes: bool,
}

impl Text {
    /// `os.fspath(value)`: `str`, `bytes` or a path object, with `fspath`'s error.
    fn from_path_like(value: &Value, vm: &VM<'_>) -> RunResult<Self> {
        Self::extract(value, vm).ok_or_else(|| ExcType::type_error_fspath(&value.py_type_name(vm)))
    }

    /// A subject handed straight to the compiled pattern: `str` or `bytes`, with
    /// `re`'s error for anything else (a path object included).
    fn from_subject(value: &Value, vm: &VM<'_>) -> RunResult<Self> {
        Self::extract_text(value, vm).ok_or_else(|| {
            ExcType::type_error(format!(
                "expected string or bytes-like object, got '{}'",
                value.py_type_name(vm)
            ))
        })
    }

    /// `fnmatchcase`'s pattern, which CPython reads with `len()` first: `str` or
    /// `bytes`, else `object of type 'X' has no len()`.
    fn from_pattern(value: &Value, vm: &VM<'_>) -> RunResult<Self> {
        Self::extract_text(value, vm).ok_or_else(|| ExcType::type_error_no_len(&value.py_type_name(vm)))
    }

    /// `translate`'s pattern, which must be `str`: CPython indexes a `bytes`
    /// pattern into ints and fails decoding the first one.
    fn from_pattern_str(value: &Value, vm: &VM<'_>) -> RunResult<Vec<char>> {
        let text = Self::from_pattern(value, vm)?;
        if text.is_bytes && !text.chars.is_empty() {
            Err(ExcType::type_error(
                "decoding to str: need a bytes-like object, int found",
            ))
        } else {
            Ok(text.chars)
        }
    }

    /// `str`, `bytes` or path-like text, else `None`.
    fn extract(value: &Value, vm: &VM<'_>) -> Option<Self> {
        value_as_path_str(value, vm.heap, vm.interns)
            .map(Self::str)
            .or_else(|| value_as_bytes(value, vm.heap, vm.interns).map(Self::bytes))
    }

    /// `str` or `bytes` text only, else `None`.
    fn extract_text(value: &Value, vm: &VM<'_>) -> Option<Self> {
        if value.is_str(vm.heap) {
            value_as_path_str(value, vm.heap, vm.interns).map(Self::str)
        } else {
            value_as_bytes(value, vm.heap, vm.interns).map(Self::bytes)
        }
    }

    fn str(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            is_bytes: false,
        }
    }

    fn bytes(bytes: &[u8]) -> Self {
        Self {
            chars: bytes.iter().map(|&byte| char::from(byte)).collect(),
            is_bytes: true,
        }
    }
}

/// One element of a compiled pattern; `translate()` renders each the way
/// CPython's `_translate` does.
#[derive(Debug, PartialEq, Eq)]
enum Token {
    /// A character matched verbatim.
    Literal(char),
    /// `?`: any single character, newline included.
    AnyChar,
    /// `*`: any run of characters; consecutive stars collapse into one.
    Star,
    /// `[...]` that CPython reduces to an empty range, which never matches.
    Never,
    /// `[...]` whose members CPython reduces to `!` alone, matching any character.
    NegatedEmpty,
    /// A character class.
    Class(Class),
}

/// The members of a `[...]` class and the text CPython writes for it.
#[derive(Debug, PartialEq, Eq)]
struct Class {
    negated: bool,
    chars: Vec<char>,
    /// Inclusive ranges, `lo <= hi`.
    ranges: Vec<(char, char)>,
    /// The regex body between the brackets, `^` prefix included.
    rendered: String,
}

impl Class {
    /// Whether `c` is a member, negation applied.
    fn contains(&self, c: char) -> bool {
        let member = self.chars.contains(&c) || self.ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&c));
        member != self.negated
    }
}

/// Compiles a pattern by CPython's `_translate` algorithm.
fn compile(pat: &[char]) -> Vec<Token> {
    let n = pat.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < n {
        let c = pat[i];
        i += 1;
        match c {
            '*' => {
                if tokens.last() != Some(&Token::Star) {
                    tokens.push(Token::Star);
                }
            }
            '?' => tokens.push(Token::AnyChar),
            '[' => {
                // Find the closing bracket: a leading `!` and a leading `]` are members.
                let mut j = i;
                if j < n && pat[j] == '!' {
                    j += 1;
                }
                if j < n && pat[j] == ']' {
                    j += 1;
                }
                while j < n && pat[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    tokens.push(Token::Literal('['));
                } else {
                    tokens.push(compile_class(&pat[i..j]));
                    i = j + 1;
                }
            }
            c => tokens.push(Token::Literal(c)),
        }
    }
    tokens
}

/// Compiles the text between `[` and `]`.
///
/// CPython splits the text at the hyphens that form ranges (`chunks`), drops
/// ranges whose bounds are reversed by merging the chunks around them, then
/// escapes what remains; the members and the rendered text come from the same chunks.
fn compile_class(stuff: &[char]) -> Token {
    let initially_negated = stuff.first() == Some(&'!');
    let chunks = if stuff.contains(&'-') {
        let mut chunks: Vec<Vec<char>> = Vec::new();
        let mut start = 0;
        // The hyphen right after `[` or `[!` is a member, not a range.
        let mut k = if initially_negated { 2 } else { 1 };
        while let Some(offset) = stuff[k.min(stuff.len())..].iter().position(|&c| c == '-') {
            k += offset;
            chunks.push(stuff[start..k].to_vec());
            start = k + 1;
            // Skip the range's end character, and the one after it: a hyphen there is a member.
            k += 3;
        }
        if start < stuff.len() {
            chunks.push(stuff[start..].to_vec());
        } else {
            // A trailing hyphen closes no range and is a member.
            chunks.last_mut().expect("a hyphen was found").push('-');
        }
        // Remove reversed ranges, which a regex rejects, by merging their chunks.
        let mut k = chunks.len() - 1;
        while k > 0 {
            let lo = *chunks[k - 1].last().expect("chunks are never empty before merging");
            let hi = chunks[k][0];
            if lo > hi {
                let tail: Vec<char> = chunks[k][1..].to_vec();
                let merged = &mut chunks[k - 1];
                merged.pop();
                merged.extend(tail);
                chunks.remove(k);
            }
            k -= 1;
        }
        chunks
    } else {
        vec![stuff.to_vec()]
    };
    let negated = chunks.first().and_then(|chunk| chunk.first()) == Some(&'!');

    let mut chars = Vec::new();
    let mut ranges = Vec::new();
    let mut rendered = String::new();
    for (k, chunk) in chunks.iter().enumerate() {
        let mut members = &chunk[..];
        if k > 0 {
            if k == 1 && negated && chunks[0].len() == 1 {
                // A negation marker leaves no range start, so this hyphen is literal.
                chars.push('-');
            } else {
                // The first character ends the range the previous chunk started.
                let hi = chunk[0];
                let lo = chars.pop().expect("a range start was recorded");
                ranges.push((lo, hi));
                members = &members[1..];
            }
            rendered.push('-');
        }
        if k == 0 && negated {
            members = &members[1..];
        }
        chars.extend_from_slice(members);
        for &c in chunk {
            // Backslashes, hyphens and set operators are escaped; everything else is verbatim.
            if matches!(c, '\\' | '-' | '&' | '~' | '|') {
                rendered.push('\\');
            }
            rendered.push(c);
        }
    }

    if rendered.is_empty() {
        Token::Never
    } else if rendered == "!" {
        Token::NegatedEmpty
    } else {
        if negated {
            rendered.replace_range(0..1, "^");
        } else if rendered.starts_with(['^', '[']) {
            rendered.insert(0, '\\');
        }
        Token::Class(Class {
            negated,
            chars,
            ranges,
            rendered,
        })
    }
}

/// Whether the whole of `text` matches `tokens`: a backtracking wildcard match
/// that is linear in the common case, remembering only the latest star.
fn matches(tokens: &[Token], text: &[char], tracker: &ResourceTracker) -> Result<bool, ResourceError> {
    let (mut t, mut s) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    // Backtracking makes this quadratic in the worst case, so the clock is polled.
    let mut steps = 0;
    while s < text.len() {
        tracker.check_time_every(steps)?;
        steps += 1;
        match tokens.get(t) {
            Some(Token::Star) => {
                star = Some((t, s));
                t += 1;
            }
            Some(token) if token_matches(token, text[s]) => {
                t += 1;
                s += 1;
            }
            _ => match star {
                // Let the latest star absorb one more character and retry after it.
                Some((star_t, star_s)) => {
                    t = star_t + 1;
                    s = star_s + 1;
                    star = Some((star_t, star_s + 1));
                }
                None => return Ok(false),
            },
        }
    }
    Ok(tokens[t..].iter().all(|token| *token == Token::Star))
}

/// Whether a single-character token matches `c`.
fn token_matches(token: &Token, c: char) -> bool {
    match token {
        Token::Literal(literal) => *literal == c,
        Token::AnyChar | Token::NegatedEmpty => true,
        Token::Never => false,
        Token::Class(class) => class.contains(c),
        Token::Star => unreachable!("stars are handled by the matcher"),
    }
}

/// Renders `tokens` as CPython's `translate()` does: an interior `*` before
/// fixed text becomes an atomic lazy group, the last one a greedy `.*`.
fn translate(tokens: &[Token]) -> String {
    let mut out = String::from("(?s:");
    let mut parts = tokens.split(|token| *token == Token::Star);
    if let Some(leading) = parts.next() {
        render_fixed(&mut out, leading);
    }
    let mut parts = parts.peekable();
    while let Some(fixed) = parts.next() {
        if parts.peek().is_none() {
            out.push_str(".*");
            render_fixed(&mut out, fixed);
        } else {
            out.push_str("(?>.*?");
            render_fixed(&mut out, fixed);
            out.push(')');
        }
    }
    out.push_str(")\\z");
    out
}

/// Renders a run of tokens without stars.
fn render_fixed(out: &mut String, tokens: &[Token]) {
    for token in tokens {
        match token {
            Token::Literal(c) => {
                // `re.escape` escapes the regex metacharacters and ASCII whitespace.
                if "()[]{}?*+-|^$\\.&~# \t\n\r\u{b}\u{c}".contains(*c) {
                    out.push('\\');
                }
                out.push(*c);
            }
            Token::AnyChar | Token::NegatedEmpty => out.push('.'),
            Token::Never => out.push_str("(?!)"),
            Token::Class(class) => {
                out.push('[');
                out.push_str(&class.rendered);
                out.push(']');
            }
            Token::Star => unreachable!("stars are split off by translate"),
        }
    }
}
