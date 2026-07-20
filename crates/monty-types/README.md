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

Host-side crates that talk to Monty workers over the wire — `monty-fs`,
`monty-pool`, the `pydantic-monty` Python bindings and the `@pydantic/monty`
JS bindings — depend on this crate **instead of `monty`**, so their binaries
never link the interpreter itself. Only the worker side (`monty-runtime`,
`monty-wasm-runtime`) links `monty`.

The `monty` crate depends on `monty-types` and re-exports everything, so if
you already depend on `monty` you can use these types under the names you
know (`monty::MontyObject`, ...).

```rust
use monty_types::MontyObject;

let value = MontyObject::List(vec![MontyObject::Int(1), MontyObject::String("x".to_owned())]);
assert_eq!(value.py_repr(), "[1, 'x']");
```
