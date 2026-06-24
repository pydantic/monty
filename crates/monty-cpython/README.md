# monty-cpython

A Monty wire-protocol child worker (like `monty subprocess`) that executes each
fed snippet in **embedded CPython** instead of Monty, routing undefined names back
to the parent as `FunctionCall`s. It lets a parent (`monty-pool` /
`pydantic_monty`) drive a *real* Python interpreter over the same protocol —
locally over stdio, or remotely over a WebSocket.

## Transports

The transport is selected by subcommand:

- `monty-cpython subprocess` — framed stdio, a drop-in worker for `monty-pool`
  (point `binary_path` at this binary).
- `monty-cpython connect <ws-url>` — dial a relay (or a parent-as-server) as a
  WebSocket client.
- `monty-cpython server <addr>` — bind and accept one parent connection (server
  mode).

The execution `globals` is a `dict` subclass whose `__missing__` turns any unbound
global that is not a builtin or dunder into a proxy; calling that proxy emits a
`FunctionCall` and blocks for the `ResumeCall`. All value conversion and transport
work happens in Rust; the Python glue is tiny (see `src/pyexec.rs`).

## SECURITY: not a sandbox

Full CPython is **not** a security boundary. A fed snippet can `import os`, open
files, spawn processes — anything this process can do. `monty-cpython` provides
**no isolation of its own**; isolation is entirely the deployment's
responsibility (run it inside a locked-down container, microVM, or a
relay-provisioned sandbox). This is the fundamental difference from the Monty
interpreter, which *is* a sandbox. Do not run `monty-cpython` on untrusted code
outside an externally-enforced jail.

## Supported vs rejected protocol requests

Supported: `StartSession`, `Feed`, `ResumeCall` (consumed inline during a feed,
never at the top level), `Reset`, `Shutdown`.

Rejected with a turn-ending `Error` (the session survives):

- **`Dump` / `Load`** — a feed suspends on a live C stack inside a blocking
  `__call__`, which cannot be serialized; snapshots are not supported.
- **`ResumeNameLookup`** — undefined names surface as `FunctionCall`s (see
  below), never as `NameLookup` suspensions, so this can never arrive.
- **`ResumeFutures`** — there is no async-future suspension (see async, below).

`StartSession` with a mismatched `monty_version` is fatal (`FatalError` + exit 4),
exactly like the Monty child; both are workspace-versioned so they match.

## Undefined-name model

The execution `globals` is a `dict` subclass whose `__missing__` turns any
unbound global that is **not a builtin and not a dunder** into a proxy. Calling
that proxy emits a `FunctionCall` and blocks for the `ResumeCall`. Consequences
that differ from CPython:

- An undefined name that is **referenced but never called** yields a proxy
  object instead of raising `NameError`. If such a proxy is the snippet's
  trailing expression (or otherwise returned), it cannot be converted to a wire
  value and the turn ends with an `Error`.
- `not_found` from the parent raises `NameError` (matching CPython for a genuinely
  undefined *call*).

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

- **Resource limits / timeouts** in `StartSession.limits` are ignored: the child
  has no `ResourceTracker`. Wall-clock timeouts are still enforced by the
  parent's watchdog (it kills the connection). `total_execution_micros` /
  `max_duration_micros` on events are always zero.
- **Type checking** (`StartSession.type_check`, `Feed.skip_type_check`) is
  ignored — snippets are always executed, never type-checked.
- **Mounts** (`Feed.mounts`) are ignored; the child performs no virtual
  filesystem mapping. Real filesystem access goes straight to the host FS
  (see the security note above).
- **`print()`**: only `stdout` is streamed as `Print` events. Output a snippet
  writes to `sys.stderr` goes to the worker process's real stderr, not the parent.
- **Values**: the Python ↔ wire value model is `pydantic_monty`'s shared
  conversion layer, so the supported types and their divergences (e.g.
  dataclasses do not round-trip to their original type) match `pydantic_monty`.
- **REPL semantics**: a trailing expression becomes the `Complete` value; a
  snippet ending in a statement completes with `None`.
