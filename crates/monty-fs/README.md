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
