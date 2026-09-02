# pydantic-monty-runtime

The `monty` command-line binary for the
[Monty](https://github.com/pydantic/monty) sandboxed Python interpreter.

Installing the wheel puts the compiled binary in the environment's scripts directory.

You normally don't install this directly — the
[`pydantic-monty`](https://pypi.org/project/pydantic-monty/) metapackage pulls
it in alongside the Python bindings, which need it to spawn worker
subprocesses. Install it on its own to get just the CLI, or to supply the binary
to an existing [`pydantic-monty-client`](https://pypi.org/project/pydantic-monty-client/)
install:

```bash
uv add pydantic-monty-runtime
# or
pip install pydantic-monty-runtime
```

Or to install the `monty` binary as a tool

```bash
uv tool install pydantic-monty-runtime
monty --help
```

## Usage

- `monty` — start an interactive REPL
- `monty file.py` — run a Python file
- `monty -c "<code>"` — run a program passed as a string (like `python -c`)
- `-i` / `--interactive` — run the file or `-c` program in a REPL session
  (like `python -i`)
- `-t` / `--type-check` — type check (powered by [ty](https://docs.astral.sh/ty/))
  before executing
- `--type-check-format` — diagnostic format: `full` (default), `concise`,
  `json`, `github` and the other ty formats (requires `--type-check`)
- `-m` / `--mount /host/path::/virtual/path[::mode[::write_limit_bytes]]` —
  mount a host directory into the sandbox (`ro`, `rw`, or `overlay`)
- `--max-memory 10MB`, `--max-duration 0.5`, `--max-recursion-depth`,
  `--max-suspensions-per-run`, `--max-total-suspensions`, `--gc-interval` —
  sandbox resource limits

## Worker mode

`monty subprocess` runs the binary as a wire-protocol child: framed protobuf
requests on stdin, framed events on stdout. This is how `pydantic-monty` runs
sandboxed code with crash isolation, and is meant to be driven by a parent
process, not by hand.

The wheels are built from the
[`monty-runtime`](https://crates.io/crates/monty-runtime) crate; see its
readme for cargo features, telemetry, and the rest of the Rust-side detail.
