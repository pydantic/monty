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

## Usage

Workers are `monty` CLI binaries spawned as subprocesses — build one with
`cargo build -p monty-runtime` in the [Monty repository](https://github.com/pydantic/monty), or
install it from PyPI as [`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/).

```rust,no_run
use monty_pool::{Pool, PoolConfig, PoolError, ReplConfig, TurnEvent};

fn main() -> Result<(), PoolError> {
    let pool = Pool::new(PoolConfig::subprocess("path/to/monty"))?;

    let mut session = pool.checkout(&ReplConfig::default())?;
    let mut on_print = |_stream, text: &str| print!("{text}");

    // session state persists between feeds on the same checkout
    session.feed("x = 21", vec![], vec![], false, &mut on_print)?;
    let event = session.feed("x * 2", vec![], vec![], false, &mut on_print)?;
    match event {
        TurnEvent::Complete(value) => println!("result: {value:?}"), // Int(42)
        // other events are suspensions (external function calls, OS calls,
        // name lookups, futures) answered with `resume` / `resume_name_lookup`
        // / `resume_futures` to continue the turn
        other => println!("suspended: {other:?}"),
    }

    // return the worker to the pool for reuse by the next checkout
    session.finish()?;
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
- **Hard timeouts** — a parent-side watchdog kills any worker whose turn exceeds
  `request_timeout` (`PoolError::Timeout`), backstopping the sandbox's own resource limits
  and catching hangs those limits cannot see. When a session has a `max_duration` budget,
  the watchdog also enforces it (plus `duration_limit_grace`) from outside the child.
- **Untrusted children** — the parent treats every frame from a (possibly compromised)
  worker as untrusted: wire decoding validates everything and never panics, and a worker
  that violates the protocol is discarded.
- **Worker recycling** — `max_checkouts_per_worker` recycles long-lived children to bound
  the impact of any slow leak.

Runtime errors inside the sandbox (`PoolError::Runtime`) are not crashes: the worker and its
session remain alive and usable.

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
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses. **this crate**
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
