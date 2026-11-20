# `re` module

Monty's `re` module is backed by the Rust `fancy-regex` crate, not
CPython's regex engine.
Patterns are parsed with CPython's grammar first, so patterns CPython's parser rejects raise CPython's messages.
The differences below are in what the engine can compile and how it matches.

## Module functions

Implemented: `compile`, `search`, `match`, `fullmatch`, `findall`, `sub`,
`split`, `finditer`, `escape`.

Not implemented: `subn`, `purge`, `template`. The pre-compiled `re._compile`
internal is not exposed.

`re.finditer` and `Pattern.finditer` return a `list` of matches, not CPython's lazy `callable_iterator`:

- Every match is found when `finditer` is called, so an error a later match hits (such as the backtracking limit)
    is raised by the call itself rather than by the iteration that reaches it.
- `next(re.finditer(...))` raises `TypeError: 'list' object is not an iterator`.
- The result can be iterated more than once, where CPython's iterator is empty the second time.

The module-level functions accept the same positional-or-keyword arguments
as CPython (including compiled `re.Pattern` objects as the first argument,
with `re.compile(p) is p` and the `cannot process flags argument with a compiled pattern` `ValueError`), and their
signature/type error messages
match CPython's, with these divergences:

- `bytes` patterns and subjects are not supported (Monty has no bytes
    matching). A bytes *subject* raises CPython's mixed-types message
    (`cannot use a string pattern on a bytes-like object`); a bytes *pattern*
    raises `first argument must be string or compiled pattern`, whereas
    CPython supports bytes patterns.
- A non-str `repl` for `re.sub` raises `callable replacement is not yet supported in re.sub()`; CPython raises
    `decoding to str: need a bytes-like object, int found`-style errors for non-callable non-strings.
    Like CPython, the check runs even when a negative `count` means zero
    substitutions.
- `re.sub` replacement templates are not generally validated: CPython parses
    the template eagerly (even with zero matches or a negative `count`) and
    raises `PatternError` for an invalid group reference (`\2` with one
    group: `invalid group reference 2 at position 1`) or an unknown escape
    (`\q`: `bad escape \q at position 0`). Monty rejects three-digit octal
    escapes above `0o377`, expands references to missing groups as the empty
    string and passes unknown escapes through literally.
- A negative or `> 0xFFFF` integer `flags` value raises
    `TypeError: flags must be a non-negative integer`; CPython accepts larger
    int-sized values and handles negative values according to the resulting flag
    bits.
- Positional `count` / `maxsplit` for `re.sub` / `re.split` do not emit
    CPython 3.13+'s `DeprecationWarning` (Monty has no warnings machinery).

## Flags

Supported: `NOFLAG`, `IGNORECASE` / `I`, `MULTILINE` / `M`, `DOTALL` / `S`,
`ASCII` / `A`.

`ASCII` is accepted but has no effect: `\w`, `\d` and `\s` stay Unicode-aware, with or without `(?a)`.

Not implemented: `VERBOSE` / `X`, `LOCALE` / `L`, `DEBUG`, `UNICODE` / `U`
(Unicode is always on). `re.VERBOSE` / `re.X` are not exposed, but the inline `(?x)` flag and the bit value `64`
enable verbose mode.
Unknown flag bits within the accepted `u16` range are
silently accepted; bits above that range are rejected by the integer range
check.

## `re.Pattern` objects

Attributes: `pattern`, `flags`.
Methods: `search`, `match`, `fullmatch`, `findall`, `sub`, `split`,
`finditer`.

Not implemented: `subn`, `groups` (count), `groupindex` (named-group
mapping), `scanner`. The `pos` / `endpos` arguments accepted by
`Pattern.search(string, pos, endpos)` etc. in CPython are **not** supported.

A non-str subject passed to a Pattern *method* raises `expected string, not {type}` rather than CPython's
`expected string or bytes-like object, got '{type}'`. The module-level functions match CPython's wording.

## `re.Match` objects

Attributes: `re`, `string`.
Methods: `group`, `groups`, `groupdict`, `start`, `end`, `span`.

Not implemented: `lastindex`, `lastgroup`, `expand`, `pos`, `endpos`,
`regs`. Indexing (`m[0]`, `m["name"]`) is not supported; use `.group()`.

## `re.PatternError` / `re.error`

Raised for invalid regex patterns. Unlike CPython, `pattern`, `pos`,
`lineno`, and `colno` attributes are not populated: `fancy-regex`'s error
representation does not carry them.

## Engine-level differences

These valid CPython patterns raise `re.PatternError` with a message from `fancy-regex`:

- A quantifier on a lookaround or an empty group, such as `(?=a)*` or `(?:)*`.
- Patterns whose *compiled* form is very large, e.g. huge counted repeats
    like `a{5000000}` (`fancy-regex`/`regex` enforce a compiled-size limit).

Groups nested 64 or more deep raise `re.PatternError: pattern too deeply nested`; CPython accepts a few hundred
levels.

`\s` follows Unicode's `White_Space` property, so it does not match `\x1c`–`\x1f`, which CPython's `\s` does.
Case-insensitive backreferences compare by Unicode case folding, so `(?i)(σ)\1` matches `Σς`, which CPython does not.

An escape for a surrogate (`\ud800`) never matches, since Monty strings cannot contain surrogates.
