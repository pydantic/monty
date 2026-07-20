# monty-fs

Host-side filesystem mounts for [Monty](https://github.com/pydantic/monty), the
sandboxed Python interpreter.

Provides `MountTable`, which maps virtual POSIX paths inside the sandbox
(e.g. `/mnt/data`) to real host directories with configurable access modes
(read-write, read-only, or in-memory overlay).

The `monty` interpreter crate never performs filesystem I/O itself — sandboxed
code suspends with an `OsFunctionCall` describing the requested operation, and
a host holding a `MountTable` services it via `MountTable::handle_os_call`.
Keeping that I/O in a separate crate means the interpreter (and worker
artifacts built from it, such as the wasm worker) contain no host-filesystem
code at all.

All path resolution goes through a single security boundary
(`path_security::resolve_path`) enforcing canonicalization, mount-boundary
checks, and symlink escape detection: the sandbox can never read, write, or
learn anything about files outside the mounted directories.

Each mount has a configurable aggregate memory budget that defaults to 100 MB.
Retained in-memory overlay data and transient filesystem results share that
budget; oversized operations return `MemoryError` before an unbounded read.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories. **this crate**
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
