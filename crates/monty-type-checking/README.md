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
use monty_type_checking::{SourceFile, TypeChecker};
use monty_types::TypeCheckingConfig;

let mut checker = TypeChecker::default();
let source = SourceFile::new("x: int = 'not an int'", "main.py");
let diagnostics = checker.run(&source, None, TypeCheckingConfig::default()).unwrap();
// `Some(...)` means typing errors were found; `None` means the code is clean
assert!(diagnostics.is_some());
```

The second argument is an optional stubs file declaring names the host will
provide at runtime (external functions, inputs). The stubs are written
alongside the source and a `from <stubs> import *` line is injected, so
checked code can reference host functions without defining them — diagnostic
line numbers are adjusted back to the original source.

`TypeCheckingConfig` picks the output format (ty's `full`, `concise`, `json`,
`github`, ... renderings) and whether to use ANSI colour. It is passed to
`run` rather than applied to the result because `TypeCheckingDiagnostics`
borrows the checker — ty's diagnostics resolve their spans against the
database that produced them, so anything that outlives the checker (notably
anything crossing a process boundary) has to keep the rendered string.

## Reusing a checker

A `TypeChecker` owns one in-memory [salsa](https://github.com/salsa-rs/salsa)
database, so reusing it across checks avoids rebuilding typeshed-derived
semantic state every time — which is what makes per-feed checking in a REPL
session affordable. Files written by a previous `run` stay in the database
(rewritten in place when the path repeats), so call `reset` before checking an
unrelated session's code: it scrubs every file written so far, including the
directories they created. `TypeChecker` is not `Sync`; concurrent checks each
need their own.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/). **this crate**
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
