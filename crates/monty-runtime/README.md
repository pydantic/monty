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

## Worker mode

`monty subprocess` runs the binary as a wire-protocol child: framed protobuf
requests on stdin, framed events on stdout (see the
[`monty-proto`](https://crates.io/crates/monty-proto) crate). This is how the
[`monty-pool`](https://crates.io/crates/monty-pool) crate — and through it the
[`pydantic-monty`](https://pypi.org/project/pydantic-monty/) and
[`@pydantic/monty`](https://www.npmjs.com/package/@pydantic/monty) packages —
runs Monty with crash isolation. It is meant to be driven by a parent
process, not by hand.

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
