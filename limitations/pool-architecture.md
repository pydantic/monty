# Subprocess execution (`monty --subprocess`, `monty-pool`, `MontyPool`)

Monty can run in worker subprocesses driven over a protobuf protocol
(`crates/monty-proto`), so that crashes a sandbox can never fully prevent —
stack overflow aborts, allocator aborts — kill only the worker process. The
language semantics inside a worker are identical to in-process execution
(it is the same interpreter); the divergences below are about the *host API*
surface compared to in-process `Monty` / `MontyRepl`.

## Execution model

- The protocol (and `pydantic_monty.MontyPool`) is **REPL-only**: a one-shot
  run is a checkout plus a single feed. There is no pooled equivalent of the
  `Monty` class, of `feed_start()` manual snapshots, or of
  `load_snapshot()`-style resumable objects (the Rust `monty-pool::Checkout`
  API does expose manual suspension driving and `Pool::checkout_load`).
- A session whose worker crashed is lost: subsequent calls raise
  `MontyCrashedError`. The pool itself recovers by replacing the worker.
- Resource exhaustion (e.g. `max_duration_secs`) is terminal for the
  *session*, exactly like in-process: later feeds keep failing with the same
  resource error. The worker process is reused for the next checkout.

## Values crossing the process boundary

- Values are encoded as protobuf (`proto/monty/v1/monty.proto`); every
  `MontyObject` variant round-trips, but nesting depth is bounded by prost's
  decode recursion limit (~50 levels of list/dict nesting). A deeper result
  value fails the protocol turn rather than crossing the boundary.
- `Cycle` markers (self-referential containers) can be *received* from a
  worker but are rejected as inputs, matching in-process semantics.

## Behavioural divergences from in-process `MontyRepl`

- **Typing errors** (`type_check=True`) raise `MontyTypingError` whose
  diagnostics were rendered in the worker with the default format —
  `display(format=..., color=...)` ignores its arguments.
- **Print callbacks** receive line-buffered chunks (one call per line or
  8 KiB), not the per-fragment writes of in-process execution. A callback
  that raises aborts the *host* call after the current protocol turn, not
  mid-`print`.
- **Overlay mounts** (`mode='overlay'`) live in the worker: writes are
  discarded when the feed ends, and the host `MountDir` object's overlay
  state is never updated.
- **`dump()`** bytes use a subprocess-specific envelope and can only be
  restored into another subprocess worker (Rust `Pool::checkout_load`); they
  are not interchangeable with `MontyRepl.dump()` / `load_repl_snapshot()`.
- **Ctrl-C / cancellation** cannot interrupt a protocol turn already blocked
  on the worker; use sandbox `limits` and/or the pool's `request_timeout`
  (which kills the worker). Cancelling the surrounding asyncio task abandons
  the session; the worker is killed when the session exits.
- **`os=` fallback** receives `(function_name, args, kwargs)` exactly as
  in-process, but mount-covered filesystem calls are handled inside the
  worker and never reach the callback.
