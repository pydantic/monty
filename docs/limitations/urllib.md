# `urllib` package

Only `urllib.parse` is provided: `import urllib` gives a package whose single attribute is `parse`, and
`urllib.request`, `urllib.error` and `urllib.robotparser` raise `ModuleNotFoundError`.
The sandbox has no network access, so a request module would have nothing to do.

`urllib.parse` provides `urlparse`, `urlunparse`, `urlsplit`, `urlunsplit`, `urljoin`, `urldefrag`, `quote`,
`quote_plus`, `quote_from_bytes`, `unquote`, `unquote_plus`, `unquote_to_bytes`, `urlencode`, `parse_qs` and
`parse_qsl`, with CPython 3.14's algorithms, including the WHATWG control-character stripping, the bracketed IPv6
checks and the NFKC netloc check.

- **URLs are `str` only.** `urlparse(b'...')`, `urlsplit`, `urljoin`, `urldefrag` and the `urlun*` functions
    raise `TypeError: urllib.parse does not support bytes URLs; decode to str first` for a `bytes` argument, where
    CPython returns the `*ResultBytes` variants; `urlparse(None)` fails as a non-string, where CPython treats a
    falsy non-string as empty bytes.
    `quote`, `unquote`, `unquote_to_bytes`, `parse_qs`, `parse_qsl` and `urlencode` accept `bytes` as CPython does.
- **The result objects are named tuples without a class.** `ParseResult`, `SplitResult` and `DefragResult` have
    the fields, `_fields`, `_replace()`, `_asdict()`, `_make()` and `geturl()`, and the first two have `hostname`,
    `port`, `username` and `password`, but `type(result).__name__` is `namedtuple` and the classes themselves are
    not importable, so `isinstance(r, ParseResult)` cannot be written. `_field_defaults`, `encode()` and
    `__class_getitem__` are absent.
- **`None` in a component tuple is an empty string.** `urlunsplit(('http', 'x', None, None, None))` gives
    `http://x` where CPython raises `TypeError` for a `None` path.
- **Encodings are the codecs Monty has.** `quote(..., encoding='latin-1')`, `unquote(..., encoding='latin-1')` and
    the `encoding` arguments of `parse_qs`, `parse_qsl` and `urlencode` raise `LookupError` for any codec outside
    UTF-8, ASCII, UTF-16 and UTF-32 (see [encoding.md](encoding.md)).
- **A bracketed IPv6 host is validated by Rust's address parser**, with CPython's error messages. Both reject the
    same common forms; exotic literals may differ.
- **`parse_qsl` of a falsy non-string returns `[]` silently**, where CPython 3.14 also emits a `DeprecationWarning`.
- **Function metadata follows other built-in Monty module functions.** Their representation uses `<function ...>`
    and the module does not expose `__all__`, the scheme tables (`uses_netloc`, ...) or the deprecated `split*`
    helpers.
