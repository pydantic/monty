# monty-types

Shared boundary types for [Monty](https://github.com/pydantic/monty), the
sandboxed Python interpreter — the owned, heap-free data types that cross
between the interpreter and the hosts that embed it, with **no interpreter
implementation**.

## What's here

- `MontyObject` / `MontyType` — Python values and their types at the host
  boundary, including the `datetime` family (`MontyDate`, `MontyDateTime`,
  `MontyTimeDelta`, `MontyTimeZone`), `DictPairs` and `MontyFileHandle`.
- `MontyException` / `ExcType` — exceptions with tracebacks (`StackFrame`,
  `CodeLoc`) and structured payloads (`ExcData`).
- `OsFunctionCall` — the typed OS-call payloads sandboxed code suspends with
  (file reads/writes, `open()`, `os.getenv`, ...), plus the `stat_result`
  builders hosts use to answer them.
- `ResourceTracker` / `ResourceLimits` — the resource-limit trait the
  interpreter is generic over, with the stock `NoLimitTracker` and
  `LimitedTracker` implementations.
- `PrintStream` / `PrintWriter` — `print()` output capture.
- `CompileOptions`, `ExtFunctionResult`, `NameLookupResult`, `FileMode`, and
  the CPython-compatible formatting helpers behind their `repr()`s.

## Who should depend on it

Host-side crates that need these types without linking the interpreter —
`monty-fs` (which services `OsFunctionCall`s locally via
`MountTable::handle_os_call`), `monty-pool` (which talks to Monty workers
over the wire), the `pydantic-monty` Python bindings and the
`@pydantic/monty` JS bindings — depend on this crate **instead of `monty`**,
so their binaries never link the interpreter itself. Only worker-side crates
(`monty-runtime`, `monty-wasm-runtime`, and `monty-proto` with its `worker`
feature) link `monty`.

```rust
use monty_types::MontyObject;

let value = MontyObject::List(vec![MontyObject::Int(1), MontyObject::String("x".to_owned())]);
assert_eq!(value.py_repr(), "[1, 'x']");
```

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter. **this crate**
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.

## License

MIT
