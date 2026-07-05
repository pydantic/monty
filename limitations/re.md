# `re` module

Monty's `re` module is backed by the Rust `fancy-regex` crate, not
CPython's regex engine. Most patterns behave identically, but the
underlying engine differs in syntax extensions and error reporting.

## Module functions

Implemented: `compile`, `search`, `match`, `fullmatch`, `findall`, `sub`,
`split`, `finditer`, `escape`.

Not implemented: `subn`, `purge`, `template`. The pre-compiled `re._compile`
internal is not exposed.

The module-level functions accept the same positional-or-keyword arguments
as CPython (including compiled `re.Pattern` objects as the first argument,
with `re.compile(p) is p` and the `cannot process flags argument with a
compiled pattern` `ValueError`), and their signature/type error messages
match CPython's, with these divergences:

- `bytes` patterns and subjects are not supported (Monty has no bytes
  matching). A bytes *subject* raises CPython's mixed-types message
  (`cannot use a string pattern on a bytes-like object`); a bytes *pattern*
  raises `first argument must be string or compiled pattern`, whereas
  CPython supports bytes patterns.
- A non-str `repl` for `re.sub` raises `callable replacement is not yet
  supported in re.sub()`; CPython raises `decoding to str: need a
  bytes-like object, int found`-style errors for non-callable non-strings.
  Like CPython, the check runs even when a negative `count` means zero
  substitutions.
- `re.sub` replacement templates are not validated: CPython parses the
  template eagerly (even with zero matches or a negative `count`) and
  raises `PatternError` for an invalid group reference (`\2` with one
  group: `invalid group reference 2 at position 1`) or an unknown escape
  (`\q`: `bad escape \q at position 0`). Monty expands references to
  missing groups as the empty string and passes unknown escapes through
  literally.
- A negative or `> 0xFFFF` integer `flags` value raises
  `TypeError: flags must be a non-negative integer`; CPython accepts larger
  int-sized values and handles negative values according to the resulting flag
  bits.
- Positional `count` / `maxsplit` for `re.sub` / `re.split` do not emit
  CPython 3.13+'s `DeprecationWarning` (Monty has no warnings machinery).

## Flags

Supported: `NOFLAG`, `IGNORECASE` / `I`, `MULTILINE` / `M`, `DOTALL` / `S`,
`ASCII` / `A`.

Not implemented: `VERBOSE` / `X`, `LOCALE` / `L`, `DEBUG`, `UNICODE` / `U`
(Unicode is always on). Unknown flag bits within the accepted `u16` range are
silently accepted; bits above that range are rejected by the integer range
check.

## `re.Pattern` objects

Attributes: `pattern`, `flags`.
Methods: `search`, `match`, `fullmatch`, `findall`, `sub`, `split`,
`finditer`.

Not implemented: `subn`, `groups` (count), `groupindex` (named-group
mapping), `scanner`. The `pos` / `endpos` arguments accepted by
`Pattern.search(string, pos, endpos)` etc. in CPython are **not** supported.

A non-str subject passed to a Pattern *method* raises `expected string, not
{type}` rather than CPython's `expected string or bytes-like object, got
'{type}'` (the module-level functions match CPython's wording).

## `re.Match` objects

Attributes: `re`, `string`.
Methods: `group`, `groups`, `groupdict`, `start`, `end`, `span`.

Not implemented: `lastindex`, `lastgroup`, `expand`, `pos`, `endpos`,
`regs`. Indexing (`m[0]`, `m["name"]`) is not supported — use `.group()`.

## `re.PatternError` / `re.error`

Raised for invalid regex patterns. Unlike CPython, `pattern`, `pos`,
`lineno`, and `colno` attributes are not populated — `fancy-regex`'s error
representation does not carry them.

## Engine-level differences

- Unsupported regex features (some Unicode property escapes, some
  CPython-specific extensions) raise `re.PatternError` at compile time.
- Backreference syntax `\10` and higher is not recognized; only `\1`–`\9`.
- Error messages for invalid patterns come from `fancy-regex` and do not
  match CPython's wording.
