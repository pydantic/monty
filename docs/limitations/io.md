# `io`

Monty's `io` module provides `StringIO`, `BytesIO`, the file classes `open()` returns, `open`, `open_code`,
`text_encoding`, `SEEK_SET` / `SEEK_CUR` / `SEEK_END`, `DEFAULT_BUFFER_SIZE`, `UnsupportedOperation` and
`BlockingIOError`.
`open()` files have their own page, [open.md](open.md).

## Missing names

- `IOBase`, `RawIOBase`, `BufferedIOBase`, `TextIOBase`, `FileIO`, `BufferedRWPair`,
    `IncrementalNewlineDecoder`, `Reader` and `Writer` raise `AttributeError`, so
    `isinstance(f, io.TextIOBase)`-style checks fail.
- The type checker's `io` stubs describe CPython's whole module, so those names pass type checking and then
    fail at runtime.
- `import _io` raises `ModuleNotFoundError`.

## File classes

- `TextIOWrapper`, `BufferedReader`, `BufferedWriter` and `BufferedRandom` are the types of `open()` files,
    for `type()` and `isinstance()` checks.
    Calling one raises `TypeError: cannot create '_io.TextIOWrapper' instances`, where CPython wraps the buffer or
    raw stream it is given.

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
- **`fileno()` and `detach()` ignore their arguments.** Both always raise `io.UnsupportedOperation`, where
    CPython first rejects an argument with `TypeError`.

## Streams as file arguments

`json.load()`, `json.dump()`, `csv.reader()`, `csv.writer()`, `csv.DictReader()`, `csv.DictWriter()` and
`print(file=...)` accept a `StringIO` or `BytesIO`.
The writers make fewer `write()` calls than CPython: `json.dump()` writes the whole document once
(see [json.md](json.md)), and `writerows()` writes all its rows once (see [csv.md](csv.md)).
A stream's `write()` returns the length written, so the visible results are the same.

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
