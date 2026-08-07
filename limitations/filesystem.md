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

The check runs before the open rather than on the handle, so a *host* process
with write access to the mount could swap in a FIFO in between and block the
servicing thread. Sandboxed code cannot — no mount mode creates special files
or symlinks — so do not mount directories untrusted local processes can write
to.

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

A mount opens a descriptor on its host directory once, at mount time, and
every operation runs relative to it, so nothing resolves from the filesystem
root.

- The mount is pinned to the **directory**, not its path: renaming the host
  directory does not detach the mount, and replacing it with a symlink does
  not redirect reads.
- `..` cannot escape, and neither can a symlink or an intermediate directory
  swapped for one mid-operation; such paths raise `PermissionError`.
- Path segments a host parser reads as absolute are rejected
  (`PermissionError`) on every OS and in every mount mode: any segment
  containing a backslash or starting with `X:`. So names CPython allows on
  Unix — `a\b.txt`, `a:b.txt` — are refused there too, since Windows would
  read them as drive-relative. Colons elsewhere are fine (`note:2026.txt`).
- The mount root itself cannot be renamed or removed: `rename` and `rmdir` on
  the mount's own path raise `PermissionError` in every mode, where CPython on
  an ordinary empty directory would succeed (the root has no name inside the
  mount, so there is nothing to detach it from).
- Null bytes in any path component are rejected (`ValueError`).
- Resolved paths returned to the sandbox (e.g. via `Path.resolve()`) are
  virtual paths, never host paths.

`/tmp`, `/etc`, `/proc`, `/dev`, `~`, and the host current working
directory are **not** available unless the host explicitly mounts them.

### `..` is resolved in the virtual namespace, not through symlinks

`..` is collapsed textually before anything touches the filesystem, so it
always names the lexical parent. POSIX instead resolves it *after* following
the preceding component, which differs whenever a symlink is involved: with
`ld -> sub/deep`, CPython reads `ld/../sibling.txt` as `sub/sibling.txt`,
while Monty reads it as `sibling.txt` — each finds a file the other does not.
The textual rule is what makes `..` unable to escape at all, so it is
deliberate; only paths mixing `..` with symlinked directories are affected.

### A mount needs read permission on its host directory

Mounting opens a descriptor, which requires read access. A search-only
directory (mode `0o111`) is therefore not mountable — `MountDir` raises at
construction — even though a host process can traverse it and read known
paths inside. Grant `r-x` on the directory being mounted.

### Symlink targets must be relative

**A symlink inside a mount is followed only if its target is relative; an
absolute target raises `PermissionError` even when it points back into the
same mount.** CPython follows both. A descriptor has no path of its own, so a
leading `/` cannot be interpreted and "absolute but inside" is
indistinguishable from "absolute and outside".

This is the one thing to check before mounting an existing directory.
Sandboxed code cannot create symlinks, so only links **already present** are
affected — `node_modules` installs, Homebrew/Nix store trees, build output
with linked artifacts. Only the links fail; the files themselves are readable,
the error is loud, and nothing returns wrong data. `exists()`, `is_file()` and
`is_dir()` answer `False` rather than raising. To keep such links working,
rewrite them as relative before mounting.

### Windows locks a mounted directory against the host

Windows refuses to rename or delete a directory while a handle to it is open,
so the host cannot move or delete a directory for as long as it is mounted —
the attempt fails with `ERROR_SHARING_VIOLATION`. Unix is unaffected. The
window is the mount's lifetime, which for `pydantic_monty` and
`@pydantic/monty` is one feed (see
[pool-architecture.md](pool-architecture.md)).

### `OverlayMemory` writes refuse symlinks

CPython (and a direct mount) writes *through* a symlink to its target. An
overlay write never touches the host, so it cannot do that — recording the
entry under the link's spelling instead would silently alias the two paths.
Any write (`write_text`, `open('w'/'a')`, `mkdir`, `unlink`, `rmdir`, rename
sources and destinations, …) whose path contains a symlink **anywhere** — as
the final component or an intermediate directory, whether it resolves
in-mount, dangles, is absolute, or escapes — raises `PermissionError`. The
refusal is uniform on purpose: the link's target is never even resolved, so
the error reveals nothing about it. Deletes are included because tombstoning
a link's spelling would report it gone while its target stayed readable under
the real name — the same aliasing, reached by deleting rather than writing.

Renaming a **directory that contains a symlink** is refused for the same
reason: the link cannot move with it (there is nothing to record) and cannot
be left behind (it would stay readable inside a directory the overlay then
reports as deleted). CPython moves the directory and the link together.

Reads still follow relative in-mount links as before, and direct mounts
still allow writes through them; only escaping/absolute links are refused
there. `mkdir(parents=True)` through an escaping symlink raises
`PermissionError` in every mode.

### `OverlayMemory` renames of real files

A rename records a mount-relative reference to the existing file rather than
copying its bytes, so it cannot come to name anything outside the mount. If
the host replaces the underlying file before the read, the read sees whatever
now occupies that path inside the mount; CPython, holding no such reference,
would not find the renamed-away original.

Renaming a directory that really contains an entry with a non-UTF-8 name
raises `OSError` (`directory contains an entry with a non-UTF-8 name`):
Monty's sandbox namespace is strictly UTF-8, so the entry cannot follow the
move. CPython renames the directory inode and never looks at the names
inside.

### Boolean predicates never raise

`exists()`, `is_file()` and `is_dir()` answer `False` for any path leaving the
mount, so a blocked path is indistinguishable from a missing one — raising
would confirm something is there to be blocked. CPython follows the link and
answers `True`.

`is_symlink()` is the exception: it does not follow the final component, so a
symlink that is itself inside the mount answers `True` even when its target
escapes. That matches CPython, and reveals nothing about the target.

## No live host descriptors

`open()` and pathlib I/O do not keep an OS handle alive between calls —
each `read`/`write` is a separate one-shot host operation. This is what
makes subprocess [dump/load](pool-architecture.md#execution-model) safe,
and it means external processes can observe partial state between writes.
See the design note in [open.md](open.md).
