# `open()` and file objects

Monty's `open()` builtin returns a file wrapper that supports a deliberate
subset of CPython's file API. The list below tracks every known difference
from CPython.

`pathlib.Path.open()` is wired to the same machinery — it prepends `self`
as the `file` argument and forwards to the same internal entry point, so
every divergence listed below applies equally whether the caller uses
`open(path, ...)` or `path.open(...)`. The only `Path.open()`-specific
quirks are listed in [Path.open()](#pathopen) at the bottom.

## Design note: no live host file descriptors

Monty **never** keeps a native file handle alive between OS or external
calls. `open()` itself yields an `OsFunction::Open` round-trip whose effect
(create / truncate / existence-check) the host performs and immediately
closes; every subsequent `read()`/`write()`/`append()` is a separate
one-shot OS call that the host opens, acts on, and closes again. The Monty
heap stores only path, mode, and small Python-visible state — no OS
handle, no buffered data, no descriptor number.

This is the property that makes snapshotting safe: a `MontySnapshot` can
be serialized at any pause point and resumed later (potentially in a
different process or on a different host) without dangling references to
host resources. It also means external processes can observe partial state
between calls, and that there is no protection against the underlying file
being changed or removed between calls — both documented further down.

## Mode strings

- `+` update modes (`r+`, `w+`, `a+`, and their `b` variants) are rejected at
  parse time with `ValueError: update modes ('+') are not yet supported`.
  Monty has no read-position state, so a write after a read would silently
  truncate the file via the one-shot OS write that backs `write()`.
- Exclusive creation mode (`x`) is rejected with `ValueError: exclusive
  creation mode is not supported`; it would need a dedicated race-free
  mount-table operation.
- The mode string is normalized to CPython's canonical form
  (`'rt'` → `'r'`, `'r+b'` → `'rb+'`); the original raw input is not
  preserved.

## `open()` arguments

Only `file` and `mode` are honored. The other six arguments
(`buffering`, `encoding`, `errors`, `newline`, `closefd`, `opener`) must be
at their CPython defaults; passing any non-default value raises
`TypeError: '<name>' argument is not yet supported`.

Two exceptions:

- `encoding="utf-8"` (any case, also `"utf8"`) is accepted as a documented
  no-op because Monty already uses UTF-8 for all text I/O.
- A wrong *type* for `encoding`/`errors`/`newline` (e.g. `encoding=123`)
  raises a typed `TypeError: open() argument '<name>' must be str or None,
  not <type>` rather than the generic "not yet supported" message.

Bytes paths are accepted but decoded as **strict** UTF-8 — not via CPython's
`os.fsdecode` / PEP 383 `surrogateescape` behavior. A non-UTF-8 bytes path
raises `UnicodeDecodeError: can't decode bytes path as UTF-8`.

This is a deliberate divergence, not a "not yet implemented" gap. PEP 383
relies on representing invalid bytes as lone surrogates (`U+DC80`–`U+DCFF`)
inside the resulting `str`. Rust's `String` is strictly valid UTF-8 and
cannot hold lone surrogates without `unsafe` code or a parallel `Vec<u8>`
path storage type — neither of which is justified given that Monty paths
are virtual POSIX strings, not host-OS filenames. A lossy `U+FFFD`
replacement was also rejected because it would silently re-route an
`open()` call to a different (wrong) file rather than failing loudly.

If you have non-UTF-8 bytes you need to pass as a path, decode them
explicitly on the caller side (e.g. via `os.fsdecode` outside the sandbox)
before handing them to Monty.

## File object surface

The returned object is one of `TextIOWrapper`, `BufferedReader`,
`BufferedWriter`, or `BufferedRandom` depending on mode. The supported
methods and attributes are:

- `read()` — full-file read (see caveats below).
- `write(data)` — full-file or appending write.
- `close()`, `flush()`, `readable()`, `writable()`, `seekable()`.
- `__enter__()` / `__exit__()` — `with open(...) as f:` works; see
  [`with.md`](with.md) for the shared protocol divergences.
- `name`, `mode`, `closed` attributes.
- `encoding` attribute on text files (always `"utf-8"`).

Everything else raises `AttributeError`, including: `read(size)`,
`readline()`, `readlines()`, file iteration (`for line in f`), `seek()`,
`tell()`, `truncate()`, `fileno()`, `isatty()`, `detach()`, `buffer`,
`raw`.

## Behavioural divergences

- `read()` accepts zero args only; `read(size)` is rejected. It always
  returns the entire remaining file content. After the first `read()` the
  file is marked consumed and subsequent `read()` calls short-circuit to
  empty `str`/`bytes` (matching CPython's EOF behavior for sequential
  reads).
- A `read()` that *fails* in the host still marks the file consumed. If
  user code catches the exception and calls `read()` again it gets empty
  output rather than re-reading. CPython would retry. Tracking
  success/failure outcomes would require per-call state plumbed through
  Monty's snapshot/resume cycle, which is not worth the complexity for a
  rarely-hit corner.
- `seekable()` returns `False`. CPython returns `True` for regular files
  because they support `seek()`/`tell()`; Monty does not implement either,
  so reporting `True` would crash the `if f.seekable(): f.seek(0)` idiom
  on the supposedly safe branch.
- `write()` to a text file requires `str`; to a binary file requires
  `bytes`. The error messages match CPython
  (`a bytes-like object is required, not '<type>'` /
  `write() argument must be str, not <type>`).
- Text I/O is whole-file UTF-8 with no error handlers and no newline
  translation; line endings written to a `'w'` file are preserved verbatim.
- `io.UnsupportedOperation` (raised by `read()` on `'w'` files, `write()`
  on `'r'` files, etc.) inherits from both `OSError` and `ValueError` for
  catch purposes — `except OSError:` and `except ValueError:` both work as
  in CPython. Monty's class name is the qualified `io.UnsupportedOperation`
  whereas CPython's `__name__` is the bare `UnsupportedOperation`.
- No host file descriptor is held between calls (see "Design note: no
  live host file descriptors" above). The user-visible consequence is
  that external processes can observe partial state between writes, and
  Monty offers no protection against the underlying file being changed
  or removed between calls.

## Open-time effects

These match CPython:

- `'r'`/`'rb'` on a missing file raises `FileNotFoundError` at open time.
- `'r'`/`'rb'` on a directory raises `IsADirectoryError` at open time.
- `'w'`/`'wb'` truncates the file immediately, before any write.
- `'w'`/`'wb'` creates a missing file immediately, before any write.
- `'a'`/`'ab'` creates a missing file immediately, preserving any existing
  content.

## Path.open()

`pathlib.Path.open(mode='r', ...)` forwards to the same `OsFunction::Open`
round-trip as `open()` with `self` prepended as the `file` argument, so
all the rules above (mode rejection, kwarg validation, returned wrapper
types, open-time effects) apply identically. The only differences to be
aware of:

- CPython's `Path.open()` signature lists only `mode, buffering, encoding,
  errors, newline` (no `closefd` / `opener`). Monty accepts `closefd=True`
  and `opener=None` at their CPython `open()` defaults as documented
  no-ops on this path too, and rejects non-default values with the same
  `"'closefd' argument is not yet supported"` / `"'opener' argument is
  not yet supported"` `TypeError` as `open()`. CPython would instead
  raise `TypeError: open() got an unexpected keyword argument 'closefd'`.
- Passing `file=...` as a keyword (which is meaningless on `Path.open()`
  because `self` already supplies the file) raises Monty's "multiple
  values for argument 'file'" `TypeError` rather than CPython's
  "unexpected keyword argument 'file'". Real callers do not use this.
