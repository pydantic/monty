# `str.encode()` / `bytes.decode()`

Monty implements a fixed, small set of text codecs rather than the full
`codecs`/`encodings` registry CPython ships.

## Supported codecs

- `utf-8` (aliases `utf8` and `utf_8`), case-insensitive.
- `ascii` (aliases `us-ascii`, `us_ascii`), case-insensitive.

Any other encoding name raises `LookupError: unknown encoding: {name}`, even
names CPython recognizes (`latin-1`, `utf-16`, `cp1252`, `iso-8859-1`, ...).

## Error handlers

`strict`, `ignore`, `replace`, and `backslashreplace` are supported for
`str.encode('ascii', errors=...)` and `bytes.decode('ascii', errors=...)`,
matching CPython's per-character/per-byte behavior.

For the `utf-8` codec:
- `str.encode('utf-8', errors=...)` — `errors` is accepted but never
  consulted, since a Monty `str` is always already valid UTF-8; there is
  nothing for any handler to do.
- `bytes.decode('utf-8', errors=...)` — **`errors` is accepted but ignored
  on invalid UTF-8 bytes; decoding always behaves as `strict`; raising
  `UnicodeDecodeError` regardless of the requested handler.** CPython's
  `ignore`/`replace`/`backslashreplace` handlers for invalid UTF-8 are not
  implemented.

As in CPython, an unrecognized `errors` value is only looked up (and raises
`LookupError: unknown error handler name '{name}'`) if a character/byte
actually needs handling — `'hello'.encode('ascii', 'bogus')` succeeds
because there's nothing for the (invalid) handler to do.

## `UnicodeEncodeError` / `UnicodeDecodeError`

Both are message-only, like every other Monty exception — see
[exceptions.md](exceptions.md#constructor-signature). CPython's
`encoding`/`object`/`start`/`end`/`reason` attributes are not exposed; only
`str(exc)` (the formatted message) matches CPython.
