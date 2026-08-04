# Rust QuickStart

There are two ways to run Monty from Rust, and the choice matters.

- **[`monty-pool`](https://crates.io/crates/monty-pool)** runs the interpreter only in
  `monty` worker subprocesses. Use this for untrusted code. It is the same engine the
  Python and JavaScript packages are built on.
- **[`monty`](https://crates.io/crates/monty)** is the in-process interpreter. Use it when
  you control the code being run, or when subprocesses are impossible.

A Monty process can never be made fully crash-proof against memory errors — a
stack-overflow abort or an allocator abort takes the whole process down. That is the
entire reason `monty-pool` exists: the crash kills a worker, the pool notices and
replaces it, and your process is untouched.

## Running untrusted code with `monty-pool`

```bash
cargo add monty-pool monty-types tokio
```

Workers are `monty` CLI binaries. Build one with `cargo build -p monty-runtime` from the
[Monty repository](https://github.com/pydantic/monty), or install it from PyPI as
[`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/).

```rust,no_run
use monty_pool::{Pool, PoolConfig, PoolError, ReplConfig, TurnEvent, on_print_sync};

#[tokio::main]
async fn main() -> Result<(), PoolError> {
    let pool = Pool::new(PoolConfig::subprocess("path/to/monty")).await?;

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

`Checkout::feed` takes the code, inputs (host values bound as sandbox globals), per-feed
filesystem mounts (`MountSpec`), a `skip_type_check` flag and a print sink. It returns a
`TurnEvent`:

| `TurnEvent` | Meaning | Answer with |
| --- | --- | --- |
| `Complete(value)` | The snippet finished | nothing; feed again |
| `FunctionCall { .. }` | The sandbox called a host function | `Checkout::resume` |
| `OsCall { .. }` | The sandbox performed an OS operation | `Checkout::resume_from_mounts` or `resume` |
| `NameLookup { name }` | The sandbox read an undefined name | `Checkout::resume_name_lookup` |
| `ResolveFutures { .. }` | Every sandbox task is blocked on host futures | `Checkout::resume_futures` |

A `Checkout` dropped without `finish()` kills its worker rather than returning it —
mid-execution state cannot be trusted back into the pool.

`ReplConfig` carries the per-session sandbox `ResourceLimits` and type-checking options.
`Checkout::dump` and `Checkout::restore` snapshot and restore a session, including onto a
different worker or machine.

### What the pool adds over in-process execution

- **Crash isolation** — a segfault, stack-overflow abort or allocator abort in the
  sandbox surfaces as `PoolError::Crashed`; the pool discards the worker and spawns a
  replacement.
- **Hard timeouts** — a parent-side deadline kills any worker whose turn exceeds
  `request_timeout` (`PoolError::Timeout`), catching hangs the in-sandbox limits cannot
  see. With a `max_duration` budget the deadline also enforces that from outside the
  child, plus `duration_limit_grace`.
- **Untrusted children** — every frame from a possibly compromised worker is validated;
  wire decoding never panics, and a protocol violation discards the worker.
- **Worker recycling** — `max_checkouts_per_worker` bounds the impact of a slow leak.

Runtime errors inside the sandbox (`PoolError::Runtime`) are not crashes: the worker and
its session stay alive and usable.

### Transports

`PoolConfig::subprocess` spawns local `monty subprocess` children over framed stdio.
These are the poolable workers: prewarmed, reused across checkouts, replaced on crash.

`PoolConfig::websocket` dials a remote child over `ws://`/`wss://`. Those workers are
single-use, never prewarmed or returned to the pool, and isolation becomes the remote
host's responsibility. See [the security model](../security.md#remote-workers) before
using it.

## The in-process interpreter

```bash
cargo add monty monty-types
```

`MontyRun` parses and compiles code once; `run` executes it with input values and returns
the value of the final expression as a `MontyObject`:

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

Errors come back as `MontyException`, with a traceback matching what CPython would
produce. `PrintWriter` controls where `print()` output goes: `Stdout`, `Disabled`, or
collected into a `String` or `(stream, text)` tuples.

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

`MontyRun::start` returns a `RunProgress` that pauses whenever the sandboxed code calls a
function the host provides. The host runs the real function and resumes with the result:

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

Async host functions work the same way: `FunctionCall::resume_pending` continues with a
pending future the sandboxed code can `await`, and when every task is blocked the run
yields `RunProgress::ResolveFutures` for the host to settle.

### Serialization

A paused `RunProgress` is a self-contained snapshot: `dump()` it, store the bytes, and
`load()` it back later in a different process. `MontyRun` itself dumps and loads too,
which caches parsed code:

```rust
use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

let runner = MontyRun::new("x + 1".to_owned(), "main.py", vec!["x".to_owned()], CompileOptions::default()).unwrap();
let bytes = runner.dump().unwrap();

// later, restore and run
let runner2 = MontyRun::load(&bytes).unwrap();
let result = runner2.run(vec![MontyObject::Int(41)], ResourceTracker::default(), PrintWriter::Stdout).unwrap();
assert_eq!(result, MontyObject::Int(42));
```

### Other pieces

- `MontyRepl` — feed code snippet by snippet with state persisting between snippets.
- The `fs` module — mount host directories into the sandbox at virtual paths, with path
  resolution hardened against escapes. See [filesystem access](../filesystem.md).
- `RunProgress::OsCall` — the filesystem and `os`-level operations the host intercepts.

## Which crate depends on what

Host-side crates (`monty-fs`, `monty-pool`, `monty-proto` without its `worker` feature,
the Python and JavaScript bindings) depend on
[`monty-types`](https://crates.io/crates/monty-types), never on `monty`. That keeps the
interpreter out of the parent process entirely. Only the worker side links it.
