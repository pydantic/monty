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

| Flag                    | Meaning                                                                                                                                                  |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-i`, `--interactive`   | Run the file or `-c` program, then drop into a REPL, like `python -i`                                                                                    |
| `-t`, `--type-check`    | [Type check](type-checking.md) before executing                                                                                                          |
| `--type-check-format`   | Diagnostic format: `full` (default), `concise`, `json`, `github` and the other ty formats                                                                |
| `-m`, `--mount`         | Mount a host directory into the sandbox (see below)                                                                                                      |
| `--cwd`                 | The sandbox's virtual working directory (default: the first mount, else `/`)                                                                             |
| `--max-feed-duration`   | Maximum execution time per feed, in seconds, e.g. `0.5`; only the REPL feeds more than once                                                              |
| `--max-turn-duration`   | Maximum execution time between host round trips, in seconds                                                                                              |
| `--max-memory`          | Maximum heap memory, e.g. `1024`, `512KB`, `10MB`, `1GB`                                                                                                 |
| `--max-recursion-depth` | Maximum call-stack depth (default 1000)                                                                                                                  |
| `--gc-interval`         | Run garbage collection every N allocations                                                                                                               |
| `--max-suspensions`     | Maximum suspensions serviced, per run or across a whole interactive session (default 1000); [what counts](resource-limits.md#suspensions)                |
| `--max-sleep`           | Longest wait a `time.sleep()` or `asyncio.sleep()` performs inside the sandbox, in seconds; longer sleeps are cut short (default 10, `inf` for no limit) |
| `--version`             | Print the version                                                                                                                                        |

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

The sandbox's [working directory](filesystem.md#working-directory) defaults to the first mount's virtual path, so
`monty -m ./data::/data script.py` runs with `os.getcwd() == '/data'` and `__file__ == '/data/script.py'`; `--cwd`
picks another absolute virtual path.
Only the file argument's name is used, so `monty ./scripts/run.py` and `monty /abs/run.py` both give
`__file__ == '/data/run.py'`; the host directory never reaches the sandbox.

## The clock, sleeping and entropy

`date.today()` and `datetime.now()` read the machine's clock and local timezone; `time.time()` reads the machine's clock as Unix epoch seconds.
`time.sleep()` and `asyncio.sleep()` wait inside the sandbox, each call cut short at `--max-sleep` (10 seconds unless
changed, `inf` for no cap), in every run — script, `-c` and REPL, with or without a mount.
An unseeded `random` draw seeds the generator from the machine's entropy, as CPython does.
These are the defaults every embedding gets; the CLI has no flag to freeze the clock, skip the sleeps or seed
`random` — `MontyRun::with_auto_os_calls` is how a Rust embedder chooses otherwise, and `checkout()` how the pools do
(see [the clock](security.md#the-clock)).

```console
$ monty -c "from datetime import datetime; print(datetime.now())"
2026-09-03 21:02:32.871568
```

Nothing answers `os.urandom()` in the CLI, so it fails.
Without `--mount` the script runs in-process and the call raises
`NotImplementedError: OS function 'os.urandom' not implemented with standard execution`; with a mount it goes
through the host loop and raises `RuntimeError: 'os.urandom' is not supported in this environment`.
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
