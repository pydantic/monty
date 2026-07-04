# `str.encode()` / `bytes.decode()`

Monty implements a fixed, small set of text codecs rather than the full
`codecs`/`encodings` registry CPython ships.

## Supported codecs

- `utf-8`
- `ascii`
- `utf-16`, `utf-16-le`, `utf-16-be`
- `utf-32`, `utf-32-le`, `utf-32-be`

Names are normalized like CPython (case-insensitive; runs of spaces/hyphens
collapse to `_`) and each codec's CPython aliases are recognized (`utf8`,
`u16`, `646`, `us-ascii`, ...). Any other encoding name raises
`LookupError: unknown encoding: {name}` — including names CPython recognizes
(`latin-1`, `cp1252`, `iso-8859-1`, `big5`, ...).

## Byte order for bare `utf-16` / `utf-32`

CPython's bare `utf-16`/`utf-32` codecs use the *platform's native* byte
order: encode writes a BOM in native order, and BOM-less input decodes as
native order. Monty always uses **little-endian** for both, so behavior is
identical on every little-endian host (all platforms Monty CI covers) but
diverges on big-endian hosts. Input with a BOM decodes identically
everywhere (the BOM's order wins).

## Error handlers

- **`bytes.decode(..., errors='surrogateescape')` raises
  `NotImplementedError`** when a byte actually needs handling: CPython maps
  undecodable bytes to lone surrogates (U+DC80–U+DCFF), which Monty's
  strict-UTF-8 strings cannot contain.
- **`errors='surrogatepass'` on decode raises `NotImplementedError`** in the
  cases where CPython would produce a lone surrogate (a CESU-8 surrogate
  triple in UTF-8 input; a lone surrogate unit/code point in UTF-16/32
  input). For any other invalid input it re-raises the strict
  `UnicodeDecodeError`, matching CPython.
- Custom handlers registered via `codecs.register_error` do not exist
  (there is no `codecs` module); any name outside CPython's built-in set
  raises `LookupError: unknown error handler name '{name}'`.
- All other built-in handlers behave as in CPython, in both directions.
  `namereplace` output for recently-added code points is subject to the
  Unicode version skew described in [unicodedata.md](unicodedata.md).

## `UnicodeEncodeError` / `UnicodeDecodeError`

Both are message-only, like every other Monty exception — see
[exceptions.md](exceptions.md#constructor-signature). CPython's
`encoding`/`object`/`start`/`end`/`reason` attributes are not exposed.
