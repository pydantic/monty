# `print()`

Output always goes to the host via a print callback (`vm.print_writer`). The
host decides where it ends up; there is no real `sys.stdout` underneath (see
[sys.md](sys.md)).

## Supported keyword arguments

- `sep=...` — separator between arguments. `None` falls back to a single
  space. Must be a `str` or `None`; otherwise `TypeError`.
- `end=...` — appended after the last argument. `None` falls back to `"\n"`.
  Must be a `str` or `None`; otherwise `TypeError`.

## Rejected / ignored

- `file=...` — rejected with `TypeError: "print() 'file' argument is not
  supported"`. Code that does `print(..., file=sys.stderr)` will not work;
  `sys.stderr` is an opaque marker (see [sys.md](sys.md)).
- `flush=...` — accepted and ignored. Output is delivered to the host through
  the subprocess protocol on its own schedule (see "Chunk boundaries" below);
  a `print()` cannot make it arrive sooner.
- Any other keyword raises `TypeError: ... unexpected keyword argument`.

## Behaviour

- Each positional argument is converted via `py_str` (equivalent to `str(x)`)
  before being written.
- The host callback receives formatted chunks. There is no atomicity guarantee
  across multiple `print()` calls if the host interleaves with other output.

## Chunk boundaries

Chunk boundaries carry no meaning. The worker holds output in a buffer and
sends it when the buffer reaches roughly 8 KiB or its oldest byte has waited
out the flush interval (5 ms by default), so:

- One `print()` can arrive in several callbacks, and several `print()` calls
  can arrive in one. A chunk does not correspond to a call, a line, or an
  argument.
- Output that is printed and then followed by silence is released by the
  interpreter's periodic checkpoint, so it does not wait for the next
  `print()`.
- That checkpoint sits in the bytecode dispatch loop, so it only fires between
  instructions. One long native operation — a large `sort`, a regex scan,
  `json.dumps` over a big structure — holds whatever was buffered for as long
  as it runs, however short the interval. This is the one case where the
  interval does not bound the wait. The output is late, never dropped: the
  buffer is still drained before the next host call and at the end of the
  turn, so a host that needs liveness here can set the interval to 0 and get a
  callback as each line is written.
- Ordering is exact, and the buffer is always drained before a host call or the
  end of a run, so output cannot arrive after the event it preceded.
- Buffered output is lost if the worker dies *hard*: the pool killing it on
  `request_timeout`, the allocator ending the process at its hard memory
  ceiling, or a crash. A graceful turn drains first, so this only affects a
  worker that never finished — but the window is up to 8 KiB or one flush
  interval of already-complete lines, where line buffering would have sent
  them. A host that would rather have that output than the batching (to see
  what a snippet printed before it hung, say) can set the interval to 0.

Hosts can set the interval per session: `print_flush_interval` (seconds) in
`pydantic_monty`, `printFlushInterval` (seconds) in `@pydantic/monty`, and
`ReplConfig::print_flush_interval` in Rust. `0` turns the timer off and
restores line buffering, one callback per completed line. The wire carries
whole milliseconds, so a positive interval under 1 ms is sent as 1 ms rather
than rounding down into that sentinel, and a negative or non-finite value is
rejected at checkout.

The wasm worker takes the same setting, but it does not stream: a turn's
frames all reach the host together when the turn ends, whatever the interval.
What the setting still decides there is how that output is *split* — one print
callback per frame, and a collector charges its cap per frame — so a snippet
that hangs or is killed yields nothing either way.

## CollectString / CollectStreams caps

`CollectString` and `CollectStreams` (Rust `PrintWriter` variants and the
matching `pydantic_monty` collectors) accumulate print output in **host-side**
buffers. That growth is **not** covered by `ResourceLimits.max_memory`
(heap-only, and in the pool only on the worker).

- Default cap: **10 MiB** (`DEFAULT_MAX_PRINT_COLLECT_BYTES`).
- Exceeding the cap fails with `memory limit exceeded: {used} bytes > {limit}
  bytes` (same wording as heap `ResourceError::Memory`), but *what raises* and
  *who can catch it* differ by host:
  - **In-process Rust** (`PrintWriter::CollectString`/`CollectStreams`): the
    error is raised inside the VM as an ordinary `MemoryError`, so sandboxed
    code **can** catch it with `except MemoryError`. Unlike a real resource
    limit it is a catchable `RunError::Exc`, not `UncatchableExc`.
  - **Pool hosts** (`pydantic_monty`, `@pydantic/monty`): the cap is enforced
    in the parent as print events arrive, so it fails the protocol *turn*
    rather than raising into the VM. Sandboxed code cannot catch it, and the
    host sees `MontyRuntimeError` whose inner exception is `MemoryError`
    (`exc.exception()` in Python, `err.exception.typeName` in JS). The JS
    check is its own TypeScript implementation
    (`crates/monty-js/ts/print.ts`), not the Rust `PrintWriter`.
- Pass `max_bytes=None` to disable the cap (trusted hosts only).
- Python `CollectStreams` also charges a fixed per-entry overhead toward the
  cap, since many tiny fragments would otherwise OOM the host before payload
  bytes hit the limit. Rust `PrintWriter::CollectStreams` merges consecutive
  same-stream fragments, so entry count stays small for normal `print()`.
- Entries follow the chunk boundaries above, not `print()` calls: several
  prints usually collect into one entry. Set `print_flush_interval=0` to get
  one entry per completed line.
- JS (`@pydantic/monty`): `CollectString` / `CollectStreams` accept `maxBytes`
  (camelCase), same 10 MiB default and message; `CollectStreams` charges the
  same **64-byte** per-entry overhead as the Python host path and does **not**
  merge consecutive same-stream fragments (unlike Rust in-process
  `PrintWriter::CollectStreams`). Output entries are `{ stream, text }` objects
  rather than Python tuples. The cap is a **logical UTF-8 charge**, not a hard
  V8/host-RSS bound: JS stores strings as UTF-16, so host RSS can exceed the
  stated cap.
- `Stdout` / `Disabled` / `Callback` are unchanged; `Callback` hosts can
  already self-limit by returning an error.
