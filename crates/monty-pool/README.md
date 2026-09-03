# monty-pool

A pool of [Monty](https://github.com/pydantic/monty) worker processes for running untrusted
Python code with crash isolation.

Monty executes untrusted Python, and a Monty process can never be made fully crash-proof
against memory errors (stack overflow aborts, allocator aborts). This crate isolates those
crashes by running the interpreter **only in worker subprocesses**, reached over Monty's wire
protocol: a crashed worker kills only itself, the pool detects the death and replaces the
worker, and the parent process is never at risk.

This is the recommended way to run Monty from Rust. It is also the engine underneath the
[`pydantic-monty`](https://pypi.org/project/pydantic-monty/) Python package and the
[`@pydantic/monty`](https://www.npmjs.com/package/@pydantic/monty) JavaScript package.

## Model

A `Pool` keeps an elastic set of workers (`min_processes` prewarmed, up to `max_processes`).
`Pool::checkout` dedicates one worker to one REPL session: the caller feeds snippets of code
and answers suspension events (`TurnEvent` — external function calls, OS calls, name lookups,
async futures) until the snippet completes, then `Checkout::finish` returns the worker to the
pool for reuse. A `Checkout` dropped without `finish` kills its worker instead — mid-execution
state cannot be trusted back into the pool.

The pool is async end-to-end and runs on [tokio](https://tokio.rs): frame reads are
cancel-safe (partial-frame state lives in the worker), and turn deadlines are ordinary timers
rather than a watchdog thread. Turn futures are not resumable after being dropped mid-flight —
the checkout notices, discards the worker, and fails the next call cleanly.

## Usage

Workers are `monty` CLI binaries spawned as subprocesses — build one with
`cargo build -p monty-runtime` in the [Monty repository](https://github.com/pydantic/monty), or
install it from PyPI as [`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/).

```rust,no_run
use std::time::Duration;

use monty_pool::{Pool, PoolConfig, PoolError, ReplConfig, TurnEvent, on_print_sync};

#[tokio::main]
async fn main() -> Result<(), PoolError> {
    let mut config = PoolConfig::subprocess("path/to/monty");
    // no timeouts by default; set one before running untrusted code
    config.request_timeout = Some(Duration::from_secs(30));
    let pool = Pool::new(config).await?;

    let mut session = pool.checkout(&ReplConfig::default()).await?;
    let mut on_print = on_print_sync(|_stream, text| print!("{text}"));

    // session state persists between feeds on the same checkout
    session.feed("x = 21", vec![], vec![], false, &mut on_print).await?;
    let event = session.feed("x * 2", vec![], vec![], false, &mut on_print).await?;
    match event {
        TurnEvent::Complete(value) => println!("result: {value:?}"), // Int(42)
        // other events are suspensions (external function calls, OS calls,
        // name lookups, futures) answered with `resume` / `resume_name_lookup`
        // / `resume_futures` to continue the turn
        other => println!("suspended: {other:?}"),
    }

    // return the worker to the pool for reuse by the next checkout
    session.finish().await?;
    Ok(())
}
```

`ReplConfig` also enables per-session sandbox `ResourceLimits` and type checking of every fed
snippet; `Checkout::feed` accepts inputs (host values exposed as sandbox globals) and
per-feed filesystem mounts (`MountSpec`). Sessions can be snapshotted with `Checkout::dump`
and restored later — including on a different worker or machine — with `Checkout::restore`.

## Protections over in-process execution

- **Crash isolation** — a segfault, stack-overflow abort, or allocator abort in the sandbox
  kills only the worker. The pool observes the death as `PoolError::Crashed`, discards the
  worker, and spawns a replacement; the parent process and every other session stay healthy.
- **Hard timeouts** — a parent-side deadline kills any worker whose turn exceeds
  `request_timeout` (`PoolError::Timeout`), backstopping the sandbox's own resource limits
  and catching hangs those limits cannot see. Synchronous host telemetry processors delay
  enforcement while they run because the timer cannot be polled. When a session has a `max_duration` budget,
  the deadline also enforces it (plus `duration_limit_grace`) from outside the child.
  A `max_suspensions` budget is enforced by the pool alone: it counts the suspensions it services
  and ends the feed past the budget with an uncatchable `RuntimeError` in the sandbox.
  `PoolConfig::subprocess` sets neither `request_timeout` nor `checkout_timeout` by
  default; set `request_timeout` yourself for untrusted code.
- **Untrusted children** — the parent treats every frame from a (possibly compromised)
  worker as untrusted: wire decoding validates everything and never panics, and a worker
  that violates the protocol is discarded.
- **Worker recycling** — `max_checkouts_per_worker` recycles long-lived children to bound
  the impact of any slow leak.
- **Memory limits** — a session's `max_memory` also caps the worker's live allocations,
  enforced in the worker's own global allocator
  ([`monty-alloc`](https://crates.io/crates/monty-alloc)) plus 4 MB of headroom (32 MB with
  type checking), rather than letting a worker grow the host until the OOM killer
  intervenes. Exceeding it, or a refused allocation, exits the worker with a dedicated code
  so it is reported as `PoolError::Runtime`/`MemoryError` instead of an unclassifiable
  abort — the one `Runtime` error whose worker does not survive.

Runtime errors inside the sandbox (`PoolError::Runtime`) are not crashes: the worker and its
session remain alive and usable — the one exception being the `MemoryError` above, raised for
a worker that has already exited.

## Observability

The optional `telemetry-adapter` feature records semantic execution for language bindings
and other hosts. `monty-pool` never selects an exporter, reads credentials or environment
variables, or shuts an exporter down; the host SDK owns those choices and its final
flush/shutdown.

Recording happens in the host process, which builds every request and decodes every event
anyway, so both transports are covered and the workers stay uninstrumented. Each instrumented checkout
becomes one session span; each feed is a nested span held across suspension round-trips, with
a child span per suspension whose duration is the host round-trip. Fed code, inputs, call
arguments and results, exceptions and `print` output are recorded in full — values encoded
the way the Python logfire SDK encodes attributes, capped at 64KB per value — while
`Load`/`Dump` snapshot blobs are recorded by size only. Supplying an SDK is therefore an
explicit opt-in to recording potentially sensitive values.

The adapter configures an exporter-free process-global Rust pipeline and returns a handle
that creates each checkout's serialized parent context. Records are emitted through
`TelemetryAdapter`; Python, Node, and third-party bindings retain ownership of their native
SDK and exporter. Without the feature, workers contain no telemetry recorder or telemetry
hot path.

## Transports

- **Subprocess** (`PoolConfig::subprocess`) — spawn local `monty subprocess` children over
  framed stdio. These are the poolable workers: prewarmed, reused across checkouts, and
  replaced on crash.
- **WebSocket** (`PoolConfig::websocket`) — dial a remote child (or a relay pairing the two
  ends) over `ws://`/`wss://`. These workers are single-use: dialed fresh per checkout,
  never prewarmed or returned to the pool. Isolation is the remote host's responsibility —
  a remote crash is observed as the connection dropping.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses. **this crate**
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
