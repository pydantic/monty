# `json` module

Monty's `json` provides `loads`, `dumps`, `load`, `dump` and the
`JSONDecodeError` exception. Parsing is backed by `jiter`; serialization is a
hand-written encoder matching CPython byte-for-byte for the supported keyword set.

## What's NOT in the module

`json.JSONEncoder` and `json.JSONDecoder` are not implemented, and the `cls=`
keyword is rejected.

## `json.loads(s, **kwargs)`

- Accepts `str` or `bytes` as input.
    `bytes` are always decoded as UTF-8 (a leading BOM is skipped, as CPython's `utf-8-sig` does):
    CPython also detects UTF-16 and UTF-32 input, which Monty rejects with `json.JSONDecodeError`.
- `parse_float` and `parse_int` are the only keyword arguments accepted (`parse_float=Decimal` works).
    Passing any of `cls`, `object_hook`, `parse_constant`, or `object_pairs_hook` raises
    `TypeError: JSONDecoder.__init__() got an unexpected keyword argument '...'`.
    CPython accepts all four and uses this message only for a keyword the decoder does not know.
- `NaN`, `Infinity`, `-Infinity` are *always* accepted, with no toggle to
    reject them.
- Nesting depth is capped at 200 levels; deeper inputs raise
    `json.JSONDecodeError`.
- JSON integers that would exceed Monty's BigInt digit limit are rejected
    with `ValueError` (matching CPython's `int_max_str_digits` behaviour)
    rather than `JSONDecodeError`.

## `json.load(fp)`

`json.load(fp)` is `json.loads(fp.read())`, but accepts no keyword arguments at all:
`parse_float` and `parse_int` raise the same `TypeError` as the other keywords.
A file from `open()` or `Path.open()` is read to EOF through the file's
own buffering, so `load` suspends to the host only when the buffer has not
been loaded yet by a read, `seek()` or iteration step (see [open.md](open.md)).
It sees the same errors as `fp.read()` (`ValueError` on a closed file,
`io.UnsupportedOperation: not readable` on a write-only one).

Any other object has its `read` attribute called synchronously, with the
restrictions of a synchronous call (see [classes.md](classes.md)): a `read`
that calls an external function or does host I/O raises `NotImplementedError`.
The `read` attribute of a host-backed object is not fetched; that raises
`NotImplementedError` too.

## `json.dump(obj, fp, **kwargs)`

`json.dump` takes the keyword subset of `dumps` below and returns `None`.
CPython streams the document to `fp.write()` in many small chunks; Monty
encodes the whole document first and makes **one** `write` call, which changes
what is observable when something fails:

- A document that cannot be encoded writes nothing. CPython writes every chunk
    up to the failing value, so `json.dump({'a': 1, 'b': {1}}, f)` leaves
    `{"a": 1, "b": ` in the file in CPython and nothing in Monty.
- When the file object is unusable *and* the document cannot be encoded, Monty
    reports the file error for a `dict`, `list` or `tuple` document and the
    encoding error for a scalar one. CPython's order depends on where in its
    chunk stream the failure lies, which only differs from Monty's for a
    `list` or `tuple` whose first element is a scalar that fails to encode
    (`json.dump([float('nan')], closed_file, allow_nan=False)` raises the
    `ValueError` for `nan` in CPython and the closed-file error in Monty).
- A duck-typed writer sees one `write(text)` call with the whole document
    rather than one per chunk.
- A non-`str` separator raises
    `TypeError: make_encoder() argument 6 must be str, not int` (`5` for the key separator), as in `dumps`.
    CPython's `dump` uses its pure-Python encoder, which fails with a different `TypeError`
    (`unsupported operand type(s) for +: 'int' and 'str'`) or hands the separator to `write()` as it is.
    Both accept any pair of separators for a `str` document, which never uses them; a `separators` value that is
    not a two-item sequence still raises.

A file from `open()` or `Path.open()` takes a single host write, with the same
checks as `fp.write(str)`: `TypeError: a bytes-like object is required, not 'str'` in binary mode, `ValueError` on a closed file,
`io.UnsupportedOperation: not writable` on a read-only one. Any other object
has its `write` attribute called synchronously, under the same restrictions as
`load`'s `read` above.

## `json.dumps(obj, **kwargs)`

Supported kwargs, with CPython semantics: `indent`, `sort_keys`,
`ensure_ascii`, `allow_nan`, `separators`, `skipkeys`.

Rejected with `TypeError` if passed:

- `cls` — custom encoder classes are not supported.
- `default` — there is no fallback encoder callback. Non-serializable
    values raise `TypeError` instead of routing through one.
- `check_circular` — circular reference detection is always on.

## `JSONDecodeError`

Inherits from `ValueError` (catchable as `except ValueError:`). The class
qualified name is `json.JSONDecodeError`; `__name__` matches CPython.
Error messages use the same `line N column M (char K)` suffix as CPython,
counting characters, not bytes.

When the input ends inside an unclosed array or object (`'['`, `'{'`,
`'{"a"'`, …), Monty always reports `Expecting ',' delimiter`, where CPython
distinguishes what was expected at that point (`Expecting value`,
`Expecting property name enclosed in double quotes`,
`Expecting ':' delimiter`). Positions match; only the message text differs.

Inside the sandbox the exception is message-only: the `msg`, `doc`, `pos`,
`lineno` and `colno` attributes CPython sets are not available. When a
sandbox `JSONDecodeError` surfaces to the host (e.g. via `pydantic_monty`),
it is rebuilt as a real `json.JSONDecodeError` with all five attributes from
a structured payload attached at raise time, except that documents larger
than 64 KiB are not carried, in which case `doc` is `''`. A `JSONDecodeError`
raised manually inside the sandbox has no payload and surfaces as a plain
`ValueError` carrying the message.
