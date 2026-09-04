# Command Line

The `monty` binary runs Python files and gives you an interactive REPL, with the same sandbox, resource limits and type
checking the libraries use.
It is the fastest way to see what the interpreter does with a piece of code.

It ships with `pydantic-monty` (through the [`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/)
dependency), or you can build it with `cargo build -p monty-runtime`.

```console
$ monty -c "print('hello world')"
hello world
```

## Usage

| Invocation | What it does |
| --- | --- |
| `monty` | Start an interactive REPL |
| `monty file.py` | Run a Python file |
| `monty -c "<code>"` | Run a program passed as a string, like `python -c` |

## Flags

| Flag | Meaning |
| --- | --- |
| `-i`, `--interactive` | Run the file or `-c` program, then drop into a REPL, like `python -i` |
| `-t`, `--type-check` | [Type check](type-checking.md) before executing |
| `--type-check-format` | Diagnostic format: `full` (default), `concise`, `json`, `github` and the other ty formats |
| `-m`, `--mount` | Mount a host directory into the sandbox (see below) |
| `--max-duration` | Maximum execution time in seconds, e.g. `0.5` |
| `--max-memory` | Maximum heap memory, e.g. `1024`, `512KB`, `10MB`, `1GB` |
| `--max-recursion-depth` | Maximum call-stack depth; defaults to 1000 when any limit is set |
| `--gc-interval` | Run garbage collection every N allocations |
| `--max-suspensions` | Maximum suspensions serviced, per run or across a whole interactive session (default 1000); [what counts](resource-limits.md#suspensions) |
| `--version` | Print the version |

See [resource limits](resource-limits.md) for what the limits actually bound.

## Mounts

```text
-m /host/path::/virtual/path[::mode[::write_limit_bytes]]
```

The separator is `::` rather than `:` so Windows drive letters stay unambiguous.

`mode` is `ro` (read-only, the default), `rw` (read-write) or `overlay` (in-memory overlay).
`write_limit_bytes` is optional and applies to the write modes.

```console
$ monty -m ./data::/data::ro -c "from pathlib import Path; print(Path('/data').iterdir())"
```

Without a mount, the sandbox has no filesystem at all.
See [filesystem access](filesystem.md).

CLI mounts always use the default per-mount memory limit of 100 MB; there is no flag to change it.

## The clock

`date.today()` and `datetime.now()` read the machine's clock and local timezone.

```console
$ monty -c "from datetime import datetime; print(datetime.now())"
2026-09-03 21:02:32.871568
```

An in-process Rust run reads the same clock; `MontyRun::with_host_clock` is how an embedder chooses otherwise, and the
CLI has no flag for it.
A sandbox driven through the pool is different: there both calls reach your `os=` handler (see
[the clock](security.md#the-clock)).
See [`limitations/datetime.md`](https://github.com/pydantic/monty/blob/main/limitations/datetime.md).

## Worker mode

`monty subprocess` runs the binary as a wire-protocol child: framed protobuf requests on stdin, framed events on stdout.
This is how [`monty-pool`](https://crates.io/crates/monty-pool) — and through it the Python and JavaScript packages —
runs Monty with crash isolation.

It is meant to be driven by a parent process, not by hand.
Normal execution flags are rejected alongside it, because a subprocess worker reads all its configuration from the
protocol.
