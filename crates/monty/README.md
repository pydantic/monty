# monty

[![CI](https://github.com/pydantic/monty/actions/workflows/ci.yml/badge.svg)](https://github.com/pydantic/monty/actions/workflows/ci.yml?query=branch%3Amain)
[![crates.io](https://img.shields.io/crates/v/monty.svg)](https://crates.io/crates/monty)
[![license](https://img.shields.io/github/license/pydantic/monty.svg?v=2)](https://github.com/pydantic/monty/blob/main/LICENSE)

The core interpreter crate of [Monty](https://github.com/pydantic/monty) — a minimal, secure Python interpreter written in Rust for use by AI.

**Experimental** — this project is still in development, and not ready for prime time.

Monty lets you safely run Python code written by an LLM inside your own process, without the cost, latency and complexity of a container based sandbox. It parses Python with [Ruff](https://github.com/astral-sh/ruff)'s parser and executes it on its own bytecode VM — no CPython, no FFI, no C dependencies. Startup takes microseconds, not hundreds of milliseconds.

The sandbox has no ambient access to the host: filesystem, environment and network are only reachable through external function calls and mounts that you explicitly provide.

This crate is the pure-Rust core. Most users want one of the bindings built on top of it:

- **Python**: [`pydantic-monty`](https://pypi.org/project/pydantic-monty/)
- **JavaScript/TypeScript**: [`@pydantic/monty`](https://www.npmjs.com/package/@pydantic/monty)
- **CLI**: the `monty` binary from the [`monty-runtime`](https://crates.io/crates/monty-runtime) crate

See the [project README](https://github.com/pydantic/monty) for the full feature matrix, motivation, and supported Python subset.

## Basic usage

`MontyRun` parses and compiles code once; `run` executes it with input values and returns the value of the final expression as a `MontyObject`:

```rust
use monty::{CompileOptions, MontyRun, MontyObject, NoLimitTracker, PrintWriter};

let code = r#"
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(x)
"#;

let runner = MontyRun::new(code.to_owned(), "fib.py", vec!["x".to_owned()], CompileOptions::default()).unwrap();
let result = runner.run(vec![MontyObject::Int(10)], NoLimitTracker, PrintWriter::Stdout).unwrap();
assert_eq!(result, MontyObject::Int(55));
```

Errors are returned as `MontyException`, with a traceback matching what CPython would produce. `PrintWriter` controls where `print()` output goes: `Stdout`, `Disabled`, or collected into a `String` / `(stream, text)` tuples for the host to inspect.

## Resource limits

Untrusted code shouldn't be able to hog the host. `LimitedTracker` enforces limits on memory, allocation count, execution time, GC interval and recursion depth; exceeding one terminates execution with a `ResourceError`:

```rust
use std::time::Duration;
use monty::{CompileOptions, MontyRun, LimitedTracker, PrintWriter, ResourceLimits};

let limits = ResourceLimits {
    max_memory: Some(10 * 1024 * 1024),
    max_duration: Some(Duration::from_millis(20)),
    ..ResourceLimits::new()
};

let runner = MontyRun::new("while True: pass".to_owned(), "spin.py", vec![], CompileOptions::default()).unwrap();
let err = runner.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout).unwrap_err();
assert!(err.to_string().contains("time limit exceeded"));
```

## External functions and snapshotting

The defining feature of the crate: instead of running to completion, `MontyRun::start` returns a `RunProgress` that pauses execution whenever the sandboxed code calls a function provided by the host. The host runs the real function (an API call, a database query, an LLM tool) and resumes with the result:

```rust
use monty::{CompileOptions, MontyRun, MontyObject, NoLimitTracker, PrintWriter, RunProgress};

let code = "data = get_data(3)\ndata * 2";
let runner = MontyRun::new(code.to_owned(), "main.py", vec!["get_data".to_owned()], CompileOptions::default()).unwrap();

// pass the external function in as an input
let get_data = MontyObject::Function { name: "get_data".to_owned(), docstring: None };
let progress = runner.start(vec![get_data], NoLimitTracker, PrintWriter::Stdout).unwrap();

// execution pauses at the `get_data(3)` call
let RunProgress::FunctionCall(call) = progress else { panic!("expected a function call") };
assert_eq!(call.function_name, "get_data");
assert_eq!(call.args, vec![MontyObject::Int(3)]);

// the host computes the result and resumes
let progress = call.resume(MontyObject::Int(21), PrintWriter::Stdout).unwrap();
let RunProgress::Complete(result) = progress else { panic!("expected completion") };
assert_eq!(result, MontyObject::Int(42));
```

A paused `RunProgress` is a self-contained snapshot of the interpreter: serialize it with `dump()`, store it in a file or database, and `load()` + resume it later — in a different process or on a different machine. `MontyRun` itself can also be dumped and loaded to cache parsed code:

```rust
use monty::{CompileOptions, MontyRun, MontyObject, NoLimitTracker, PrintWriter};

let runner = MontyRun::new("x + 1".to_owned(), "main.py", vec!["x".to_owned()], CompileOptions::default()).unwrap();
let bytes = runner.dump().unwrap();

// later, restore and run
let runner2 = MontyRun::load(&bytes).unwrap();
let result = runner2.run(vec![MontyObject::Int(41)], NoLimitTracker, PrintWriter::Stdout).unwrap();
assert_eq!(result, MontyObject::Int(42));
```

Async host functions are supported too: `FunctionCall::resume_pending` continues execution with a pending future the sandboxed code can `await`; when all tasks are blocked, execution yields `RunProgress::ResolveFutures` for the host to supply results.

## Other pieces

- `MontyRepl` — a REPL-style interface: feed code snippet by snippet with state persisting between snippets.
- `fs` module — mount real host directories into the sandbox at virtual paths (read-write, read-only, or copy-on-write in-memory overlay), with path resolution hardened against escapes.
- `RunProgress::OsCall` — filesystem and other `os`-level operations the host can intercept or delegate.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox. **this crate**
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.

## License

MIT
