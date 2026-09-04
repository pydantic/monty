# Filesystem Access

A Monty sandbox has no filesystem.
With nothing configured, `open()` and every `pathlib.Path` method that reaches the filesystem raise `PermissionError`
for every path, because nothing is exposed for them to reach — including the existence checks CPython answers without
raising.
`/tmp`, `/etc`, `/proc`, `/dev`, `~` and the host working directory are not reachable.

You give the sandbox a filesystem in one of two ways: **mounts**, which map real host directories to virtual paths, or
an **`os` callback**, which answers filesystem operations in host code.

## Mounts

A `MountDir` maps a host directory to a virtual path inside the sandbox.
Mounts are per-feed, and all arguments are keyword-only:

```python
import tempfile
from pathlib import Path

from pydantic_monty import Monty, MountDir

with tempfile.TemporaryDirectory() as tmp:
    Path(tmp, 'greeting.txt').write_text('hello from the host')

    mount = MountDir(host_path=tmp, virtual_path='/data', mode='read-only')
    code = "from pathlib import Path\nPath('/data/greeting.txt').read_text()"

    with Monty() as pool:
        with pool.checkout() as session:
            print(session.feed_run(code, mount=mount))
            #> hello from the host
```

Pass a list to `mount=` for several at once.

### Modes

| Mode | Reads | Writes |
| --- | --- | --- |
| `'read-only'` | from the host directory | raise `PermissionError` |
| `'read-write'` | from the host directory | written through to the host |
| `'overlay'` (default) | fall through to the host | captured in memory, discarded when the feed ends |

`'overlay'` is the default: writes are kept in memory and discarded when the feed ends, and sandboxed code still reads
back its own writes.
Each feed starts with a fresh overlay.

!!! warning "`'read-write'` writes files from untrusted code to your real filesystem"
    Those files are untrusted input; do not execute them.
    Importing counts as executing, and the import can be indirect: with a directory on `sys.path` mounted, sandboxed
    code can write `json.py`, or any module not yet imported, and the host's next `import` runs it — including imports
    `pydantic_monty` makes itself.
    Use `'read-write'` only with a directory that contains no code or config and is not on `sys.path` or any other
    execution path.

### Options

| Argument | Default | Meaning |
| --- | --- | --- |
| `host_path` | required | Real host directory; canonicalized at construction, raises if missing or not a directory |
| `virtual_path` | required | Absolute POSIX-style path prefix inside the sandbox, whatever the host OS |
| `mode` | `'overlay'` | One of the three above |
| `write_bytes_limit` | `None` | Cap on bytes written through the mount per feed; exceeding it raises `OSError` |
| `memory_usage_limit` | `100_000_000` | Byte budget for overlay data and transient results; exceeding it raises `MemoryError` |

Validation happens at construction, not at feed time — a bad `virtual_path` raises immediately.

The names are keyword-only on purpose: mount tools disagree on whether host or virtual comes first (`docker -v` versus
nginx's `alias`), so requiring names removes the ambiguity.

### What a mount guarantees

Confinement is structural, not a check.
Each mount opens a directory descriptor (`cap_std::fs::Dir`) once, at mount time, and every operation runs relative to
that descriptor, so `..`, symlinks and directories swapped mid-operation cannot reach outside it.
`..` and `.` are collapsed in the virtual namespace before anything touches the filesystem, null bytes are rejected, and
paths handed back to the sandbox (from `Path.resolve()`, for example) are virtual paths — a host path never leaks in.
See the [security model](security.md#mounts-and-the-os-callback).

The cost of the structural boundary is symlink support: read-only and read-write mounts refuse a symlink with an
absolute target even when it points back inside the mount, and overlay mounts refuse symlinks entirely.
Relative symlinks that stay inside the mount are followed in the non-overlay modes.

### Things that differ from CPython

- **Virtual paths are always POSIX**, on every host OS.
  `Path('C:/Users/foo')` is a literal POSIX path, and `repr` is always `PosixPath(...)`.
- **No live file descriptors.** `open()` keeps no OS handle between calls; each read or write is a separate one-shot
  host operation.
  This is what makes mid-execution [snapshots](snapshots.md) safe.
  It also means `for line in f` is not supported.
- **Only regular files** can be read, written or opened.
  FIFOs, sockets and device nodes raise `PermissionError`, because mount I/O must never block on sandbox-reachable
  input.
  Existence checks and `stat()` still work on them.

The full list is in [`limitations/filesystem.md`](limitations/filesystem.md)
and [`limitations/open.md`](limitations/open.md).

## The `os` callback

Operations no mount covers fall through to the `os=` handler.
It is called as `(function_name, args, kwargs)` and its return value is handed back to the sandbox:

```python
from pydantic_monty import NOT_HANDLED, Monty


def handle_os(function_name, args, kwargs):
    if function_name == 'os.getenv' and args[0] == 'STAGE':
        return 'production'
    return NOT_HANDLED


with Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run("import os\nos.getenv('STAGE')", os=handle_os))
        #> production
```

Returning the `NOT_HANDLED` sentinel declines the call, and the sandbox raises whatever it would have raised with no
handler at all.

The operations that can arrive are a fixed set: `Path.exists`, `Path.is_file`, `Path.is_dir`, `Path.is_symlink`, `open`,
`Path.read_text`, `Path.read_bytes`, `Path.write_text`, `Path.write_bytes`, `Path.append_text`, `Path.append_bytes`,
`Path.mkdir`, `Path.unlink`, `Path.rmdir`, `Path.iterdir`, `Path.stat`, `Path.rename`, `Path.resolve`, `Path.absolute`,
`os.getenv`, `os.environ`, `date.today` and `datetime.now`.

`os` callbacks run in your process with your process's authority.
Everything in [designing a safe tool surface](host-functions.md#designing-a-safe-tool-surface) applies.

### Resolution order

Within a `feed_run`, an OS call is offered to the feed's mounts first, then to the `os=` handler, then falls back to the
sandbox's own no-handler error.

`feed_start` is different: it surfaces every OS call as a snapshot instead of answering it.
`snapshot.resume_auto()` applies the same mounts-then-`os` order, and `snapshot.resume_not_handled()` applies the
no-handler default explicitly.

## A virtual filesystem in Python

`pydantic_monty` ships a ready-made `AbstractOS` implementation for when you want a filesystem with no host directory
behind it at all:

```python
from pydantic_monty import MemoryFile, Monty, OSAccess

fs = OSAccess(
    [
        MemoryFile('/data/report.csv', content='name,total\nada,42\n'),
    ],
    environ={'STAGE': 'test'},
)

code = "from pathlib import Path\nPath('/data/report.csv').read_text().count(chr(10))"

with Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run(code, os=fs))
        #> 2
```

`OSAccess` backed by `MemoryFile` objects is fully sandboxed: content lives in host memory, path traversal cannot escape
to real files, and `os.getenv` sees only the `environ` mapping you passed.

`CallbackFile` read and write callbacks run in the host and can reach real resources.
That is the point of it, but it means an `OSAccess` containing a `CallbackFile` is exactly as sandboxed as the callback
you wrote.

For anything more specific, subclass `OSAccess` and override the methods you want to change, or implement every
abstract method of `AbstractOS` yourself; the optional hooks (`path_open`, the append methods, `date_today`,
`datetime_now`) report `NOT_HANDLED` to Monty if you make them raise `NotImplementedError`.

## In other languages

JavaScript exposes `MountDir` from the `@pydantic/monty/node` subpath with the same options in camelCase (`hostPath`,
`virtualPath`, `mode`, `writeBytesLimit`, `memoryUsageLimit`), plus the same `os` callback and `NOT_HANDLED` sentinel.
The WebAssembly build rejects mounts outright, because a browser has no host filesystem.

Rust hosts hold a `MountTable` from [`monty-fs`](https://crates.io/crates/monty-fs) and service suspensions with
`MountTable::handle_os_call`; `monty-pool` does this for you when you pass `MountSpec`s to `Checkout::feed`.
