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
- `--cwd /virtual/path` — the sandbox's working directory (default: the first
  mount's virtual path, else `/`); relative paths resolve against it
- `--max-memory 10MB`, `--max-feed-duration 0.5`,
  `--max-turn-duration`, `--max-recursion-depth`, `--gc-interval`,
  `--max-suspensions` — sandbox resource limits
- `--max-sleep 10` — longest wait a `time.sleep()` / `asyncio.sleep()` performs,
  in seconds; longer sleeps are cut short (`inf` for no limit)
- `--max-total-sleep 30` — maximum cumulative time those sleeps may ask for, in
  seconds; a sleep that would go over is refused (off unless given)

`date.today()` and `datetime.now()` read this machine's clock and local
timezone, and `time.time()` its clock as Unix epoch seconds; `time.sleep()` and
`asyncio.sleep()` are waited out by the CLI, capped by `--max-sleep`; an unseeded
`random` draw seeds from the machine's entropy — the defaults of every
embedding. `MontyRun::with_auto_os_calls` is how a Rust embedder chooses
otherwise; the CLI has no flag for it. Nothing answers `os.urandom()`, so it
raises `NotImplementedError` (or `RuntimeError` under `--mount`).

## Worker mode

`monty subprocess` runs the binary as a wire-protocol child: framed protobuf
requests on stdin, framed events on stdout. This is how `pydantic-monty` runs
sandboxed code with crash isolation, and is meant to be driven by a parent
process, not by hand.

The wheels are built from the
[`monty-runtime`](https://crates.io/crates/monty-runtime) crate; see its
readme for cargo features, telemetry, and the rest of the Rust-side detail.
