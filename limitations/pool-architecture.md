# Worker execution (`monty subprocess`, `monty-pool`, `Monty`/`AsyncMonty`)

The monty type checker, compiler, and interpreter should run in a separate
process, except in environments where that's not possible (like wasm), so
that sandbox crashes that cannot be fully prevented — stack overflow aborts
and allocator aborts — kill only the worker. The Python package
(`pydantic_monty`) and the JS package (`@pydantic/monty`) both do this: they
run everything in workers driven over a protobuf protocol
(`crates/monty-proto`) and expose no in-process execution API. By default the
worker is a local `monty subprocess` child; the Python package additionally
offers `pydantic_monty.AsyncMontyWebsocket`, which reaches a remote child over
a WebSocket instead (the JS package is subprocess-only). For a `monty subprocess`
worker the language semantics are identical to embedding the interpreter directly
(it is the same interpreter), and the notes below are about the *host API* surface.

A WebSocket worker is whatever the relay bridges to, and need not be a Monty
sandbox at all: a remote child may run the snippet in **real CPython with no
sandbox, no resource limits, and full host filesystem/network/subprocess
access** (relying on the deployment — a container/VM per session — for
isolation, not on the language). So none of Monty's in-process safety
guarantees hold for that transport; treat the remote as a trusted-deployment
execution surface, not a sandbox.

## Execution model

The guarantees below describe a **Monty sandbox worker** (`monty subprocess`).
A WebSocket remote honours the *protocol* shape (REPL turns, version-skew
check, value encoding) but **none** of the sandbox guarantees — resource
limits, the no-subprocess invariant (an embedded-CPython child shells out to
`uv` for installs), and the empty-environment property are Monty-sandbox
properties that real CPython does not provide, per the caveat above.

- The protocol (and `pydantic_monty`) is **REPL-only**: a pool checkout is a
  REPL session in a dedicated worker, and a one-shot run is a checkout plus a
  single feed. `feed_run` drives external function calls, OS callbacks, and
  print callbacks automatically. `feed_start` instead returns a *snapshot* at
  each suspension (`FunctionSnapshot` / `NameLookupSnapshot` / `FutureSnapshot`,
  or `MontyComplete`) for the caller to inspect, `dump()`, and `resume(...)`;
  see the snapshot divergences below.
- A session whose worker crashed is lost: subsequent calls raise
  `MontyCrashedError`. The pool itself recovers by replacing the worker.
- **The session `Configure` request carries the parent's `monty_version`, and
  the worker rejects a mismatch.** The protocol has no in-band negotiation and
  assumes parent and child are deployed in lockstep, so a child whose version
  differs from the `monty_version` in `Configure` replies `FatalError` (with a
  `version skew: parent=… child=…` message) and exits non-zero rather than
  risk a frame desync. A local subprocess child is built in lockstep with the
  parent, so this mostly matters for the WebSocket transport, where the remote
  child is deployed separately — a remote child on a different version replies
  `FatalError` and the pool surfaces it cleanly.
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
  backstop covers cases where it cannot — a worker that stops answering
  (e.g. compromised or wedged) — and surfaces as `MontyCrashedError`, losing
  the session. Mount I/O runs on the host between watchdog exchanges and does
  not count against the worker's deadline. Because the budget and consumed time are also stamped onto the
  worker's replies, sessions restored via the Rust `Pool::checkout_load`
  regain the backstop too. A *compromised* worker could under-report its
  total, stretching each turn to the full budget plus grace — turns stay
  bounded, and `request_timeout` applies independently.
- **Workers are spawned with an empty environment** (on Windows only
  `SystemRoot` is kept, which CRT/WinAPI lookups need): host secrets are
  never in a worker's memory, where a sandbox escape or memory disclosure
  could reach them. This is invisible to sandbox code — `os.getenv` etc. are
  OS calls answered by the host, never reads of the worker's own
  environment. The public Python and JS bindings expose no worker
  configuration channel outside the protocol.
- **Worker binary resolution is part of the host trust boundary.** Python and
  JS resolve the worker from an explicit constructor path first, then
  `MONTY_BIN`, then their bundled platform package (or Python scripts
  directory), then `PATH` and development fallbacks. Hosts running untrusted
  code should pin the binary path when their process environment or `PATH` is
  not trusted.

## Values crossing the process boundary

- Values are encoded as protobuf (`proto/monty/v1/monty.proto`); every
  `MontyObject` variant round-trips, but nesting depth is bounded by prost's
  decode recursion limit. The exact bound depends on container shape: roughly
  48 nested list-like containers, 32 nested dicts, or 24 nested dataclasses.
  Deeper values fail the protocol turn rather than crossing the boundary.
- `Cycle` markers (self-referential containers) can be *received* from a
  worker but are rejected as inputs.
- A single value whose encoded form would exceed the wire frame limit
  (256 MiB) — a feed input, external-function argument or return value, or a
  snippet's final result — cannot cross the boundary. This is a
  *session-preserving* failure: the host call raises an error and the worker
  stays usable, rather than the oversize frame being treated as a worker crash.
  When an external-function argument makes the suspension announcement itself
  too large, the current feed is aborted with a host-visible `RuntimeError`;
  Monty code cannot catch that error inside the aborted feed.
- Independently of the wire-byte limit, a frame is rejected if the values it
  decodes into would exceed a **per-frame host-memory budget** — a hard,
  non-configurable limit of 1 GiB of *resident* decoded bytes. The wire cap
  bounds bytes, but the cheapest elements (e.g. `None` in a list, ~4 wire bytes)
  materialize into 88-byte `MontyObject`s — a ~22× blow-up that a ≤256 MiB frame
  could turn into multiple GiB on the host. The budget is charged incrementally
  during decode and trips before the full value is built, so a parent reading
  such a frame discards the worker with a protocol error rather than risking an
  out-of-memory abort. A value large enough to hit it (tens of millions of
  elements) cannot cross the boundary even though it is under the wire-byte
  limit. Every payload — containers and function/OS-call args & kwargs alike —
  decodes straight into its final type with no intermediate copy, so the
  worst-case host *peak* is ~1× the budget plus the ≤256 MiB frame buffer, and
  the bound applies per concurrent worker.
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
  protocol turn, not mid-`print`; if that turn had suspended (an external
  function, OS call, or name lookup), the binding resets/discards the
  suspension before surfacing the print error so later feeds can continue.
- **Mounts are host-side.** `MountDir` objects contribute configuration only;
  the pool builds a fresh mount table per feed on the *host* and services the
  worker's filesystem OS calls itself — the worker never sees host paths, so
  mounts work identically for local subprocess and remote WebSocket workers.
  `mode='overlay'` writes live in that per-feed table and are discarded when
  the feed ends — the `MountDir` object's overlay state is never updated.
  `read-write` mounts write through to the real host directory as before. An
  invalid mount (host path missing / not a directory) raises at `feed` time,
  before the snippet runs, as a session-preserving error.
- **Special files are rejected.** Reading, writing, or `open()`ing a
  non-regular file in a mounted directory (FIFO, socket, device) raises
  `PermissionError` instead of blocking — CPython would block until a peer
  appears, but mount I/O runs on the host thread driving the session and must
  never block on sandbox-reachable input.
- **Mount I/O is not covered by `request_timeout`.** Covered filesystem calls
  run synchronously on the host thread driving the session; the watchdog's
  only lever is killing the worker, which cannot interrupt host-side I/O.
  Sandbox code cannot *hang* the host this way (special files are rejected,
  above), but a mount on a pathological host filesystem — a stalled NFS or
  FUSE volume — blocks the feed with no timeout. Like a blocking
  `print_callback` or external function, hang-free host I/O is the embedder's
  responsibility: do not mount directories on filesystems that can hang.
  Worker execution time is still hard-bounded: each covered call deducts the
  worker's elapsed interval from the turn's allowance, so cumulative worker
  execution per turn never exceeds `request_timeout` no matter how many
  covered calls it makes. The parent-side I/O itself is deducted from
  nothing, though, so sandbox code *can* stretch a feed's wall clock well
  beyond `request_timeout` on a perfectly healthy filesystem — e.g. a loop of
  large mounted reads pays only its own (bounded) execution time while the
  host does up to a mount-memory-budget's worth of free I/O per call. Feed
  wall clock with mounts is therefore not a hard bound; only worker execution
  is.
- **`os=` fallback** receives `(function_name, args, kwargs)`; mount-covered
  filesystem calls are serviced by the pool and never reach the callback.
- **Mounts have a 100 MB memory budget by default.** Retained overlay data and
  transient filesystem results share the configurable per-mount budget.
  Oversized operations raise `MemoryError` inside the sandbox before protocol
  encoding. CPython has no equivalent default limit. Raising the budget above
  256 MiB re-exposes the wire frame cap: a mounted read whose result exceeds
  one 256 MiB frame raises `RuntimeError` inside the sandbox instead of
  returning the data.
- **`external_lookup` resolves undefined names lazily.** `feed_run` /
  `feedRun` take `external_lookup` (`externalLookup` in JS): a name the snippet
  leaves undefined is resolved on first reference against this dict — a
  *callable* entry becomes a host function proxy (invoked on the eventual call),
  any *other value* is converted and returned directly, and an absent name
  raises `NameError`. It is the lazy counterpart to the eager `inputs` (a name
  present in both is served by the `inputs` binding, so no lookup fires). A
  non-callable value that cannot be converted rejects the turn host-side —
  because `external_lookup` (and `inputs`) may hold untrusted values, an
  unrepresentable *type* surfaces as a dedicated `MontyError` subclass (in
  `pydantic_monty`, `MontyConversionError`; its `exception()` reconstructs a
  native `TypeError`), **never** as a masquerading `NameError`; other converter
  failures, such as exceeding the max input nesting depth, keep their own type
  (`MontyRuntimeError`). The two
  workers diverge on *re-reading* a lazily-resolved **value**: the Monty sandbox
  worker caches it in the namespace slot, so a second reference in the same feed
  does not re-fire `NameLookup` (a later host mutation of the dict entry is not
  observed), whereas an embedded-CPython worker caches only function proxies and
  re-fires `NameLookup` on every value reference (re-reading live). Function
  proxies are cached by both — but unlike a CPython function object, a proxy
  dispatches by *name* against the dict passed to the current feed at call
  time: replacing an entry rebinds every reference already holding the proxy,
  and replacing it with a non-callable makes calls raise the `TypeError`
  CPython would for calling that value (`'int' object is not callable`).
  Because only *undefined* names fire lookups, an entry shadowing a builtin
  (e.g. `{'len': ...}`) is silently ignored. `feed_start` / `feedStart` take no
  `external_lookup` — they surface name lookups as snapshots, which resolve only
  to a function (see below).
- **Dependency installation is only available on an embedded-CPython worker.**
  `session.install_dependencies([...])` (sync and async in `pydantic_monty`;
  `session.installDependencies([...])` in `@pydantic/monty`) makes an
  embedded-CPython worker `uv pip install` the PEP 508 requirements so later
  feeds can import them. It is session-scoped and repeatable; an empty list is
  a no-op; and it is bounded by the pool's `request_timeout` (raise it for
  large dependency sets). The Monty sandbox worker (`monty subprocess`) has no
  host interpreter to install for, so the call raises `MontyRuntimeError` (the
  session stays usable).
- **PEP 723 inline dependencies are auto-installed by a CPython worker.**
  Before running a feed, an embedded-CPython worker scans the snippet for a
  PEP 723 `# /// script` block and installs its `dependencies` (same `uv` path
  as above) so the imports resolve — no protocol involvement, mirroring
  `uv run`. The Monty sandbox worker has no such behavior: a `# /// script`
  block is just a comment and its dependencies are never installed.
- **`dump()`** bytes use a subprocess-specific envelope and can only be
  restored into another subprocess worker of the same version, via
  `session.load` / `session.load_snapshot` (Rust `Checkout::restore`).
- **`feed_start` snapshots are live cursors, not owned state.** The execution
  state lives in the worker, so only one suspension is live per session, each
  snapshot may be resumed at most once (a second resume raises
  `RuntimeError`), and feeding while suspended raises. This differs from the
  pre-subprocess in-process API, where a snapshot owned freely-copyable state.
- **Restoring a dump is a session method, split by dump kind.** The old
  module-level `load_snapshot` / `load_repl_snapshot` are replaced by two
  fresh-session-only methods: `session.load(state)` restores a dump taken
  between feeds (an idle session) so you can keep feeding it, and
  `session.load_snapshot(state, *, mount=…)` restores a dump taken mid-feed and
  returns the re-announced snapshot to resume. The caller knows which kind it
  dumped (`session.dump()` between feeds vs `snapshot.dump()`); using the wrong
  method raises. Both restore *into* a freshly checked-out worker, so they are
  rejected (`RuntimeError`) after any `feed_run` / `feed_start` / `load` /
  `load_snapshot` — restoring would otherwise discard work. The dump restores
  its own `script_name` / limits / type-check state (the `checkout()` config
  for those is not applied); the dataclass registry from `checkout()` is reused.
  A *failed* load (wrong dump kind, or a protocol desync) poisons the session
  — its worker is discarded, so every later feed fails too; the load is not
  retryable and the caller must check out a fresh session.
- **`resume` takes no `mount=`.** Mounts are fixed for the whole feed (passed
  to `feed_start`), so there is no per-`resume` mount argument.
- **Mounts are re-supplied to `load_snapshot`, not stored in the dump.** Mounts
  are host configuration serviced by the host, not sandbox state, so nothing
  about them (host paths included) enters the (opaque, possibly-transmitted)
  dump bytes — dump contents can never cause any directory to be mounted. To
  resume a suspended feed with its mounts, pass the same `mount=` the original
  `feed_start` used to `load_snapshot`; the pool rebuilds its mount table.
  (`load` takes no `mount` — an idle session has no in-flight feed; the next
  feed supplies its own.)
- **Re-supplied mounts are not validated.** The dump records nothing about the
  feed's mounts, so `load_snapshot` cannot check what you pass: a mount
  silently omitted (or altered) simply degrades the resumed feed's covered
  filesystem calls into surfaced OS calls — unhandled ones raise
  `PermissionError` inside the sandbox.
- **`'overlay'` writes are not preserved across a dump.** A restored overlay
  mount starts empty; `read-only` / `read-write` mounts have no overlay state
  and restore fully.
- **A re-announced OS-call snapshot after `load_snapshot` carries no
  payload** — its arguments were consumed before the dump, so it surfaces
  with an empty function name and empty `args`/`kwargs`, and there is no
  per-call default error to decline with: `resume_not_handled()` raises
  `RuntimeError` instead of resuming, so the host must answer from its own
  records via `resume(...)` / `resume_error(...)`.
- **Natural-JSON host serialization was removed.** Results now cross the
  subprocess boundary as structured protocol values; the old
  `MontyComplete.output_json()` / `FunctionSnapshot.args_json()` /
  `kwargs_json()` helper format is not part of the pool API. (`feed_start`
  snapshots and `MontyComplete` expose `args` / `kwargs` / `output` as
  converted Python objects only.)

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
- **Snapshots mirror `pydantic_monty`.** `session.feedStart(code, opts)`
  returns a `FunctionSnapshot` / `NameLookupSnapshot` / `FutureSnapshot` (or a
  `MontyComplete`); `session.dump()` / `snapshot.dump()` serialize the worker,
  and `session.loadSnapshot(bytes, opts)` restores it (fresh-session-only,
  returning the re-announced snapshot or `null`). Differences from Python: a
  name lookup resolves only to an external *function* (`resume(functionName?)`,
  matching `resumeNameLookup`), not an arbitrary value; resume verbs are
  methods (`resume`, `resumeError`, `resumeNotFound`, `resumeFuture`,
  `resumeNotHandled`) rather than a result dict; and the sandbox-future
  mechanism is fully caller-driven (`resumeFuture()` then
  `FutureSnapshot.resume([{callId, value}|{callId, error}])`).
- Sessions and pools support `await using` (async disposal) in addition to
  explicit `close()`.
