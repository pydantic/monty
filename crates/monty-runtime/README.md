# pydantic-monty-runtime

The `monty` command-line binary for the
[Monty](https://github.com/pydantic/monty) sandboxed Python interpreter.

```console
$ monty -c "print('hello world')"
hello world
```

## Usage

- `monty` — start an interactive REPL
- `monty file.py` — run a Python file
- `monty -c "<code>"` — run a program passed as a string (like `python -c`)
- `-i` / `--interactive` — run the file or `-c` program in a REPL session
  (like `python -i`)
- `-t` / `--type-check` — type check (powered by [ty](https://docs.astral.sh/ty/))
  before executing
- `-m` / `--mount /host/path::/virtual/path[::mode[::write_limit_bytes]]` —
  mount a host directory into the sandbox (`ro`, `rw`, or `overlay`)
- `--max-memory 10MB`, `--max-duration 0.5`, `--max-recursion-depth`,
  `--gc-interval` — sandbox resource limits

## Features

- `run-cli` (default) — the standalone `monty <file>` / `-c` / REPL path and
  its terminal stack (`rustyline`, `anstream`, `anstyle`). Without it the binary
  serves `monty subprocess` alone, which is all a pool ever spawns; the flags
  still parse, but anything other than `subprocess` is refused.
- `telemetry` — see below. Implies `run-cli`, since it only instruments that
  path.

## Observability

Behind the `telemetry` feature, off by default because its exporter links MBs
of TLS that `monty subprocess` — which never exports — would otherwise carry.
Without it, `LOGFIRE_TOKEN` is ignored.

Built with `--features telemetry`, standalone CLI runs configure the Rust
Logfire SDK when `LOGFIRE_TOKEN` is set. The SDK also honors standard
OpenTelemetry exporter and resource environment variables. The CLI owns the SDK
lifecycle and flushes it before exiting.

`monty subprocess` deliberately ignores telemetry environment configuration:
worker processes are instrumented by their parent pool, avoiding duplicate
exporters and keeping credentials out of sandbox workers.

## Worker mode

`monty subprocess` runs the binary as a wire-protocol child: framed protobuf
requests on stdin, framed events on stdout (see the
[`monty-proto`](https://crates.io/crates/monty-proto) crate). This is how the
[`monty-pool`](https://crates.io/crates/monty-pool) crate — and through it the
[`pydantic-monty`](https://pypi.org/project/pydantic-monty/) and
[`@pydantic/monty`](https://www.npmjs.com/package/@pydantic/monty) packages —
runs Monty with crash isolation. It is meant to be driven by a parent
process, not by hand.

The binary runs under the
[`monty-alloc`](https://crates.io/crates/monty-alloc) global allocator, which
provides the session's soft-limit usage and enforces a higher hard ceiling.
Crossing the soft limit raises `MemoryError` at an interpreter checkpoint;
crossing the hard limit exits with a dedicated status the parent can classify.

## PyPI packaging (`pydantic-monty-runtime`)

The binary is also packaged for PyPI as
[`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/), the same
way `uv` and `ruff` package theirs: installing the wheel places the compiled
binary in the environment's scripts directory. It exists so that
`pydantic-monty` can find a `monty` binary without any manual setup, and is
installed automatically as a dependency of that package — you normally don't
install it directly.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode. **this crate**
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
