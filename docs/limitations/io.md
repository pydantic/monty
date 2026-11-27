# `io`

As in CPython, `io` is split in two: the native `_io` provides `StringIO`, `BytesIO`, the file classes `open()`
returns, `open`, `open_code`, `text_encoding`, `DEFAULT_BUFFER_SIZE`, `UnsupportedOperation` and `BlockingIOError`,
and `io` is CPython's `Lib/io.py`, which re-exports them and adds `SEEK_SET` / `SEEK_CUR` / `SEEK_END`
(see [modules.md](modules.md#vendored-cpython-modules)).
`open()` files have their own page, [open.md](open.md).

## Missing names

- `io.py` is patched to drop the `IOBase`, `RawIOBase`, `BufferedIOBase` and `TextIOBase` ABCs and the `Reader`
    and `Writer` protocols, since Monty has no metaclasses or `abc`.
    They raise `AttributeError`, so `isinstance(f, io.TextIOBase)`-style checks fail, and `io.__all__` omits them.
- `FileIO`, `BufferedRWPair` and `IncrementalNewlineDecoder` are missing from both modules, as are `_io`'s
    `_IOBase`, `_RawIOBase`, `_BufferedIOBase`, `_TextIOBase` and `_BytesIOBuffer`.
- The type checker's stubs omit the same names, except `_io`'s private base classes, which type the stream methods.
- Every import of `_io` builds a fresh module, so `io._io is _io` is `False` (see [modules.md](modules.md)).

## File classes

- `TextIOWrapper`, `BufferedReader`, `BufferedWriter` and `BufferedRandom` are the types of `open()` files,
    for `type()` and `isinstance()` checks.
    Calling one raises `TypeError: cannot create '_io.{name}' instances` (e.g. `'_io.BufferedReader'`), where
    CPython wraps the buffer or raw stream it is given.

## `StringIO` and `BytesIO`

- **No subclasses.** `class S(io.StringIO)` raises `TypeError`, as for every native base (see [classes.md](classes.md)).
- **No instance attributes.** `s.x = 1` raises
    `AttributeError: '_io.StringIO' object has no attribute 'x' and no __dict__ for setting new attributes`;
    CPython's instances have a `__dict__`.
- **Methods are call-only**, like every builtin method (see [builtins.md](builtins.md)): `w = s.write` raises
    `AttributeError`.
- **Missing methods:** `BytesIO.getbuffer()`, `readinto()`, `readinto1()`, `__getstate__()` and `__setstate__()`.
    Monty has no `bytearray` or `memoryview` for the first three to use.
    `copy.copy()` and `copy.deepcopy()` work without the last two.
- **`BytesIO` takes only `bytes`.** The constructor, `write()` and `writelines()` reject `bytearray` and
    `memoryview`, which Monty does not have.

## Streams as file arguments

`json.load()`, `json.dump()`, `csv.reader()`, `csv.writer()`, `csv.DictReader()`, `csv.DictWriter()` and
`print(file=...)` take a `StringIO` or `BytesIO` and call its `read()` or `write()` as CPython does.
Only `json.load()` can use a `BytesIO`: the writers pass `str` to `BytesIO.write()`, which raises `TypeError`, and
the CSV readers raise `csv.Error` on its `bytes` lines, both as in CPython.
`json.dump()` makes fewer `write()` calls than CPython, writing the whole document once (see [json.md](json.md)).
A stream's `write()` returns the length written, so the visible result is the same.

## `text_encoding()` and `open_code()`

- `io.text_encoding(None)` returns `'locale'`, as CPython does outside UTF-8 mode.
    `open()` accepts `encoding='locale'` and reads it as UTF-8, the sandbox's only encoding
    (see [open.md](open.md)).
- `io.open_code(path)` is `open(path, 'rb')`; there is no `PyFile_SetOpenCodeHook` to change it.

## `BlockingIOError`

- Nothing in Monty raises it, since there are no non-blocking streams.
- Its constructor accepts at most one string, like every exception constructor (see
    [exceptions.md](exceptions.md)), so `BlockingIOError(errno, strerror, characters_written)` and the
    `characters_written` attribute are unavailable.
