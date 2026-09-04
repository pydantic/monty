# Getting Started with Rust

## Installation

For running untrusted code, use [`monty-pool`](https://crates.io/crates/monty-pool):

```bash
cargo add monty-pool monty-types tokio --features tokio/macros,tokio/rt-multi-thread
```

Workers are `monty` CLI binaries: build one with `cargo build -p monty-runtime` from the
[Monty repository](https://github.com/pydantic/monty), or install it from PyPI as
[`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/).

The in-process interpreter is the [`monty`](https://crates.io/crates/monty) crate:

```bash
cargo add monty monty-types
```

| Crate                                                                 | What it is                                                  |
| --------------------------------------------------------------------- | ----------------------------------------------------------- |
| [`monty`](https://crates.io/crates/monty)                             | The core interpreter: Python parser, bytecode VM, sandbox   |
| [`monty-types`](https://crates.io/crates/monty-types)                 | Shared boundary types: values, exceptions, OS calls, limits |
| [`monty-fs`](https://crates.io/crates/monty-fs)                       | Host-side filesystem mounts                                 |
| [`monty-runtime`](https://crates.io/crates/monty-runtime)             | The `monty` binary: REPL, file runner, subprocess worker    |
| [`monty-pool`](https://crates.io/crates/monty-pool)                   | Elastic pool of crash-isolated worker subprocesses          |
| [`monty-proto`](https://crates.io/crates/monty-proto)                 | The protobuf wire protocol between pool parents and workers |
| [`monty-type-checking`](https://crates.io/crates/monty-type-checking) | Type checking, powered by ty                                |
| [`monty-typeshed`](https://crates.io/crates/monty-typeshed)           | Trimmed typeshed stubs for Monty's stdlib subset            |

Host-side crates depend on `monty-types`, never on `monty`, so the interpreter is not linked into your parent process
at all.
The [Rust API](../api/rust/monty.md) pages document `monty`, `monty-pool`, `monty-types`, `monty-fs`, `monty-proto` and
`monty-type-checking`.

## Two ways to run Monty

- **[`monty-pool`](../api/rust/monty-pool.md)** runs the interpreter only in `monty` worker subprocesses.
    Use this for untrusted code.
    It is the same engine the Python and JavaScript packages are built on.
- **[`monty`](../api/rust/monty.md)** is the in-process interpreter.
    Use it when you control the code being run, or when subprocesses are impossible.

A Monty process can never be made fully crash-proof against memory errors — a stack-overflow abort or an allocator abort
takes the whole process down.
That is the entire reason `monty-pool` exists: the crash kills a worker, the pool notices and replaces it, and your
process is untouched.

## Running untrusted code with `monty-pool`

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

`Checkout::feed` takes the code, inputs (host values bound as sandbox globals), per-feed filesystem mounts
(`MountSpec`), a `skip_type_check` flag and a print sink.
It returns a `TurnEvent`:

| `TurnEvent`                      | Meaning                                                                                                                                                                | Answer with                                |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| `Complete(value)`                | The snippet finished                                                                                                                                                   | nothing; feed again                        |
| `FunctionCall { object_id, .. }` | The sandbox called a host function, or with `object_id` (the wrapper's uuid) `Some`, a method on a host object or a host class's construction (arriving as `__call__`) | `Checkout::resume`                         |
| `OsCall { .. }`                  | The sandbox performed an OS operation                                                                                                                                  | `Checkout::resume_from_mounts` or `resume` |
| `NameLookup { name, object_id }` | The sandbox read an undefined name, or a lazy attribute of a host object when `object_id` is `Some`                                                                    | `Checkout::resume_name_lookup`             |
| `ResolveFutures { .. }`          | Every sandbox task is blocked on host futures                                                                                                                          | `Checkout::resume_futures`                 |

A `Checkout` dropped without `finish()` kills its worker rather than returning it — mid-execution state cannot be
trusted back into the pool.

`ReplConfig` carries the per-session sandbox `ResourceLimits` and type-checking options.
`Checkout::dump` and `Checkout::restore` snapshot and restore a session, including onto a different worker or machine.

### What the pool adds over in-process execution

- **Crash isolation** — a segfault, stack-overflow abort or allocator abort in the sandbox becomes `PoolError::Crashed`;
    the pool discards the worker and spawns a replacement.
- **Hard timeouts** — a parent-side deadline kills any worker whose turn exceeds `request_timeout`
    (`PoolError::Timeout`), catching hangs the in-sandbox limits cannot see.
    With a `max_duration` budget the deadline also enforces that from outside the child, plus `duration_limit_grace`.
    `PoolConfig::subprocess` sets neither `request_timeout` nor `checkout_timeout` by default; set `request_timeout`
    yourself for untrusted code.
- **Suspension limits** — the pool counts external calls, OS calls, name lookups and future-resolution turns against
    `ResourceLimits::max_suspensions`.
    The first suspension over the limit ends the feed with an uncatchable `RuntimeError`.
- **Untrusted children** — every frame from a possibly compromised worker is validated; wire decoding never panics, and
    a protocol violation discards the worker.
- **Worker recycling** — `max_checkouts_per_worker` bounds the impact of a slow leak.

Runtime errors inside the sandbox (`PoolError::Runtime`) are not crashes: the worker and its session stay alive and
usable.
Memory and time limits return `PoolError::Runtime` with a `MemoryError` or `TimeoutError`, but
[no guarantees hold about heap state afterwards](../resource-limits.md#after-a-limit-fires).
A spent `max_duration` rejects every later `feed`.
Finish the checkout and take a fresh one.

`max_suspensions` also returns `PoolError::Runtime`, but leaves the session consistent.
Later feeds run until they suspend; the count remains spent.

### Transports

`PoolConfig::subprocess` spawns local `monty subprocess` children over framed stdio.
These are the poolable workers: prewarmed, reused across checkouts, replaced on crash.

`PoolConfig::websocket` dials a remote child over `ws://`/`wss://`.
Those workers are single-use, never prewarmed or returned to the pool, and isolation becomes the remote host's
responsibility.
See [the security model](../security.md#remote-workers) before using it.

## The in-process interpreter

`MontyRun` parses and compiles code once; `run` executes it with input values and returns the value of the final
expression as a `MontyObject`:

```rust
use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

let code = r#"
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(x)
"#;

let runner = MontyRun::new(code.to_owned(), "fib.py", vec!["x".to_owned()], CompileOptions::default()).unwrap();
let result = runner.run(vec![MontyObject::Int(10)], ResourceTracker::default(), PrintWriter::Stdout).unwrap();
assert_eq!(result, MontyObject::Int(55));
```

Errors come back as `MontyException`, with a traceback matching what CPython would produce.
`PrintWriter` controls where `print()` output goes: `Stdout`, `Disabled`, or collected into a `String` or `(stream, text)` tuples.

### Resource limits

```rust
use std::time::Duration;

use monty::MontyRun;
use monty_types::{CompileOptions, PrintWriter, ResourceLimits, ResourceTracker};

let limits = ResourceLimits {
    max_memory: Some(10 * 1024 * 1024),
    max_duration: Some(Duration::from_millis(20)),
    ..ResourceLimits::default()
};

let runner = MontyRun::new("while True: pass".to_owned(), "spin.py", vec![], CompileOptions::default()).unwrap();
let err = runner.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout).unwrap_err();
assert!(err.to_string().contains("time limit exceeded"));
```

### Host functions and pausing

`MontyRun::start` returns a `RunProgress` that pauses whenever the sandboxed code calls a function the host provides.
The host runs the real function and resumes with the result:

```rust
use monty::{MontyRun, RunProgress};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

let code = "data = get_data(3)\ndata * 2";
let runner = MontyRun::new(code.to_owned(), "main.py", vec!["get_data".to_owned()], CompileOptions::default()).unwrap();

// pass the external function in as an input
let get_data = MontyObject::Function { name: "get_data".to_owned(), docstring: None };
let progress = runner.start(vec![get_data], ResourceTracker::default(), PrintWriter::Stdout).unwrap();

// execution pauses at the `get_data(3)` call
let RunProgress::FunctionCall(call) = progress else { panic!("expected a function call") };
assert_eq!(call.function_name, "get_data");
assert_eq!(call.args, vec![MontyObject::Int(3)]);

// the host computes the result and resumes
let progress = call.resume(MontyObject::Int(21), PrintWriter::Stdout).unwrap();
let RunProgress::Complete(result) = progress else { panic!("expected completion") };
assert_eq!(result, MontyObject::Int(42));
```

Async host functions work the same way: `FunctionCall::resume_pending` continues with a pending future the sandboxed
code can `await`, and when every task is blocked the run yields `RunProgress::ResolveFutures` for the host to settle.

`FunctionCall`, `OsCall`, `NameLookup` and `ResolveFutures` expose `abort`, which raises a host-supplied
`MontyException` uncatchably at the suspension point and unwinds the run with a traceback.
A host driving the interpreter directly must count suspensions and call `abort` to enforce `max_suspensions`;
`ResourceTracker` stores that limit but does not enforce it.

### Serialization

The free function `monty::dump` serializes a session — idle between feeds (`SessionRef::Idle`) or suspended mid-run
(`SessionRef::Suspended`) — together with its script name and type-check state.
`Dump::load` restores it, in the same process or a different one:

```rust
use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
repl.feed_run("x = 40", vec![], PrintWriter::Stdout).unwrap();

// dumping is read-only: the live session can keep feeding
let bytes = dump("repl.py", None, SessionRef::Idle(&repl)).unwrap();

// later, restore and keep going
let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else { panic!() };
let result = restored.feed_run("x + 2", vec![], PrintWriter::Stdout).unwrap();
assert_eq!(result, MontyObject::Int(42));
```

### Other pieces

- `MontyRepl` — feed code snippet by snippet with state persisting between snippets.
- The `fs` module — mount host directories into the sandbox at virtual paths, with path resolution hardened against
    escapes.
    See [filesystem access](../filesystem.md).
- `RunProgress::OsCall` and `RunProgress::NameLookup` — the filesystem/`os` operations and undefined-name reads the host
    intercepts.
- `FunctionCall::object_id` and `NameLookup::object_id` — set for method calls and lazy attribute lookups routed to a
    host object sent as `MontyObject::ClassInstance` or `MontyObject::Type`; the receiver is not in `args`.
