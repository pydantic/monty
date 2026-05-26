# `re` module

Monty's `re` module is backed by the Rust `fancy-regex` crate, not
CPython's regex engine. Most patterns behave identically, but the
underlying engine differs in syntax extensions and error reporting.

## Module functions

Implemented: `compile`, `search`, `match`, `fullmatch`, `findall`, `sub`,
`split`, `finditer`, `escape`.

Not implemented: `subn`, `purge`, `template`. The pre-compiled `re._compile`
internal is not exposed.

## Flags

Supported: `NOFLAG`, `IGNORECASE` / `I`, `MULTILINE` / `M`, `DOTALL` / `S`,
`ASCII` / `A`.

Not implemented: `VERBOSE` / `X`, `LOCALE` / `L`, `DEBUG`, `UNICODE` / `U`
(Unicode is always on). Passing an unknown flag bit is silently accepted.

## `re.Pattern` objects

Attributes: `pattern`, `flags`.
Methods: `search`, `match`, `fullmatch`, `findall`, `sub`, `split`,
`finditer`.

Not implemented: `subn`, `groups` (count), `groupindex` (named-group
mapping), `scanner`. The `pos` / `endpos` arguments accepted by
`Pattern.search(string, pos, endpos)` etc. in CPython are **not** supported.

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
