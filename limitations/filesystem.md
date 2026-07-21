# Filesystem and sandbox boundary

The sandbox has no default filesystem access. The host explicitly mounts
real directories at virtual paths through Monty's `MountTable`; everything
outside a mount is invisible. Without any mounts, [`open()`](open.md) and
all of [pathlib](pathlib.md)'s I/O methods raise `FileNotFoundError` for
every path.

## Virtual paths are always POSIX

Inside the sandbox, paths use forward slashes regardless of host OS.
`Path("C:/Users/foo")` is not a Windows path — it is the literal POSIX
path `C:/Users/foo`. Path repr is always `PosixPath(...)`.

Bytes paths are accepted but decoded as strict UTF-8 (no `surrogateescape`
/ PEP 383 round-tripping). See [open.md](open.md) for the full rationale.

## Mount modes

Each mount is configured by the host as one of:

- **`ReadOnly`** — reads allowed; any write (open with `w`/`a`, `mkdir`,
  `unlink`, `write_text`, ...) raises `PermissionError`.
- **`ReadWrite`** — full read/write into the underlying host directory.
- **`OverlayMemory`** — copy-on-write: reads fall through to the host
  directory, writes are captured in memory and never touch the host. Via the
  pool, the changes are discarded when the feed ends — each feed starts with
  a fresh overlay.

## Only regular files can be read, written, or opened

Reading, writing, appending to, or `open()`ing a path that resolves to an
existing **non-regular file** (FIFO/named pipe, socket, device node) raises
`PermissionError`. CPython would block until a peer appears; mount I/O runs on
the host thread driving the sandbox, so it must never block on
sandbox-reachable input. Directories raise `IsADirectoryError` as in CPython.
Existence checks (`exists`, `is_file`, `is_dir`, `is_symlink`) and `stat()`
still work on special files.

## Mount memory limits

Each mount has a configurable `memory_usage_limit`, defaulting to 100 MB
(100,000,000 bytes). Retained in-memory overlay entries and transient
filesystem results share the budget. Host files are read incrementally up to
the remaining budget without trusting file-size metadata; an operation that
would exceed it raises
`MemoryError: mount memory usage limit of 100 MB exceeded`. CPython has no
equivalent default limit.

Consequences of the shared budget that have no CPython analogue:

- Reading a file back needs transient budget for the result alongside the
  retained copy, so an overlay file larger than roughly half the budget can be
  written but not read back.
- Overlay deletions (`unlink`, `rmdir`, and the tombstones a `rename` leaves
  behind) record in-memory entries, so they too can raise `MemoryError` when
  the budget is exhausted.
- The `monty` CLI's `-m` mounts always use the default limit; there is no CLI
  flag to change it.

## Write limits

Hosts can configure a cumulative `write_bytes_limit` per mount. In
`OverlayMemory`, appending to an existing real file can materialize that
file into the in-memory overlay, so the existing file bytes count against
the limit along with the newly appended bytes.

## Sandbox guarantees

The host enforces these invariants on every path operation:

- Canonicalization happens *after* mapping virtual → host paths.
- The canonical path must remain inside the mount; `..` traversal cannot
  escape (raises `PermissionError`).
- Symlinks pointing outside the mount are rejected on resolution.
- Null bytes in any path component are rejected (`ValueError`).
- Resolved paths returned to the sandbox (e.g. via `Path.resolve()`) are
  virtual paths, never host paths.

`/tmp`, `/etc`, `/proc`, `/dev`, `~`, and the host current working
directory are **not** available unless the host explicitly mounts them.

## No live host descriptors

`open()` and pathlib I/O do not keep an OS handle alive between calls —
each `read`/`write` is a separate one-shot host operation. This is what
makes subprocess [dump/load](pool-architecture.md#execution-model) safe,
and it means external processes can observe partial state between writes.
See the design note in [open.md](open.md).
