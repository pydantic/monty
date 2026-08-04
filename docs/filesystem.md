# Filesystem Access

A Monty sandbox has no filesystem. With nothing configured, `open()` and every
`pathlib.Path` method that reaches the filesystem raise `PermissionError` for every path,
because nothing is exposed for them to reach — including the existence checks CPython
answers without raising. `/tmp`, `/etc`, `/proc`, `/dev`, `~` and the host working
directory are not reachable.

You give the sandbox a filesystem in one of two ways: **mounts**, which map real host
directories to virtual paths, or an **`os` callback**, which answers filesystem operations
in host code.

## Mounts

A `MountDir` maps a host directory to a virtual path inside the sandbox. Mounts are
per-feed, and all arguments are keyword-only:

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

`'overlay'` is the default because it is the safe one: sandboxed code can write, read back
what it wrote, and never touch your disk. Each feed starts with a fresh overlay.

### Options

| Argument | Default | Meaning |
| --- | --- | --- |
| `host_path` | required | Real host directory. Canonicalized at construction; raises if it does not exist or is not a directory |
| `virtual_path` | required | Absolute POSIX-style path prefix inside the sandbox, whatever the host OS |
| `mode` | `'overlay'` | One of the three above |
| `write_bytes_limit` | `None` | Cap on cumulative bytes written through the mount in one feed; exceeding it raises `OSError` in the sandbox |
| `memory_usage_limit` | `100_000_000` | Per-mount budget in bytes shared by retained overlay data and transient results; exceeding it raises `MemoryError` in the sandbox |

Validation happens at construction, not at feed time — a bad `virtual_path` raises
immediately.

The names are keyword-only on purpose: mount tools disagree on whether host or virtual
comes first (`docker -v` versus nginx's `alias`), so requiring names removes the ambiguity.

### What a mount guarantees

Every path operation goes through a single security boundary that canonicalizes after
mapping virtual to host, checks the result is still inside the mount, rejects symlinks
pointing outside it, and rejects null bytes. Resolved paths handed back to the sandbox are
virtual paths — a host path never leaks in. See the
[security model](security.md#mounts-and-the-os-callback).

### Things that differ from CPython

- **Virtual paths are always POSIX**, on every host OS. `Path('C:/Users/foo')` is a
  literal POSIX path, and `repr` is always `PosixPath(...)`.
- **No live file descriptors.** `open()` keeps no OS handle between calls; each read or
  write is a separate one-shot host operation. This is what makes mid-execution
  [snapshots](snapshots.md) safe. It also means `for line in f` is not supported.
- **Only regular files** can be read, written or opened. FIFOs, sockets and device nodes
  raise `PermissionError`, because mount I/O must never block on sandbox-reachable input.
  Existence checks and `stat()` still work on them.

The full list is in
[`limitations/filesystem.md`](https://github.com/pydantic/monty/blob/main/limitations/filesystem.md)
and [`limitations/open.md`](https://github.com/pydantic/monty/blob/main/limitations/open.md).

## The `os` callback

Operations no mount covers fall through to the `os=` handler. It is called as
`(function_name, args, kwargs)` and its return value is handed back to the sandbox:

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

Returning the `NOT_HANDLED` sentinel declines the call, and the sandbox raises whatever it
would have raised with no handler at all.

The operations that can arrive are a fixed set: `Path.exists`, `Path.is_file`,
`Path.is_dir`, `Path.is_symlink`, `open`, `Path.read_text`, `Path.read_bytes`,
`Path.write_text`, `Path.write_bytes`, `Path.append_text`, `Path.append_bytes`,
`Path.mkdir`, `Path.unlink`, `Path.rmdir`, `Path.iterdir`, `Path.stat`, `Path.rename`,
`Path.resolve`, `Path.absolute`, `os.getenv`, `os.environ`, `date.today` and
`datetime.now`.

`os` callbacks run in your process with your process's authority. Everything in
[designing a safe tool surface](host-functions.md#designing-a-safe-tool-surface) applies.

### Resolution order

Within a `feed_run`, an OS call is offered to the feed's mounts first, then to the `os=`
handler, then falls back to the sandbox's own no-handler error.

`feed_start` is different: it surfaces every OS call as a snapshot instead of answering
it. `snapshot.resume_auto()` applies the same mounts-then-`os` order, and
`snapshot.resume_not_handled()` applies the no-handler default explicitly.

## A virtual filesystem in Python

`pydantic_monty` ships a ready-made `AbstractOS` implementation for when you want a
filesystem with no host directory behind it at all:

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

`OSAccess` backed by `MemoryFile` objects is fully sandboxed: content lives in host
memory, path traversal cannot escape to real files, and `os.getenv` sees only the
`environ` mapping you passed.

`CallbackFile` is the escape hatch within the escape hatch — its read and write callbacks
run in the host and can reach real resources. That is the point of it, but it means an
`OSAccess` containing a `CallbackFile` is exactly as sandboxed as the callback you wrote.

For anything more specific, subclass `AbstractOS` and implement the methods you want;
anything you leave raising `NotImplementedError` is reported to Monty as `NOT_HANDLED`.

## In other languages

JavaScript exposes `MountDir` from the `@pydantic/monty/node` subpath with the same
options in camelCase (`hostPath`, `virtualPath`, `mode`, `writeBytesLimit`,
`memoryUsageLimit`), plus the same `os` callback and `NOT_HANDLED` sentinel. The
WebAssembly build rejects mounts outright, because a browser has no host filesystem.

Rust hosts hold a `MountTable` from [`monty-fs`](https://crates.io/crates/monty-fs) and
service suspensions with `MountTable::handle_os_call`; `monty-pool` does this for you when
you pass `MountSpec`s to `Checkout::feed`.
