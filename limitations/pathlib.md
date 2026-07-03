# `pathlib` module

Only one class is exported: `pathlib.Path`. It always represents a virtual
POSIX path inside the sandbox — `/mnt/data/foo.txt`, never a Windows path
even when the host is Windows. `PurePath`, `PurePosixPath`, `PureWindowsPath`,
`PosixPath`, `WindowsPath` are not separately exposed (the printed `repr`
of a `Path` is `PosixPath(...)` for compatibility).

## Construction

`Path(*segments)` works. Each segment may be a `str` or another `Path`.
Bytes paths are rejected with `TypeError`.

`Path.cwd()` and `Path.home()` are **not** implemented — there is no
notion of "current directory" or "home" inside the sandbox.

## Pure (no I/O) methods and attributes

Implemented: `name`, `parent`, `stem`, `suffix`, `suffixes`, `parts`,
`is_absolute()`, `joinpath(*other)`, `with_name(name)`, `with_stem(stem)`,
`with_suffix(suffix)`, `as_posix()`, `__fspath__()`.

The `/` operator works (`Path("a") / "b"` → `Path("a/b")`).

Not implemented: `anchor`, `drive`, `root`, `relative_to`, `is_reserved`,
`match`, `full_match`, `__truediv__` with a `Path` on the *left* of a str
(but right-side str is fine), `with_segments`.

## I/O methods (yield to host)

These yield an `OsCall` for the host to resolve:

- `exists()`, `is_file()`, `is_dir()`, `is_symlink()`
- `read_text()`, `read_bytes()`
- `write_text(data)`, `write_bytes(data)`
- `mkdir(mode=0o777, parents=False, exist_ok=False)`, `unlink()`, `rmdir()`
- `iterdir()`, `stat()`, `rename(target)`
- `resolve()`, `absolute()`

`Path.mkdir()` parses `mode`, `parents`, and `exist_ok`, but `mode` is
accepted only for signature compatibility — Monty does not model POSIX
permission bits. The `missing_ok` and `target_is_directory` keyword arguments
accepted by other CPython methods are not parsed; pass only the positional
arguments documented above.

`Path.mkdir()`'s too-many-positional error counts only the visible
parameters (`Path.mkdir() takes from 0 to 3 positional arguments but 4 were
given`); CPython counts the bound `self` as well (`takes from 1 to 4 … but 5
were given`).

Not implemented: `glob`, `rglob`, `touch`, `chmod`, `lchmod`, `owner`,
`group`, `symlink_to`, `hardlink_to`, `link_to`, `readlink`, `lstat`,
`samefile`, `walk`, `open` (use the builtin `open()` instead),
`replace`, `expanduser`.

## Path normalization and the sandbox

Every I/O call routes through the host's mount table; paths are resolved
strictly within mounted roots. See [filesystem.md](filesystem.md).
