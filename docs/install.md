# Installation

Monty ships as a Python package, an npm package, a set of Rust crates and a standalone binary.
All of them are built from the same Rust core and released in lockstep.

## Python

```bash
uv add pydantic-monty
```

Or with pip:

```bash
pip install pydantic-monty
```

Requires Python 3.10 or newer.

`pydantic-monty` depends on [`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/), which ships
the `monty` binary the worker subprocesses run — the same way `uv` and `ruff` package theirs.
Installing the wheel places the binary in the environment's scripts directory, so there is no extra setup step.
You do not normally install it yourself.

The import name is `pydantic_monty`:

```python
import pydantic_monty

with pydantic_monty.Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run('1 + 2'))
        #> 3
```

### Where the worker binary comes from

`Monty(binary_path=...)` overrides binary resolution.
When it is omitted, the binary is resolved from the `MONTY_BIN` environment variable, then the environment's scripts
directory (where `pydantic-monty-runtime` installs it), then `PATH`.

If you are running untrusted code, pin `binary_path` explicitly rather than relying on `PATH`.

## JavaScript / TypeScript

```bash
npm install @pydantic/monty
```

Under Node the package is a native (napi) binding over the same Rust worker pool the Python package uses.
The binding and the `monty` worker binary ship as platform-specific packages selected through `optionalDependencies`, so
a plain `npm install` gets you everything.

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()
console.log(await session.feedRun('1 + 2')) // 3
```

For browsers, or anywhere subprocesses are impossible, the same package exposes an in-process WebAssembly build under
the `@pydantic/monty/wasm` subpath.
A bundler resolving the `browser` condition on the main entry point gets that build automatically.
See the [JavaScript quickstart](quickstart/javascript.md#browsers-and-webassembly) for what differs there.

## Rust

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

See the [Rust quickstart](quickstart/rust.md) for both, and for when the in-process API is the right choice.

### The crates

| Crate | What it is |
| --- | --- |
| [`monty`](https://crates.io/crates/monty) | The core interpreter: Python parser, bytecode VM, sandbox |
| [`monty-types`](https://crates.io/crates/monty-types) | Shared boundary types: values, exceptions, OS calls, limits |
| [`monty-fs`](https://crates.io/crates/monty-fs) | Host-side filesystem mounts |
| [`monty-runtime`](https://crates.io/crates/monty-runtime) | The `monty` binary: REPL, file runner, subprocess worker |
| [`monty-pool`](https://crates.io/crates/monty-pool) | Elastic pool of crash-isolated worker subprocesses |
| [`monty-proto`](https://crates.io/crates/monty-proto) | The protobuf wire protocol between pool parents and workers |
| [`monty-type-checking`](https://crates.io/crates/monty-type-checking) | Type checking, powered by ty |
| [`monty-typeshed`](https://crates.io/crates/monty-typeshed) | Trimmed typeshed stubs for Monty's stdlib subset |

Host-side crates depend on `monty-types`, never on `monty`, so the interpreter is not linked into your parent process at
all.

The [Rust API](api/rust/monty.md) pages document `monty`, `monty-pool`, `monty-types`, `monty-fs`, `monty-proto` and
`monty-type-checking`.

## Command line

The `monty` binary is a REPL and a file runner.
It comes with `pydantic-monty` (via `pydantic-monty-runtime`), or from `cargo build -p monty-runtime`:

```console
$ monty -c "print('hello world')"
hello world
```

See [command line](cli.md) for the flags.
