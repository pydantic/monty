# Subprocess execution (`monty --subprocess`, `monty-pool`, `Monty`/`AsyncMonty`)

Monty's Python package runs the interpreter exclusively in worker
subprocesses driven over a protobuf protocol (`crates/monty-proto`), so that
crashes a sandbox can never fully prevent — stack overflow aborts, allocator
aborts — kill only the worker. The language semantics inside a worker are
identical to embedding the interpreter directly (it is the same interpreter);
the notes below are about the *host API* surface.

## Execution model

- The protocol (and `pydantic_monty`) is **REPL-only**: a pool checkout is a
  REPL session in a dedicated worker, and a one-shot run is a checkout plus a
  single feed. There are no manual suspension snapshots in Python; external
  function calls, OS callbacks, and print callbacks are driven automatically
  by `feed_run` / `feed_run_async`. (The Rust `monty-pool::Checkout` API does
  expose manual suspension driving and `Pool::checkout_load`.)
- A session whose worker crashed is lost: subsequent calls raise
  `MontyCrashedError`. The pool itself recovers by replacing the worker.
- Resource exhaustion (e.g. `max_duration_secs`) is terminal for the
  *session*: later feeds keep failing with the same resource error. The
  worker process is reused for the next checkout.
- Ctrl-C / asyncio cancellation cannot interrupt a protocol turn already
  blocked on the worker; use sandbox `limits` and/or the pool's
  `request_timeout` (which kills the worker).

## Values crossing the process boundary

- Values are encoded as protobuf (`proto/monty/v1/monty.proto`); every
  `MontyObject` variant round-trips, but nesting depth is bounded by prost's
  decode recursion limit (~50 levels of list/dict nesting). A deeper result
  value fails the protocol turn rather than crossing the boundary.
- `Cycle` markers (self-referential containers) can be *received* from a
  worker but are rejected as inputs.

## Host-API behaviour notes

- **Typing errors** (`checkout(type_check=True)`) raise `MontyTypingError`
  whose diagnostics were rendered in the worker with the default format —
  `display()` takes no arguments.
- **Print callbacks** receive line-buffered chunks (one call per line or
  8 KiB), not per-fragment writes. A callback that raises aborts the host
  call after the current protocol turn, not mid-`print`.
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
