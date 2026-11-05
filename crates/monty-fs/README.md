# monty-fs

Host-side filesystem mounts for [Monty](https://github.com/pydantic/monty), the
Python sandbox.

Provides `MountTable`, which maps virtual POSIX paths inside the sandbox
(e.g. `/mnt/data`) to real host directories with configurable access modes
(read-write, read-only, or in-memory overlay).

The `monty` interpreter crate never performs filesystem I/O itself — sandboxed
code suspends with an `OsFunctionCall` describing the requested operation, and
a host holding a `MountTable` services it via `MountTable::handle_os_call`.
Keeping that I/O in a separate crate means the interpreter (and worker
artifacts built from it, such as the wasm worker) contain no host-filesystem
code at all.

Confinement is structural rather than a check: each mount holds a
`cap_std::fs::Dir` opened at mount time, and every operation runs relative to
that descriptor, so `..`, symlinks, and directories swapped mid-operation
cannot reach outside it. `path_security.rs` is left with path policy alone —
null-byte rejection and length limits, with lexical normalization delegated to
`monty_types::normalize_virtual_path`.

Each mount has a configurable aggregate memory budget that defaults to `DEFAULT_MEMORY_USAGE_LIMIT`.
Retained in-memory overlay data and transient filesystem results share that
budget; oversized operations return `MemoryError` before an unbounded read.

## Positional writes

`WriteBytesAt` overwrites an existing regular file at an absolute byte offset, preserving surrounding bytes.
It never creates or truncates the file; an empty payload does not extend it.
Native mounts seek and write through one confined handle, which is closed before returning.
Overlays copy backing content on the first mutation and keep all changes in memory.
The write budget charges the payload and any zero-filled gap, or the resulting materialized content on first copy.
Overlay memory limits apply separately.
Native write failures can leave partially modified content.

## Exclusive creation

`OsFunctionCall::CreateExclusive` reserves an empty file and returns a `MontyFileHandle` in one request.
An occupied name, including a directory or dangling symlink, returns `FileExistsError` without changing it.
Parent directories must already exist, and creation requires a writable mount even when the returned handle is read-only.
The handle accepts `r`, `rb`, `w`, `wb`, `a`, `ab`, and binary write/update (`w+b` / `wb+`).
Other update modes are rejected.

Native mounts use descriptor-relative exclusive creation, so checking and reserving the name are atomic.
The `permissions` field accepts read/write access bits (`0o666` mask), applied on Unix subject to umask.
Execute bits and bits outside the access mask are rejected with `ValueError` before creation.
On Windows, Monty ignores the requested permission bits and uses the filesystem's default access rules,
including inheritance from the parent directory.
Overlay mounts insert an empty overlay file and never create or replace a backing file.
Their exclusive check and insertion hold a mutable borrow of the overlay state; the host must not concurrently modify
backing entries during creation.
Overlay mounts ignore requested permission bits and report fixed stat modes.
Python `OSAccess` reports the requested bits through `stat()`, but does not enforce them for access.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories. **this crate**
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
