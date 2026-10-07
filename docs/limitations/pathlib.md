# `pathlib` module

Only one class is exported: `pathlib.Path`. It always represents a virtual
POSIX path inside the sandbox (`/mnt/data/foo.txt`), never a Windows path
even when the host is Windows. `PurePath`, `PurePosixPath`, `PureWindowsPath`,
`PosixPath`, `WindowsPath` are not separately exposed; the printed `repr`
of a `Path` is `PosixPath(...)` for compatibility.

Because the class object and its instances share one type, the class object
answers to the instance name: `pathlib.Path.__name__` and `repr(pathlib.Path)`
give `PosixPath` / `<class 'PosixPath'>`, and `pathlib.Path.nonexistent` raises
`type object 'PosixPath' has no attribute 'nonexistent'`, where CPython names
`Path` (the instance-level spellings, e.g. `Path('/a') / 1`, match CPython).

## Construction

`Path(*segments)` works. Each segment may be a `str` or another `Path`.
Bytes paths are rejected with `TypeError`.

`Path.cwd()`, on the class or any instance, returns the sandbox's virtual working directory without a host
round-trip; the host sets it per feed and relative paths are resolved against
it before any I/O method reaches the host (see [os.md](os.md)). `Path.home()`
is **not** implemented: the sandbox has no home directory.

## Pure (no I/O) methods and attributes

Implemented: `name`, `parent`, `stem`, `suffix`, `suffixes`, `parts`,
`is_absolute()`, `joinpath(*other)`, `with_name(name)`, `with_stem(stem)`,
`with_suffix(suffix)`, `as_posix()`, `__fspath__()`.

The `/` operator works in both directions (`Path("a") / "b"`,
`Path("a") / Path("b")`, `"a" / Path("b")`).

Not implemented: `anchor`, `drive`, `root`, `relative_to`, `is_reserved`,
`match`, `full_match`, `with_segments`.

## I/O methods (yield to host)

These yield an `OsCall` for the host to resolve:

- `exists()`, `is_file()`, `is_dir()`, `is_symlink()`
- `read_text()`, `read_bytes()`
- `write_text(data)`, `write_bytes(data)`, `append_text(data)`, `append_bytes(data)`
- `mkdir(mode=0o777, parents=False, exist_ok=False)`, `unlink()`, `rmdir()`
- `iterdir()`, `stat()`, `rename(target)`
- `glob(pattern, *, case_sensitive=None, recurse_symlinks=False)`, `rglob(...)`,
    `walk(top_down=True, on_error=None, follow_symlinks=False)` — see below
- `resolve()`, `absolute()`
- `open(...)` — see [open.md](open.md) for the supported file API and divergences

`Path.mkdir()` parses `mode`, `parents`, and `exist_ok`, but `mode` is
accepted only for signature compatibility: Monty does not model POSIX
permission bits. The `missing_ok` and `target_is_directory` keyword arguments
accepted by other CPython methods are not parsed; pass only the positional
arguments documented above.

`Path.mkdir()`'s too-many-positional error counts only the visible
parameters (`Path.mkdir() takes from 0 to 3 positional arguments but 4 were given`); CPython counts the bound `self` as
well (`takes from 1 to 4 … but 5 were given`).

`Path.glob()`, `Path.rglob()` and `Path.walk()` count only the visible parameters in arity errors the same way.

Not implemented: `touch`, `chmod`, `lchmod`, `owner`,
`group`, `symlink_to`, `hardlink_to`, `link_to`, `readlink`, `lstat`,
`samefile`, `replace`, `expanduser`.

## `glob()`, `rglob()` and `walk()`

Each reads the whole subtree it needs from the host in one call when it is called, then globs or walks that snapshot
inside the sandbox (`os.walk` and `os.scandir` work the same way, see [os.md](os.md)).
Pattern semantics follow CPython 3.14's: hidden files match `*`, `**` descends only real directories unless
`recurse_symlinks=True`, other wildcards descend symlinked ones, a trailing `/` selects directories, and an explicit
`case_sensitive` matches literal parts by listing.

- **Not lazy.** `glob()` and `rglob()` return a `list_iterator` (CPython: `map`) and `walk()` an iterator typed
    `generator`; both are built from the snapshot, so files created or removed while iterating are not seen.
    Iterating a huge tree still reads all of it up front, charged to the mount's memory limit
    (see [filesystem.md](filesystem.md#directory-scans)).
- **Pruning `dirnames` in `walk()` works** (`dirnames.remove(...)`, `dirnames.clear()`), but only saves sandbox
    work: the host has already read the pruned directories.
    Monty has no list slice assignment, so the common `dirnames[:] = [...]` raises `TypeError`.
- **Order is sorted by name within each directory.** CPython yields directory entries in filesystem order.
- **`..` after a wildcard is collapsed lexically** (`'*/../a.txt'`), and one that climbs above the directory being
    globbed matches nothing; CPython resolves it through the filesystem. Leading `..` parts work as in CPython.
- **Duplicates** that CPython yields for overlapping recursive patterns (`'**/*/**'`) may not be repeated.
- **Unreadable subdirectories read as empty**, so `walk(on_error=...)` is only called for the top directory, or for
    a name added to `dirnames` that is not a directory.
- **Errors name the absolute path** for the top directory (`[Errno 2] No such file or directory: '/data/missing'`), as
    every host error does (see [os.md](os.md)); errors for subdirectories use the spelling `walk()` yields.
- **A `bytes` pattern** raises `TypeError: a bytes-like object is required, not 'str'`, as CPython does by accident;
    any other non-path pattern raises CPython's `_path_splitroot_ex` `TypeError`.
- **Without a mount or an `os` handler that answers `Path.scan`**, the host's `PermissionError` is an `OSError`, so
    `glob()` returns nothing and `walk()` reports it to `on_error`, as CPython does for an unreadable directory.
    Python's `AbstractOS` answers `Path.scan` by default; see [the filesystem guide](../filesystem.md).

## Path normalization and the sandbox

I/O calls are handled by mounts or a custom `os` callback. Mounts resolve paths
strictly within mounted roots. See [filesystem.md](filesystem.md).

`iterdir()` preserves the receiver's spelling: `Path('.').iterdir()` returns relative paths such as `Path('file.txt')`,
while `Path('subdir').iterdir()` returns paths beneath `subdir`, matching CPython.
The host receives an absolute request; the interpreter joins each returned entry's name onto the original directory path.
