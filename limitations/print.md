# `print()`

Output always goes to the host via a print callback (`vm.print_writer`).
The host decides where it ends up — there is no real `sys.stdout`
underneath (see [sys.md](sys.md)).

## Supported keyword arguments

- `sep=...` — separator between arguments. `None` falls back to a single
  space. Must be a `str` or `None`; otherwise `TypeError`.
- `end=...` — appended after the last argument. `None` falls back to `"\n"`.
  Must be a `str` or `None`; otherwise `TypeError`.

## Rejected / ignored

- `file=...` — explicitly rejected with `TypeError: "print() 'file'
  argument is not supported"`. Code that does `print(..., file=sys.stderr)`
  will not work; `sys.stderr` is an opaque marker (see [sys.md](sys.md)).
- `flush=...` — silently accepted but ignored. Output is delivered to the
  host through the subprocess protocol, which line-buffers and also flushes
  large partial lines.
- Any other keyword raises `TypeError: ... unexpected keyword argument`.

## Behaviour

- Each positional argument is converted via `py_str` (equivalent to
  `str(x)`) before being written.
- The host callback receives formatted chunks. In subprocess execution,
  chunks are flushed on newline or after an internal buffer reaches roughly
  8 KiB; a single `print()` call can therefore arrive in more than one
  callback. There is no atomicity guarantee across multiple `print()` calls
  if the host interleaves with other output.

## CollectString / CollectStreams caps

`CollectString` and `CollectStreams` (Rust `PrintWriter` variants and the
matching `pydantic_monty` collectors) accumulate print output in **host-side**
buffers. That growth is **not** covered by
`ResourceLimits.max_memory` (heap-only, and in the pool only on the worker).

- Default cap: **10 MiB** (`DEFAULT_MAX_PRINT_COLLECT_BYTES`).
- Exceeding the cap raises host-visible `MemoryError` with
  `memory limit exceeded: {used} bytes > {limit} bytes` (same wording as
  heap `ResourceError::Memory`).
- Pass `max_bytes=None` to disable the cap (trusted hosts only).
- Python `CollectStreams` also charges a fixed per-entry overhead toward the
  cap (many tiny fragments would otherwise OOM the host before payload bytes
  hit the limit). Rust `PrintWriter::CollectStreams` merges consecutive
  same-stream fragments, so entry count stays small for normal `print()`.
- JS (`@pydantic/monty`): `CollectString` / `CollectStreams` accept `maxBytes`
  (camelCase), same 10 MiB default and message; `CollectStreams` charges the
  same **64-byte** per-entry overhead as the Python host path and does **not**
  merge consecutive same-stream fragments (unlike Rust in-process
  `PrintWriter::CollectStreams`). Output entries are `{ stream, text }` objects
  rather than Python tuples. The cap is a **logical UTF-8 charge**, not a hard
  V8/host-RSS bound (JS stores strings as UTF-16, so host RSS can exceed the
  stated cap).
- `Stdout` / `Disabled` / `Callback` are unchanged — `Callback` hosts can
  already self-limit by returning an error.
