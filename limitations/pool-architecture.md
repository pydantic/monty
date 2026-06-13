# Subprocess execution (`monty --subprocess`, `monty-pool`, `Monty`/`AsyncMonty`)

The monty type checker, compiler, and interpreter should run in a separate
process, except in environments where that's not possible (like wasm), so
that crashes a sandbox can never fully prevent — stack overflow aborts,
allocator aborts — kill only the worker. The Python package
(`pydantic_monty`) and the JS package (`@pydantic/monty`) both do this: they
run everything exclusively in `monty --subprocess` workers driven over a
protobuf protocol (`crates/monty-proto`), and expose no in-process execution
API. The language semantics inside a worker are identical to embedding the
interpreter directly (it is the same interpreter); the notes below are about
the *host API* surface.

## Execution model

- The protocol (and `pydantic_monty`) is **REPL-only**: a pool checkout is a
  REPL session in a dedicated worker, and a one-shot run is a checkout plus a
  single feed. There are no manual suspension snapshots in Python; external
  function calls, OS callbacks, and print callbacks are driven automatically
  by `feed_run` (sync or awaited). (The Rust `monty-pool::Checkout` API does
  expose manual suspension driving and `Pool::checkout_load`.)
- A session whose worker crashed is lost: subsequent calls raise
  `MontyCrashedError`. The pool itself recovers by replacing the worker.
- Resource exhaustion (e.g. `max_duration_secs`) is terminal for the
  *session*: later feeds keep failing with the same resource error. The
  worker process is reused for the next checkout.
- Ctrl-C / asyncio cancellation cannot interrupt a protocol turn already
  blocked on the worker; use sandbox `limits` and/or the pool's
  `request_timeout` (which kills the worker).
- **Workers never spawn subprocesses, and the pool depends on it.** The
  interpreter exposes no `fork`/`exec`/subprocess surface. The watchdog
  enforces `request_timeout` (and the `max_duration` backstop) by killing the
  single worker PID, which closes the worker's stdout and unblocks the
  parent's blocked read. A worker that forked a grandchild inheriting that
  pipe could hold it open past the kill and hang the parent forever, so the
  no-subprocess property is a hard sandbox invariant, not just a missing
  feature — and the pool deliberately does **not** add process-group / Job
  Object teardown to defend against it. A sandbox escape that bypassed the
  invariant is out of scope here: it is already arbitrary native code running
  in the worker.
- **`max_duration` measures cumulative execution time, and the worker's
  clock is the single source of truth.** The in-sandbox clock runs only
  while the interpreter executes — never while suspended waiting on the
  host (external functions, OS callbacks) or between feeds — accumulates
  across feeds, and travels inside dumps. The worker reports its total on
  every protocol turn; the host never keeps a second clock.
- **`max_duration` is backstopped by the host.** From the reported total the
  host arms each execution turn's watchdog with the remaining budget plus
  `duration_limit_grace` (default 1s) and kills the worker when it expires.
  The in-sandbox limit normally fires first with a clean `TimeoutError`; the
  backstop covers cases where it cannot — e.g. a blocking syscall inside a
  mount (reading a FIFO) — and surfaces as `MontyCrashedError`, losing the
  session. Because the budget and consumed time are also stamped onto the
  worker's replies, sessions restored via the Rust `Pool::checkout_load`
  regain the backstop too. A *compromised* worker could under-report its
  total, stretching each turn to the full budget plus grace — turns stay
  bounded, and `request_timeout` applies independently.
- **Workers are spawned with an empty environment** (on Windows only
  `SystemRoot` is kept, which CRT/WinAPI lookups need): host secrets are
  never in a worker's memory, where a sandbox escape or memory disclosure
  could reach them. This is invisible to sandbox code — `os.getenv` etc. are
  OS calls answered by the host, never reads of the worker's own
  environment — but means `extra_args` is the only way to configure a worker
  process externally.

## Values crossing the process boundary

- Values are encoded as protobuf (`proto/monty/v1/monty.proto`); every
  `MontyObject` variant round-trips, but nesting depth is bounded by prost's
  decode recursion limit (~50 levels of list/dict nesting). A deeper result
  value fails the protocol turn rather than crossing the boundary.
- `Cycle` markers (self-referential containers) can be *received* from a
  worker but are rejected as inputs.
- A single value whose encoded form would exceed the wire frame limit
  (256 MiB) — a feed input, an external-function return value, or a snippet's
  final result — cannot cross the boundary. This is a *session-preserving*
  failure: the host call raises a catchable error and the worker stays
  usable, rather than the oversize frame being treated as a worker crash. The
  one exception is an external function whose *arguments* exceed the limit:
  the worker has already suspended to ask for them and cannot be cleanly
  resumed, so that case still loses the session (surfacing as a crash).
- Semantic validation of wire values (date ranges, timedelta normalization,
  exception/type/builtin names) happens *while decoding* the frame. A frame
  carrying an invalid value therefore fails the whole protocol turn: a parent
  receiving one discards the worker with a protocol error; a worker receiving
  one answers with a `RuntimeError("protocol violation: malformed request:
  ...")` turn and keeps the session. Parents written in other languages (e.g.
  the JS client) see the same behaviour.

## Host-API behaviour notes

- **Typing errors** (`checkout(type_check=True)`) raise `MontyTypingError`
  whose diagnostics were rendered in the worker with the default format —
  `display()` takes no arguments.
- **Print callbacks** receive buffered chunks flushed at newline boundaries
  or once ~8 KiB accumulates — not per-fragment writes. A chunk may contain
  more than one line, and output larger than the threshold is split into
  ~8 KiB pieces (so a chunk is bounded, but is not guaranteed to be exactly
  one line). A callback that raises aborts the feed after the current
  protocol turn, not mid-`print`; and if that turn had suspended (an external
  function, OS call, or name lookup), the session cannot be resumed and is
  ended — a later feed on it raises rather than continuing.
- **Mounts are worker-local.** `MountDir` objects contribute configuration
  only; `mode='overlay'` writes live in the worker for the duration of one
  feed and are discarded when it ends — the host `MountDir` object's overlay
  state is never updated. `read-write` mounts write through to the real host
  directory as before.
- **`os=` fallback** receives `(function_name, args, kwargs)`; mount-covered
  filesystem calls are handled inside the worker and never reach the
  callback.
- **`dump()`** bytes use a subprocess-specific envelope and can only be
  restored into another subprocess worker (Rust `Pool::checkout_load`); there
  is currently no Python API to restore them.

## JavaScript client (`@pydantic/monty`)

The npm package implements the same parent side of the protocol in pure
TypeScript (`crates/monty-js`) — no Rust in the package; workers are `monty`
binaries shipped in platform npm packages. Everything above applies, plus:

- **Dataclass method calls are unsupported.** JS has no dataclass registry,
  so a sandbox call to a method on a host dataclass (`method_call` on the
  wire) raises `RuntimeError: method calls on host objects are not
  supported: <name>` instead of dispatching to a host method.
- **Exception pass-through is by name.** A thrown JS error crosses into the
  sandbox using `error.name` when it matches one of monty's exception types
  (`TypeError`, `ValueError`, `KeyError`, ...); anything else becomes
  `RuntimeError`. Tracebacks of host errors are not preserved.
- **Deep external-function return values** (beyond the wire depth bound)
  raise a *catchable* `RuntimeError: Max input depth exceeded` inside the
  sandbox, where `pydantic_monty` raises host-side and abandons the feed.
  Return values that cannot be converted at all (e.g. a `Symbol`, or a
  malformed `__monty_type__` marker object) likewise raise a catchable
  in-sandbox `TypeError` instead of failing host-side.
- **`dump()`** returns the opaque bytes; there is no JS restore API.
- Sessions and pools support `await using` (async disposal) in addition to
  explicit `close()`.

For browsers (no subprocesses), the `@pydantic/monty/wasm` subpath keeps the
old napi in-process API compiled to `wasm32-wasip1-threads`; it has none of
the crash isolation described here — a sandbox crash is a host crash.
