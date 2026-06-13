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
  directory, writes are captured in memory and never touch the host. The
  changes vanish when the VM is discarded.

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
