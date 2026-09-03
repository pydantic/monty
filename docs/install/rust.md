# Install for Rust

For running untrusted code from Rust, use [`monty-pool`](https://crates.io/crates/monty-pool) rather than the in-process
interpreter:

```bash
cargo add monty-pool monty-types tokio --features tokio/macros,tokio/rt-multi-thread
```

`monty-pool` runs code only in `monty` worker subprocesses, which is what gives you crash isolation and, once you ask
for it, a hard per-turn timeout.
`PoolConfig::subprocess` defaults to no timeouts, so set `request_timeout` before running untrusted code.
It is the same engine the Python and JavaScript packages are built on.
Workers are `monty` CLI binaries: build one with `cargo build -p monty-runtime`, or install it from PyPI as
`pydantic-monty-runtime`.

The in-process interpreter is the [`monty`](https://crates.io/crates/monty) crate:

```bash
cargo add monty monty-types
```

See the [Rust quickstart](../quickstart/rust.md) for both, and for when the in-process API is the right choice.

## The crates

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

Host-side crates depend on `monty-types`, never on `monty`, so the interpreter is not linked into your parent process at
all.

The [Rust API](../api/rust/monty.md) pages document `monty`, `monty-pool`, `monty-types`, `monty-fs`, `monty-proto` and
`monty-type-checking`.
