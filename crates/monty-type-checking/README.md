# monty-type-checking

Type checking for [Monty](https://github.com/pydantic/monty), powered by
[ty](https://docs.astral.sh/ty/).

Monty supports full modern Python type hints. This crate embeds ty's semantic
analysis (Astral's `ty_python_semantic` engine — the same one behind the `ty`
type checker) and checks code against
[`monty-typeshed`](https://crates.io/crates/monty-typeshed): a trimmed
typeshed describing the stdlib subset Monty actually implements. Code that
uses unsupported stdlib surface is therefore flagged *before* it runs rather
than failing at runtime.

It backs `monty --type-check` in the CLI and the `type_check` option on
sessions in the [`pydantic-monty`](https://pypi.org/project/pydantic-monty/)
and [`@pydantic/monty`](https://www.npmjs.com/package/@pydantic/monty)
packages.

## Usage

```rust
use monty_type_checking::{SourceFile, type_check};

let source = SourceFile::new("x: int = 'not an int'", "main.py");
let diagnostics = type_check(&source, None).unwrap();
// `Some(...)` means typing errors were found; `None` means the code is clean
assert!(diagnostics.is_some());
```

The second argument is an optional stubs file declaring names the host will
provide at runtime (external functions, inputs). The stubs are written
alongside the source and a `from <stubs> import *` line is injected, so
checked code can reference host functions without defining them — diagnostic
line numbers are adjusted back to the original source.

`TypeCheckingDiagnostics` renders ty's full diagnostic output (source
context, underlines, and optional ANSI color) via its `Display`
implementation.

## Pooled databases

Each check leases a pre-configured in-memory [salsa](https://github.com/salsa-rs/salsa)
database from a small process-wide pool instead of rebuilding typeshed-derived
semantic state per call. Leased databases are scrubbed of the checked files
and returned to the pool when the check (or the diagnostics value it
produced) is dropped, keeping salsa's single-writer invariant while allowing
concurrent checks on different databases.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/). **this crate**
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
