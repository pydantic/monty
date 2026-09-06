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

`Path.cwd()` and `Path('.').cwd()` return the sandbox's virtual working directory without a host
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

Not implemented: `glob`, `rglob`, `touch`, `chmod`, `lchmod`, `owner`,
`group`, `symlink_to`, `hardlink_to`, `link_to`, `readlink`, `lstat`,
`samefile`, `walk`, `replace`, `expanduser`.

## Path normalization and the sandbox

I/O calls are handled by mounts or a custom `os` callback. Mounts resolve paths
strictly within mounted roots. See [filesystem.md](filesystem.md).

`iterdir()` preserves the receiver's spelling: `Path('.').iterdir()` returns relative paths such as `Path('file.txt')`,
while `Path('subdir').iterdir()` returns paths beneath `subdir`, matching CPython.
The host receives an absolute request; the interpreter joins each returned entry's name onto the original directory path.
