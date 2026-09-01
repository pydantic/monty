# Worker execution (`monty subprocess`, `monty-pool`, `Monty`/`AsyncMonty`)

The monty type checker, compiler, and interpreter should run in a separate
process, except where that's impossible (wasm), so that sandbox crashes that
cannot be fully prevented — stack overflow aborts and allocator aborts — kill
only the worker. The Python package (`pydantic_monty`) and Node JS package
(`@pydantic/monty`) drive local subprocesses over the `monty-proto` protobuf
protocol and expose no in-process execution API. Browsers instead drive the
same Rust child state machine in a Web Worker through a semantic WIT component
interface; protobuf never crosses into JavaScript. The Python package also
offers `pydantic_monty.AsyncMontyWebsocket`, which reaches a remote child over a
WebSocket. For a `monty subprocess` worker the language semantics are identical
to embedding the interpreter directly (it is the same interpreter), and the
notes below are about the *host API* surface.

A WebSocket worker is whatever the relay bridges to, and need not be a Monty
sandbox at all: a remote child may run the snippet in **real CPython with no
sandbox, no resource limits, and full host filesystem/network/subprocess
access**, relying on the deployment (a container/VM per session) for
isolation rather than on the language. None of Monty's in-process safety
guarantees hold for that transport; treat the remote as a trusted-deployment
execution surface, not a sandbox.

## Execution model

The guarantees below describe a **Monty sandbox worker** (`monty subprocess`).
A WebSocket remote honours the *protocol* shape (REPL turns, version-skew
check, value encoding) but **none** of the sandbox guarantees: resource
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
  `MontyCrashedError`, which also carries a worker's own account when it
  announced a `FatalError` before exiting (e.g. an unsupported protocol
  version), plus the exit status when the process could be reaped. The pool
  itself recovers by replacing the worker.
- **WebSocket sessions are lost in two additional ways.** A connection that
  closes mid-session raises `MontyDisconnectError`: the client cannot tell a
  dead remote sandbox from a server-side policy drop (idle/session/turn
  timeout, over capacity), so the error claims no more than that the
  connection went away. A server that is shutting down instead answers the
  session's next request with `MontyShutdown`. That request did **not** run,
  and its `dump` (when present) restores the session onto a fresh checkout
  via `session.load_session` / `session.load_snapshot`. If the interrupted
  request was answering a suspension (external function or `os` callback),
  the host already ran that call and the restored session re-announces it, so
  it runs twice unless the callback is idempotent. Neither error occurs on
  the local subprocess transport; a local child claiming shutdown is a
  protocol violation.
- **The session `Configure` request carries the parent's `protocol_version`,
  and the worker rejects one it does not serve.** The protocol has no in-band
  negotiation, so a parent outside the child's supported range
  (`MIN_SUPPORTED_PROTOCOL_VERSION..=PROTOCOL_VERSION`) gets a `FatalError`
  naming that range, enough to downgrade and retry, and the child exits
  non-zero rather than risk a frame desync. A version of `0` means the parent
  declared nothing and is always rejected. A local subprocess child ships with
  its parent, so this mostly matters for the WebSocket transport, where the
  remote child is deployed separately; the pool surfaces the rejection as a
  `MontyCrashedError` carrying the message.
- **The package version is not checked.** `Configure` also carries the
  parent's `monty_version`, but only for telemetry and to make a rejection
  legible. Parent and child may run different monty releases as long as their
  protocol versions are compatible. This does *not* extend to dumps: the
  dump envelope is versioned separately again, and restoring one still
  requires a worker built from the same version (see the snapshot
  divergences below).
- Resource exhaustion (e.g. `max_duration_secs`) is terminal for the
  *session*: later feeds keep failing with the same resource error. The
  worker process is reused for the next checkout.
- Asyncio cancellation of an in-flight call (`feed_run`, `dump`, ...)
  **loses the session**: the protocol turn was abandoned mid-flight, so its
  worker can no longer be trusted. It is killed immediately, or, when the
  checkout is contended by a concurrent call, discarded by the next call,
  which raises `RuntimeError`. A call cancelled while still queued behind
  another call never touched the worker, so the session stays usable. The
  pool itself stays healthy either way; Ctrl-C in sync code still cannot
  interrupt a turn blocked on the worker.
- **Workers never spawn subprocesses, and the pool depends on it.** The
  interpreter exposes no `fork`/`exec`/subprocess surface. `request_timeout`
  (and the `max_duration` backstop) is enforced by abandoning the turn at the
  deadline and killing the single worker PID. A worker that forked a
  grandchild would leave that grandchild running (and holding the stdout
  pipe) after the kill, so the no-subprocess property is a hard sandbox
  invariant, not just a missing feature, and the pool deliberately does
  **not** add process-group / Job Object teardown to defend against it. A
  sandbox escape that bypassed the invariant is out of scope here: it is
  already arbitrary native code running in the worker.
- **Synchronous Python telemetry can delay `request_timeout`.** The optional
  Logfire adapter runs trusted Python SDK callbacks inside the protocol turn.
  A callback that does not return prevents Tokio from polling the otherwise
  hard parent-side deadline, just like other non-yielding host work. The Node
  adapter uses non-blocking queued delivery and does not have this limitation.
- **`max_duration` measures cumulative execution time, and the worker's
  clock is the single source of truth.** The in-sandbox clock runs only
  while the interpreter executes, never while suspended waiting on the
  host (external functions, OS callbacks) or between feeds, accumulates
  across feeds, and travels inside dumps. The worker reports its total on
  every protocol turn; the host never keeps a second clock.
- **`max_duration` is backstopped by the host.** From the reported total the
  host bounds each execution turn by the remaining budget plus
  `duration_limit_grace` (default 1s) and kills the worker when it expires.
  The in-sandbox limit normally fires first with a clean `TimeoutError`; the
  backstop covers cases where it cannot, such as a worker that stops
  answering (compromised or wedged), and surfaces as `MontyCrashedError`,
  losing the session. Mount I/O runs on the host between protocol turns and
  does not count against the worker's deadline. The budget and consumed time
  are also stamped onto the worker's replies, so sessions restored via the
  Rust `Checkout::restore` regain the backstop too. A *compromised* worker
  could under-report its total, stretching each turn to the full budget plus
  grace; turns stay bounded, and `request_timeout` applies independently.
  Both deadlines fire between the turn's polls, so decoding one maximal reply
  frame (~1s worst case) can delay enforcement by that long.
- **`max_memory` is also enforced in the worker's allocator.** It caps the live
  bytes the worker's allocator will hand out, plus headroom (see
  ./resource_limits.md, which covers how exceeding it surfaces); a
  session without a limit is uncapped. The worker derives it from the session it
  holds, so nothing travels outside the protocol, and the wasm worker counts the
  same way (not the linear memory it has grown to, which never shrinks).
  Ignored by the WebSocket transport, whose exit codes do not travel, so a
  remote failure degrades to `Disconnected`. The exit code
  borrows [`sysexits.h`](https://man.freebsd.org/sysexits) so a bare status is
  legible in a log.
- **A refused allocation is the one `MemoryError` that kills the session.** The
  worker's allocator exits 65 (`EX_DATAERR`: the fed snippet asked for more than
  it may have) rather than letting Rust abort (`SIGABRT`, which a stack overflow
  also produces and which would be unclassifiable), so the host gets
  `MontyRuntimeError`/`MemoryError` with a
  distinct message instead of `MontyCrashedError`. The worker is already
  dead and later calls on that checkout report `Finished`. An ordinary in-sandbox
  exception leaves the session usable; a failed `load_session` / `load_snapshot`
  is the other `MontyRuntimeError` that does not (see below). Applies on all
  platforms, with or without a session budget, except in the wasm worker, which
  has no exit status to carry the distinction and so reports `MontyCrashedError`
  for both.
- **Workers are spawned with an empty environment** (on Windows only
  `SystemRoot` is kept, which CRT/WinAPI lookups need): host secrets are
  never in a worker's memory, where a sandbox escape or memory disclosure
  could reach them. This is invisible to sandbox code, since `os.getenv` etc.
  are OS calls answered by the host, never reads of the worker's own
  environment. The public Python and JS bindings expose no worker
  configuration channel outside the protocol.
- **Worker binary resolution is part of the host trust boundary.** Python and
  JS resolve the worker from an explicit constructor path first, then
  `MONTY_BIN`, then their bundled platform package (or Python scripts
  directory), then `PATH` and development fallbacks. Hosts running untrusted
  code should pin the binary path when their process environment or `PATH` is
  not trusted.

## Values crossing the process boundary

- Process/WebSocket transports encode values as protobuf
  (`proto/monty/v1/monty.proto`). The browser component instead uses semantic
  flat node arenas because WIT cannot express recursive types. Every
  `MontyObject` variant round-trips through either representation, with the
  same nesting bound: roughly 48 nested list-like containers, 32 nested dicts,
  or 24 nested class instances. Deeper values fail the turn rather than
  crossing the boundary.
- `Cycle` markers (self-referential containers) can be *received* from a
  worker but are rejected as inputs.
- A sandbox value with no `MontyObject` equivalent — a class, a class
  instance, a function, a compiled `re` pattern — is **silently degraded to
  its repr string** on the way out, rather than failing. A host function
  receiving one gets a `str`: `"<class 'C'>"`, `"<C object at 0x5>"`,
  `"<function '<lambda>' at 0x2c>"`, `"re.compile('a')"`. This applies to
  external-function arguments and to a snippet's final result, so a host
  cannot distinguish such a value from a sandbox string of the same text.
- A single value whose encoded form would exceed the wire frame limit
  (256 MiB) — a feed input, external-function argument or return value, or a
  snippet's final result — cannot cross the boundary. This is a
  *session-preserving* failure: the host call raises an error and the worker
  stays usable, rather than the oversize frame being treated as a worker crash.
  When an external-function argument makes the suspension announcement itself
  too large, the current feed is aborted with a host-visible `RuntimeError`;
  Monty code cannot catch that error inside the aborted feed.
- The same frame limit applies to `dump()`: a session whose serialized state
  (heap plus any retained suspension payload) exceeds 256 MiB cannot be
  dumped. The call raises a `RuntimeError` and the session is unaffected: a
  suspended session stays suspended and resumable.
- On protobuf transports, independently of the wire-byte limit, a frame is
  rejected if the values it decodes into would exceed a **per-frame host-memory budget**, a hard,
  non-configurable limit of 1 GiB of *resident* decoded bytes. The wire cap
  bounds bytes, but the cheapest elements (e.g. `None` in a list, ~4 wire bytes)
  materialize into 88-byte `MontyObject`s, a ~22× blow-up that a ≤256 MiB frame
  could turn into multiple GiB on the host. The budget is charged incrementally
  during decode and trips before the full value is built, so a parent reading
  such a frame discards the worker with a protocol error rather than risking an
  out-of-memory abort. A value large enough to hit it (tens of millions of
  elements) cannot cross the boundary even though it is under the wire-byte
  limit. Every payload, containers and function/OS-call args & kwargs alike,
  decodes straight into its final type with no intermediate copy, so the
  worst-case host *peak* is ~1× the budget plus the ≤256 MiB frame buffer, and
  the bound applies per concurrent worker. The browser component applies the
  same expanded-value budget across all WIT value arenas in a request before
  constructing their `MontyObject`s, and before lifting a semantic event into
  JavaScript.
- Semantic validation of protobuf values (date ranges, timedelta normalization,
  exception/type/builtin names) happens *while decoding* the frame; the browser
  component applies the same checks while converting its WIT value arena. A frame
  carrying an invalid value therefore fails the whole protocol turn: a parent
  receiving one discards the worker with a protocol error; a worker receiving
  one answers with a `RuntimeError("protocol violation: malformed request:
  ...")` turn and keeps the session. Parents written in other languages (e.g.
  the JS client) see the same behaviour.

## Host-API behaviour notes

- **A host-function return value the wire cannot carry fails *inside* the
  sandbox, not host-side.** An unrepresentable type becomes a catchable
  `TypeError: Cannot convert X to Monty value`; one nested past the wire depth
  bound becomes a catchable `RuntimeError: Max input depth exceeded`. Either
  reaches the host as `MontyRuntimeError` only when the sandbox does not catch
  it. The same holds for an `os=` callback's return value, and for the JS
  client. `MontyConversionError` covers only values the host supplies up
  front, in `inputs` or `external_lookup`.
- **Typing errors** (`checkout(type_check=True)`) raise `MontyTypingError`
  whose diagnostics were rendered *in the worker*, so the format is a
  checkout argument (`type_check_format=`, `type_check_color=`; JS
  `typeCheckFormat` / `typeCheckColor`) and `display()` takes no arguments:
  it cannot re-render, because ty's structured diagnostics resolve their spans
  against the checker's database and so never cross the wire. Formats are
  ty's: `full` (default), `concise`, `azure`, `json`, `jsonlines`, `rdjson`,
  `pylint`, `gitlab`, `github`; only `full` and `concise` carry colour.
- **Print callbacks** receive buffered chunks flushed at newline boundaries
  or once ~8 KiB accumulates, not per-fragment writes. A chunk may contain
  more than one line, and output larger than the threshold is split into
  ~8 KiB pieces, so a chunk is bounded but not guaranteed to be exactly
  one line. A callback that raises aborts the feed after the current
  protocol turn, not mid-`print`; if that turn had suspended (an external
  function, OS call, or name lookup), the binding resets/discards the
  suspension before surfacing the print error so later feeds can continue.
- **The sync API adapts to the caller's Tokio context.** `Monty` methods block
  the calling thread on the binding's Tokio runtime. Called from a worker
  thread of a multi-thread runtime, e.g. a sync external function or
  `print_callback` invoked by an `AsyncMonty` drive, the wait is wrapped in
  `tokio::task::block_in_place`, so opening an independent nested sync
  pool/session works (each concurrent nested call occupies an extra OS thread
  while it waits). Called from any *current-thread* Tokio runtime context,
  blocking would starve the tasks that drive the pool, so every sync method
  raises `RuntimeError: the synchronous Monty API cannot run inside a
  current-thread Tokio runtime`. Only independent nested pools/sessions are
  supported: re-entering the *same* session from its own callback deadlocks
  on the session's internal lock.
- **Mounts are host-side.** `MountDir` objects contribute configuration only;
  the pool builds a fresh mount table per feed on the *host* and services the
  worker's filesystem OS calls itself. The worker never sees host paths, so
  mounts work identically for local subprocess and remote WebSocket workers.
  `mode='overlay'` writes live in that per-feed table and are discarded when
  the feed ends; the `MountDir` object's overlay state is never updated.
  `read-write` mounts write through to the real host directory as before. An
  invalid mount (host path missing / not a directory) raises when the mount
  object is *created*, not at `feed` time: constructing it opens the directory.
- **A mount object is bound to a directory, not to a path.** `MountDir` opens
  the host directory once and every feed mounts that descriptor, so renaming or
  replacing the directory afterwards does not change what the mount serves:
  the mount keeps following the original directory under its new name, and a
  new directory at the old path is not picked up. Recreate the `MountDir` to
  follow a path instead. This is deliberate: the sandbox can rename inside a
  `read-write` mount, so a path re-resolved each feed is a path the sandbox can
  redirect.
- **On Windows a mounted directory is locked for as long as the mount object
  lives.** It holds an open descriptor, and Windows refuses to rename or delete
  a directory while a handle to it is open, so the host gets
  `ERROR_SHARING_VIOLATION` until the mount is closed, not just until the feed
  ends. `MountDir.close()` releases it (also `with` in Python, `using` in
  JavaScript); a feed already running keeps its own reference, and feeding a
  closed mount raises. Unix is unaffected, and closing is optional there.
- **Mounts only answer calls on the automatic path.** Every OS call the sandbox
  makes surfaces as a suspension; the pool consults the mount table only when
  the caller asks it to. `feed_run` (and the JS `feedRun`) asks on every OS
  call, so mounted I/O is transparent there. `feed_start` never does: a mounted
  read comes back as a `FunctionSnapshot` with `is_os_function` set, and it is
  `resume_auto()` that offers the call to the mounts and then to `os=`.
  Answering such a snapshot with an explicit `resume(...)` bypasses the mount
  entirely; the value you supply is what the sandbox sees.
- **Special files are rejected.** Reading, writing, or `open()`ing a
  non-regular file in a mounted directory (FIFO, socket, device) raises
  `PermissionError` instead of blocking. CPython would block until a peer
  appears, but mount I/O blocks the feed (and holds a host thread) for its
  full duration and must never wait on sandbox-reachable input.
- **Mount I/O is not bounded by any timeout.** Covered filesystem calls run
  on the host *between* protocol turns, with no turn deadline armed, and the
  deadline's only lever, killing the worker, could not interrupt host I/O
  anyway. Special files are rejected (above) so sandbox code cannot hang the
  host, but a stalled NFS/FUSE volume blocks the feed indefinitely; hang-free
  host I/O is the embedder's responsibility, as for `print_callback` and
  external functions. The I/O runs on Tokio's blocking thread pool, so a
  stalled mount ties up its own feed and one blocking thread, not the
  runtime workers that drive other sessions' turns and timers. Cancelling the
  feed does not cancel the filesystem call: the detached operation keeps its
  blocking thread until it returns, and a `read-write` mount's write, rename,
  or delete can complete on the host *after* cancellation was observed (and
  the worker discarded). Each covered
  call is answered by its own turn, so a *loop* of mounted reads resets
  `request_timeout` every iteration, exactly like a loop of external calls.
  `max_duration` still bounds such a feed's worker execution, but nothing
  bounds its wall clock.
- **`os=` fallback** receives `(function_name, args, kwargs)`. On the
  automatic path (`feed_run`, `resume_auto`) mounts get first refusal, so
  mount-covered filesystem calls never reach the callback. Under `feed_start`
  the callback is consulted only by `resume_auto()`; it is never invoked
  between snapshots.
- **Mounts have a 100 MB memory budget by default.** Retained overlay data and
  transient filesystem results share the configurable per-mount budget.
  Oversized operations raise `MemoryError` inside the sandbox before protocol
  encoding. CPython has no equivalent default limit. Raising the budget above
  256 MiB re-exposes the wire frame cap: a mounted read whose result exceeds
  one 256 MiB frame raises `RuntimeError` inside the sandbox instead of
  returning the data.
- **`external_lookup` resolves undefined names lazily.** `feed_run` /
  `feedRun` take `external_lookup` (`externalLookup` in JS): a name the snippet
  leaves undefined is resolved on first reference against this dict. A
  *callable* entry becomes a host function proxy (invoked on the eventual call),
  any *other value* is converted and returned directly, and an absent name
  raises `NameError`. It is the lazy counterpart to the eager `inputs` (a name
  present in both is served by the `inputs` binding, so no lookup fires). A
  non-callable value that cannot be converted rejects the turn host-side:
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
  proxies are cached by both, but unlike a CPython function object, a proxy
  dispatches by *name* against the dict passed to the current feed at call
  time: replacing an entry rebinds every reference already holding the proxy,
  and replacing it with a non-callable makes calls raise the `TypeError`
  CPython would for calling that value (`'int' object is not callable`).
  Because only *undefined* names fire lookups, an entry shadowing a builtin
  (e.g. `{'len': ...}`) is silently ignored. `feed_start` / `feedStart` take no
  `external_lookup`; they surface name lookups as snapshots, which resolve only
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
  as above) so the imports resolve, with no protocol involvement, mirroring
  `uv run`. The Monty sandbox worker has no such behavior: a `# /// script`
  block is just a comment and its dependencies are never installed.
- **`dump()`** bytes carry monty's own versioned session format and can only be
  restored into a worker built with the same `DUMP_VERSION`, via
  `session.load_session` / `session.load_snapshot` (Rust `Checkout::restore`).
  A version mismatch is reported as such, naming both versions, so a stale
  snapshot is distinguishable from a corrupt one.
- **`feed_start` snapshots are live cursors, not owned state.** The execution
  state lives in the worker, so only one suspension is live per session, each
  snapshot may be resumed at most once (a second resume raises
  `RuntimeError`), and feeding while suspended raises. This differs from the
  pre-subprocess in-process API, where a snapshot owned freely-copyable state.
- **Restoring a dump is a session method, split by dump kind.** The old
  module-level `load_snapshot` / `load_repl_snapshot` are replaced by two
  fresh-session-only methods: `session.load_session(state)` restores a dump
  taken between feeds (an idle session) so you can keep feeding it, and
  `session.load_snapshot(state, *, mount=…)` restores a dump taken mid-feed and
  returns the re-announced snapshot to resume. The caller knows which kind it
  dumped (`session.dump()` between feeds vs `snapshot.dump()`); using the wrong
  method raises. Both restore *into* a freshly checked-out worker, so they are
  rejected (`RuntimeError`) after any `feed_run` / `feed_start` / `load_session`
  / `load_snapshot` — restoring would otherwise discard work. The dump restores
  its own `script_name` / limits / type-check state (the `checkout()` config
  for those is not applied). The session's class-instance store is host-side
  and NOT part of the dump: restoring into a fresh session/process starts with
  an empty store, so a returned host instance becomes a `MontyClassProxy`
  stand-in, a method call on it raises `RuntimeError` ("no host object
  registered..."), a lazy attribute lookup raises `AttributeError`, and a
  construction or classmethod call on a `ClassType`-granted class raises
  `RuntimeError` ("no host class registered...").
- **The class-instance store retains every wrapper sent into the sandbox until
  the session ends** — that retention is what makes method routing and
  original-object return work, and it is not covered by the sandbox's
  `max_memory` (it is host memory). Re-sending the same instance overwrites
  its entry (last wrapper's policy wins), but distinct instances accumulate:
  a `convert_value` override that wraps a fresh object per method call grows
  the store by one entry per call — as does every sandbox-driven construction
  of an `init=True` `ClassType` — and wrappers registered during a feed that
  later fails conversion are kept too. Long-running sessions exposing
  object-returning methods or instantiable classes should bound what they
  hand out (or recycle sessions periodically); the store has no built-in
  entry cap.
  A *failed* load (wrong dump kind, or a protocol desync) poisons the session
  — its worker is discarded, so every later feed fails too; the load is not
  retryable and the caller must check out a fresh session.
- **`resume` takes no `mount=` or `os=`.** Mounts and the OS fallback are
  fixed for the whole feed (passed to `feed_start` / `load_snapshot`), and a
  plain `resume(...)` answers only the call in hand, consulting neither.
  `resume_auto()` is the method that uses them.
- **Mounts are re-supplied to `load_snapshot`, not stored in the dump.** Mounts
  are host configuration serviced by the host, not sandbox state, so nothing
  about them (host paths included) enters the (opaque, possibly-transmitted)
  dump bytes; dump contents can never cause any directory to be mounted. To
  resume a suspended feed with its mounts, pass the same `mount=` the original
  `feed_start` used to `load_snapshot`; the pool rebuilds its mount table.
  (`load_session` takes no `mount`: an idle session has no in-flight feed; the
  next feed supplies its own.)
- **Re-supplied mounts are not validated.** The dump records nothing about the
  feed's mounts, so `load_snapshot` cannot check what you pass: a mount
  silently omitted (or altered) simply means `resume_auto()` finds nothing
  covering the resumed feed's filesystem calls, and they fall through to `os=`
  or raise `PermissionError` inside the sandbox. A dump taken *while suspended
  on an OS call* re-announces that call in full, so a mount supplied only at
  `load_snapshot` can still answer it.
- **`'overlay'` writes are not preserved across a dump.** A restored overlay
  mount starts empty; `read-only` / `read-write` mounts have no overlay state
  and restore fully.
- **Natural-JSON host serialization was removed.** Results now cross the
  subprocess boundary as structured protocol values; the old
  `MontyComplete.output_json()` / `FunctionSnapshot.args_json()` /
  `kwargs_json()` helper format is not part of the pool API. (`feed_start`
  snapshots and `MontyComplete` expose `args` / `kwargs` / `output` as
  converted Python objects only.)

## JavaScript client (`@pydantic/monty`)

The Node.js npm package implements the same parent side of the protocol through
a napi-rs binding over `monty-pool`; platform npm packages ship both the native
addon and the `monty` worker binary. The browser entry point implements the
same protocol in TypeScript over a WASM worker. Everything above applies, plus:

- **Exception pass-through is by name.** A thrown JS error crosses into the
  sandbox using `error.name` when it matches one of monty's exception types
  (`TypeError`, `ValueError`, `KeyError`, ...); anything else becomes
  `RuntimeError`. Tracebacks of host errors are not preserved.
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
