# monty-cpython

A Monty wire-protocol child worker (like `monty subprocess`) that executes each
fed snippet in **embedded CPython** instead of Monty, resolving undefined names
against the parent (a `NameLookup`, then a `FunctionCall` if the name is a host
function that gets called). It lets a parent (`monty-pool` / `pydantic_monty`)
drive a *real* Python interpreter over the same protocol — locally over stdio, or
remotely over a WebSocket.

## Transports

The transport is selected by subcommand:

- `monty-cpython subprocess` — framed stdio, a drop-in worker for `monty-pool`
  (point `binary_path` at this binary).
- `monty-cpython websocket <ws-url>` — dial a relay (or a parent-as-server) as a
  WebSocket client.

The execution `globals` is a `dict` subclass whose `__missing__` resolves any
unbound global that is not a builtin or dunder through the host: it emits a
`NameLookup` and, based on the parent's answer, returns the host value, an
`ExternalFunction` proxy (whose call emits a `FunctionCall`), or raises
`NameError`. All value conversion and transport work happens in Rust; the Python
glue is tiny (see `src/pyexec.rs`).

## Docker image

This crate ships a `Dockerfile` (with `Dockerfile.dockerignore`) that builds an
OCI image of the worker on `python:3.14-slim-trixie` and bundles the `uv` binary
on `PATH` for dependency installs. Build it with `make build-cpython-image` from the
workspace root (the build context must be the workspace root — the crate has
path-local deps), or see the header of `Dockerfile` for the raw `docker buildx`
invocation.

## SECURITY: not a sandbox

Full CPython is **not** a security boundary. A fed snippet can `import os`, open
files, spawn processes — anything this process can do. `monty-cpython` provides
**no isolation of its own**; isolation is entirely the deployment's
responsibility (run it inside a locked-down container, microVM, or a
relay-provisioned sandbox). This is the fundamental difference from the Monty
interpreter, which *is* a sandbox. Do not run `monty-cpython` on untrusted code
outside an externally-enforced jail.

## One session per sandbox

A `monty-cpython` worker serves **exactly one session per process**. There is no
in-process reuse: a parent's checkout (`with pool.checkout() as session: ...`)
dials a fresh worker, and when that context manager exits, the session — and the
per-session sandbox holding the worker — is torn down. `Reset` and `Shutdown` are
therefore equivalent: both acknowledge, then exit the process, letting the OS /
sandbox reclaim the interpreter, its `sys.modules`, and the session's installed
packages. Nothing leaks from one session into the next, because in the same
process there *is* no next session.

This one-shot model assumes the deployment provisions a **per-session sandbox**
(the relay-provisioned sandbox above) with a **writable filesystem** — the worker
writes a session's dependencies into a virtualenv (see below). Pausing and
resuming a session would need extra protocol support and is not implemented.

## Supported vs rejected protocol requests

Supported: `Configure`, `Feed`, `InstallDependencies` (see below),
`ResumeNameLookup` and `ResumeCall` (both consumed inline during a feed, never at
the top level), `Reset`, `Shutdown`.

Rejected with a turn-ending `Error` (the session survives):

- **`Dump` / `Load`** — a feed suspends on a live C stack inside a blocking
  `__call__`, which cannot be serialized; snapshots are not supported.
- **`ResumeFutures`** — there is no async-future suspension (see async, below).

`Configure` with a mismatched `monty_version` is fatal (`FatalError` + exit 4),
exactly like the Monty child; both are workspace-versioned so they match.

## Undefined-name model

The execution `globals` is a `dict` subclass whose `__missing__` resolves any
unbound global that is **not a builtin and not a dunder** through the host: it
emits a `NameLookup` and branches on the parent's `ResumeNameLookup` answer:

- a host **value** → the converted Python value, returned directly (re-read live
  on every reference — host values are not cached);
- a host **function** → an `ExternalFunction` proxy whose call emits a
  `FunctionCall` (cached per session, so repeated references/calls skip the
  lookup);
- **undefined** → `NameError`, raised on the reference itself.

Consequences that differ from CPython:

- A referenced name the host resolves to a **function** yields an
  `ExternalFunction` proxy rather than raising. If such a proxy is the snippet's
  trailing expression (or otherwise returned), it cannot be converted to a wire
  value and the turn ends with an `Error`.
- A `FunctionCall` the parent answers with `not_found` raises `NameError`
  (matching CPython for a genuinely undefined *call*).

## Installing dependencies (`InstallDependencies`)

A parent can install third-party packages into a session before (or between)
feeds with the `InstallDependencies` request, carrying a list of PEP 508
requirement strings. The child shells out to:

```
uv pip install --python .venv/bin/python <requirements...>
```

then makes the venv importable on the embedded interpreter —
`site.addsitedir(<.venv site-packages>)` (so the venv's `.pth` files run, which
legacy namespace packages need) plus `importlib.invalidate_caches()` — so
subsequent feeds can `import` the packages. The turn ends with `Ok` on success or
`Error` (carrying uv's stderr, truncated) on failure. An empty requirement list
is a no-op `Ok`.

- **`uv` must be on `PATH`** (the deployment's Docker image installs it),
  overridable with the `MONTY_UV` env var.
- **Network access is required** — uv fetches from a package index.
- **Packages install into a virtualenv at `./.venv`**, relative to the worker's
  working directory. The image pre-creates it with `uv venv` (see the
  `Dockerfile`), pinned to the same Python 3.14 the worker embeds — so its
  `site-packages` is ABI-compatible — and sets the working directory so `./.venv`
  resolves. The venv is a deployment contract: if it is missing, the install
  fails with a clear error rather than silently creating one.
- **Installs are session-scoped.** The worker serves one session then exits, and
  the per-session sandbox (with its `.venv`) is discarded — so packages never
  leak into another session (see *One session per sandbox*).
- The Monty sandbox child (`monty subprocess`) rejects `InstallDependencies`
  with a session-preserving `Error` — it has no host interpreter to install for,
  and would never shell out to the host.

### PEP 723 inline dependencies

Independently of the `InstallDependencies` request, every `Feed` is scanned for
a [PEP 723](https://peps.python.org/pep-0723/) `# /// script` … `# ///` metadata
block before it runs; the `dependencies` it declares are installed (via the same
session-scoped `uv` path) so the snippet's imports resolve. This needs no
protocol support — it is entirely a worker concern, mirroring `uv run`.
Extraction is pure Rust (the spec's block regex + the `toml` crate), so it does
not depend on the embedded interpreter. A snippet with no metadata block is the
common fast path (a single regex, no uv). A malformed or duplicated block ends
the feed with an `Error` before execution. See `src/pep_723.rs`.

## One blocking host call at a time

The host-call model is synchronous: a `FunctionCall` blocks the interpreter until
its `ResumeCall` arrives, so only one external call is outstanding at a time.

- **Top-level `await` is supported.** The runner compiles snippets with
  `PyCF_ALLOW_TOP_LEVEL_AWAIT` and drives any resulting coroutine with
  `asyncio.run`, so `await`, `async for`, and `async with` work at module level
  (and `async def`s defined in the snippet run normally). A trailing `await`
  expression becomes the `Complete` value.
- **Host calls are not awaitable.** An undefined name resolves to a *synchronous*
  blocking proxy that returns a plain value, so `await fetch(...)` raises
  `TypeError` (you cannot await a host call). Call host functions without `await`;
  use `await` only for coroutines defined inside the snippet or in the stdlib.
- **Async external functions are not supported** — an `ExtFunctionResult::future`
  answer from the parent raises `RuntimeError`.

## Other behaviour notes

- **Resource limits / timeouts** in `Configure.limits` are ignored: the child
  has no `ResourceTracker`. Wall-clock timeouts are still enforced by the
  parent's watchdog (it kills the connection). `total_execution_micros` /
  `max_duration_micros` on events are always zero.
- **Type checking** (`Configure.type_check`, `Feed.skip_type_check`) is
  ignored — snippets are always executed, never type-checked.
- **`Configure.script_name`** is the filename reported for every sandbox frame
  in a traceback. Internally each feed compiles under a unique `<input-N>` name
  (with its source registered in `linecache`, so previews resolve even for a
  frame from a function defined in an earlier feed); `script_name` is substituted
  in when the traceback is rebuilt.
- **Error tracebacks** are reconstructed from the CPython traceback and sent in
  `RaisedException.traceback`: one frame per *user* frame (the `runner.py` driver
  frames — `run`/`drive_async`/`eval` — are filtered out), each with the
  `script_name` filename, line number, function name (`<module>` for top-level
  code), source-line preview, and caret span. The caret column span comes from
  CPython's reported offsets, rendered in Monty's uniform `~~~` style rather than
  CPython's two-tone `~~~^^^`. Caret *visibility*, however, is a rough heuristic,
  not CPython's exact anchor-aware decision: a caret line is drawn for every frame
  except `raise` statements. So CPython's other no-caret cases — attribute access,
  a bare-name lookup on its own line, and full-line `x = f()` / `return f()` calls
  — get a whole-line underline here where CPython draws none. `SyntaxError`s raise
  during compilation, so their frames are runner-internal and filtered out — only
  the type and message survive.
- **Mounts** (`Feed.mounts`) are ignored; the child performs no virtual
  filesystem mapping. Real filesystem access goes straight to the host FS
  (see the security note above).
- **`print()`**: both `sys.stdout` and `sys.stderr` are streamed to the parent
  as `Print` events, each tagged with its stream (`Stdout` / `Stderr`).
- **Values**: the Python ↔ wire value model is `pydantic_monty`'s shared
  conversion layer, so the supported types and their divergences (e.g.
  dataclasses do not round-trip to their original type) match `pydantic_monty`.
- **REPL semantics**: a trailing expression becomes the `Complete` value; a
  snippet ending in a statement completes with `None`.
