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

| Invocation          | What it does                                       |
| ------------------- | -------------------------------------------------- |
| `monty`             | Start an interactive REPL                          |
| `monty file.py`     | Run a Python file                                  |
| `monty -c "<code>"` | Run a program passed as a string, like `python -c` |

## Flags

| Flag                    | Meaning                                                                                                                                    |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `-i`, `--interactive`   | Run the file or `-c` program, then drop into a REPL, like `python -i`                                                                      |
| `-t`, `--type-check`    | [Type check](type-checking.md) before executing                                                                                            |
| `--type-check-format`   | Diagnostic format: `full` (default), `concise`, `json`, `github` and the other ty formats                                                  |
| `-m`, `--mount`         | Mount a host directory into the sandbox (see below)                                                                                        |
| `--cwd`                 | The sandbox's virtual working directory (default: the first mount, else `/`)                                                               |
| `--max-feed-duration`   | Maximum execution time per feed, in seconds, e.g. `0.5`; only the REPL feeds more than once                                                |
| `--max-turn-duration`   | Maximum execution time between host round trips, in seconds                                                                                |
| `--max-memory`          | Maximum heap memory, e.g. `1024`, `512KB`, `10MB`, `1GB`                                                                                   |
| `--max-recursion-depth` | Maximum call-stack depth (default 1000)                                                                                                    |
| `--gc-interval`         | Run garbage collection every N allocations                                                                                                 |
| `--max-suspensions`     | Maximum suspensions serviced, per run or across a whole interactive session (default 1000); [what counts](resource-limits.md#suspensions)  |
| `--max-sleep`           | Longest wait a `time.sleep()` or `asyncio.sleep()` performs, in seconds; longer sleeps are cut short (default 10, `inf` for no limit)      |
| `--max-total-sleep`     | Maximum cumulative host wait for those sleeps, in seconds; a sleep that would go past it is refused (off unless given, `inf` for no limit) |
| `--version`             | Print the version                                                                                                                          |

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

Repeat `-m` for several mounts; they must have distinct virtual paths and disjoint host directories, or `monty` exits
before running anything.
Without a mount, the sandbox has no filesystem at all.
See [filesystem access](filesystem.md).

CLI mounts always use the default per-mount memory limit of 100 MB; there is no flag to change it.

The sandbox's [working directory](filesystem.md#working-directory) defaults to the first mount's virtual path, so
`monty -m ./data::/data script.py` runs with `os.getcwd() == '/data'` and `__file__ == '/data/script.py'`; `--cwd`
picks another absolute virtual path.
Only the file argument's name is used, so `monty ./scripts/run.py` and `monty /abs/run.py` both give
`__file__ == '/data/run.py'`; the host directory never reaches the sandbox.

## The clock, sleeping and entropy

`date.today()` and `datetime.now()` read the machine's clock and local timezone; `time.time()` reads the machine's clock as Unix epoch seconds.
`--max-sleep` caps each `time.sleep()` and `asyncio.sleep()` at ten seconds by default (`inf` removes the cap).
`--max-total-sleep` optionally bounds their cumulative duration.
Both limits apply to scripts, `-c` and the REPL, with or without mounts.
Unseeded random generators use system entropy.
To freeze the clock or configure random seeds, use the [session options](security.md#the-clock) in Rust or the pools;
the CLI has no flags for them.

```console
$ monty -c "from datetime import datetime; print(datetime.now())"
2026-09-03 21:02:32.871568
```

The CLI has no `os.urandom()` handler.
It raises `NotImplementedError: OS function 'os.urandom' not implemented with standard execution`, or
`RuntimeError: 'os.urandom' is not supported in this environment` when mounts or `--max-total-sleep` require a host loop.
See [`limitations/datetime.md`](https://github.com/pydantic/monty/blob/main/limitations/datetime.md),
[`limitations/time.md`](https://github.com/pydantic/monty/blob/main/limitations/time.md) and
[`limitations/random.md`](https://github.com/pydantic/monty/blob/main/limitations/random.md).

## Worker mode

`monty subprocess` runs the binary as a wire-protocol child: framed protobuf requests on stdin, framed events on stdout.
This is how [`monty-pool`](https://crates.io/crates/monty-pool) — and through it the Python and JavaScript packages —
runs Monty with crash isolation.

It is meant to be driven by a parent process, not by hand.
Normal execution flags are rejected alongside it, because a subprocess worker reads all its configuration from the
protocol.
