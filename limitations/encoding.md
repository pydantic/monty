# `str.encode()` / `bytes.decode()`

Monty implements a fixed, small set of text codecs rather than the full
`codecs`/`encodings` registry CPython ships.

## Supported codecs

- `utf-8` (aliases `utf8` and `utf_8`), case-insensitive.
- `ascii` (aliases `us-ascii`, `us_ascii`), case-insensitive.

Any other encoding name raises `LookupError: unknown encoding: {name}`, even
names CPython recognizes (`latin-1`, `utf-16`, `cp1252`, `iso-8859-1`, ...).

## Error handlers

- `bytes.decode('ascii', errors='surrogateescape')` raises
  `NotImplementedError` when a byte actually needs handling: CPython maps
  undecodable bytes to lone surrogates (U+DC80–U+DCFF), which Monty's
  strict-UTF-8 strings cannot contain. All other built-in handlers behave
  as in CPython.
- `namereplace` output for recently-added code points is subject to the
  Unicode version skew described in [unicodedata.md](unicodedata.md).
- Custom handlers registered via `codecs.register_error` do not exist
  (there is no `codecs` module); any name outside the built-in set raises
  `LookupError: unknown error handler name '{name}'`.
- `bytes.decode('utf-8', errors=...)` — `errors` is ignored on invalid
  UTF-8 bytes; decoding always behaves as `strict`, raising
  `UnicodeDecodeError` regardless of the requested handler (where CPython
  would apply the handler, or raise `LookupError` for an unknown name),
  and with Monty's generic invalid-UTF-8 wording rather than CPython's
  byte-and-position-specific message.

## `UnicodeEncodeError` / `UnicodeDecodeError`

Both are message-only, like every other Monty exception — see
[exceptions.md](exceptions.md#constructor-signature). CPython's
`encoding`/`object`/`start`/`end`/`reason` attributes are not exposed, and
invalid UTF-8 decode errors use the generic message noted above.
