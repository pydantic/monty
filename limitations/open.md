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

This is the property that makes subprocess `dump()` / `load()` safe: a
session can be serialized at a pause point and resumed later without
dangling references to host resources. The wasm in-process API exposes the
same idea as `MontySnapshot`. It also means external processes can observe
partial state between calls, and that there is no protection against the
underlying file being changed or removed between calls — both documented
further down.

## Mode strings

- `+` update modes (`r+`, `w+`, `a+`, and their `b` variants) are rejected at
  parse time with `ValueError: update modes ('+') are not yet supported`.
  Monty has no read-position state, so a write after a read would silently
  truncate the file via the one-shot OS write that backs `write()`.
- Exclusive creation mode (`x`) is rejected with `ValueError: exclusive
  creation mode is not supported`; it would need a dedicated race-free
  mount-table operation.
- The mode string is normalized to a canonical form at parse time
  (`'rt'` → `'r'`, `'br'` → `'rb'`); the original raw input is not
  preserved, and `file.mode` reports the canonical form. For *text* modes
  this diverges from CPython, whose `TextIOWrapper` preserves the string
  as passed: `open(p, 'rt').mode` is `'rt'` in CPython but `'r'` in Monty.
  Binary modes match, because CPython's buffered classes normalize too
  (`open(p, 'br').mode` is `'rb'` in both).

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

- `read()` / `read(-1)` — read everything remaining from the current
  position. On the first call this performs a full-file OS read into a
  heap-resident buffer; subsequent reads slice the buffer in pure Monty.
- `read(N)` / `read(None)` — read up to N chars (text) or bytes (binary)
  from the current position, or everything remaining for `None`. Same
  backing buffer as `read()`.
- `readline()` — read up to and including the next `\n`, or the remainder
  of the buffer if the final line has no newline. Returns `''`/`b''` at
  EOF.
- `readlines()` — return a `list` of all remaining lines (each ending with
  `\n` except possibly the last).
- `tell()` — current position. **Text-mode divergence**: returns a
  char-index, not CPython's opaque byte cookie. Round-trips through
  `seek()` correctly.
- `seek(offset, whence=0)` — reposition within the buffer for readable
  files (loading it on demand), or within tracked logical write state for
  write-only files. Returns the new absolute position.
- `write(data)` — full-file or appending write.
- `close()`, `flush()`, `readable()`, `writable()`, `seekable()`.
- `__enter__()` / `__exit__()` — `with open(...) as f:` works; see
  [`with.md`](with.md) for the shared protocol divergences.
- `name`, `mode`, `closed` attributes.
- `encoding` attribute on text files (always `"utf-8"`).

Everything else raises `AttributeError`, including: `truncate()`,
`fileno()`, `isatty()`, `detach()`, `buffer`, `raw`, and the iterator
protocol (`__iter__`/`__next__`, including `for line in f:`).

## Behavioural divergences

- All reads (bare `read()`, sized `read(N)`, `readline`, `readlines`) and
  `seek()` share a single heap-resident buffer populated on the first such
  call. The host serves only one full-file `ReadText`/`ReadBytes` per
  file; everything after is sliced in pure Monty. Memory cost: the whole
  file lives in the heap and counts against the configured `max_memory`
  via `heap.allocate` tracking — the same path every other heap entry
  takes. The buffer is **never invalidated** — external modifications to
  the underlying file after the first read are not visible to subsequent
  reads.
- `close()` releases the cached buffer (matching CPython), so
  `current_memory()` drops by the buffer size as soon as `close()`
  returns. Other holders of the buffer (e.g. a `data = f.read()`
  reference) keep it alive via their own refcounts.
- A read that *fails* in the host leaves the file in a retry-safe state:
  `pending_read` is cleared, the buffer stays empty, and `eof` is not
  flipped. A user-caught exception followed by a retry will re-attempt
  the OS load. This applies uniformly to bare `read()`, sized `read(N)`,
  `readline()`, `readlines()`, and `seek()`, and matches CPython.
- `seekable()` returns `True` for all open Monty file wrappers, matching
  regular CPython files.
- Text-mode `tell()` returns a **char index** rather than CPython's opaque
  byte cookie. Round-trips through `seek()` work correctly (`pos = tell();
  seek(pos)` resumes the same position) but the raw integer differs from
  what CPython returns for non-ASCII content.
- Text-mode `seek(N)` accepts any non-negative char-index. CPython
  restricts `TextIOWrapper.seek` to `seek(0)`, `seek(0, 2)`, and cookies
  from `tell()`. Monty is more permissive here.
- `seek(-1)` raises `OSError("[Errno 22] Invalid argument")` matching
  CPython's `BufferedReader.seek`. Note that CPython's `TextIOWrapper`
  raises `ValueError("negative seek position -1")` instead — Monty uses the
  binary-mode message in both modes for consistency.
- `seek(0, 99)` raises `ValueError("whence value 99 unsupported")`
  matching CPython's `BufferedReader`. CPython's `TextIOWrapper` uses a
  different `"invalid whence ..."` message; Monty does not.
- `read(N)` accepts only int or `None`. The `TypeError` message differs
  from CPython (CPython: `"argument should be integer or None, not 'str'"`;
  Monty: `"'str' object cannot be interpreted as an integer"`).
- Write-only `seek()`/`tell()` maintain logical position state, so common
  `write(); tell()` and `seek(0, 2)` cases match CPython. However, writes
  are still full-file or append one-shot host operations: seeking backwards
  and then writing does **not** overwrite at that offset the way CPython's
  live file descriptor would.
- `readline(size)` and `readlines(hint)` are zero-argument only — passing
  a size/hint argument raises `TypeError`. CPython accepts both and uses
  them to cap the returned bytes/chars.
- File iteration (`for line in f`) is NOT supported: it goes through the
  `GetIter` opcode which cannot yield to the host. Use `readlines()` and
  iterate the resulting list instead.
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
