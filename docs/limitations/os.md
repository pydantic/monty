# `os` module

The sandbox exposes a small, host-mediated subset of `os`. Filesystem
functions route through the same OS-call mechanism as `pathlib` and
`open()` (see [pathlib.md](pathlib.md), [open.md](open.md)): the host's mount table (or `os` callback) decides
whether each call is permitted.

## Implemented

- `os.getenv(key, default=None)` — yields to the host; the host decides
    which environment variables are visible (typically a curated subset, not
    the full host environment).
- `os.environ` — property that yields to the host and returns a `dict` of
    the same curated environment. It is a plain dict, not an `os._Environ`
    object: mutating it does **not** propagate back to the host.
- `os.listdir(path=None)` — returns a list of entry names.
- `os.stat(path)` — returns the same 10-field stat result as `Path.stat()`.
- `os.mkdir(path, mode=0o777)`, `os.makedirs(name, mode=0o777, exist_ok=False)`
- `os.remove(path)`, `os.unlink(path)`, `os.rmdir(path)`
- `os.rename(src, dst)`, `os.replace(src, dst)`
- `os.fspath(path)` — pure, no host involvement.
- Constants (fixed POSIX values on every host OS, matching the sandbox's
    POSIX-only path model): `os.sep == '/'`, `os.altsep is None`,
    `os.extsep == '.'`, `os.curdir == '.'`, `os.pardir == '..'`,
    `os.linesep == '\n'`, `os.name == 'posix'`, `os.devnull == '/dev/null'`.

## Divergences from CPython

- **No file descriptors, no `bytes` paths.** Paths must be `str` or
    `pathlib.Path`. `bytes` paths and integer fds (bools included, which CPython
    fd-converts with only a `RuntimeWarning`) raise the path-converter
    `TypeError` with the accepted-types phrase narrowed to what Monty takes,
    e.g. `stat: path should be string or os.PathLike, not bytes`. For every other
    rejected type the phrase is CPython's verbatim, so `os.stat(1.5)` still
    says `should be string, bytes, os.PathLike or integer`. Note `open()`
    *does* accept `bytes` paths, decoding them as UTF-8; the `os` functions do
    not. The verbatim `os.listdir` phrase is POSIX CPython's, which includes `integer`
    even though Windows CPython omits it (no fd-based listdir there); the narrowed
    phrase for `bytes`, `int` and `bool` is `string, os.PathLike or None`.
- **No `__fspath__` protocol.** `os.fspath` (and every path-taking function)
    accepts only `str`, `bytes` (fspath only), and `pathlib.Path`: a
    user-defined class implementing `__fspath__` raises `TypeError` instead of
    having its method called.
- **`dir_fd` keywords** (`dir_fd`, `src_dir_fd`, `dst_dir_fd`) are parsed
    for signature parity, but any non-`None` value raises the
    `NotImplementedError` CPython uses on platforms without them
    (`dir_fd unavailable on this platform`). Non-int values raise the
    converter `TypeError` (`argument should be integer or None, not str`).
- **`os.stat(..., follow_symlinks=...)`** raises
    `NotImplementedError: stat: follow_symlinks unavailable on this platform`
    for any *falsy* value. CPython truth-tests the argument, so `False`,
    `None` and `0` all mean "lstat", which Monty has no behavior for.
    `os.lstat` itself is not implemented.
- **All-keyword calls that overflow the signature** are not always reported
    the way CPython reports them. `os.fspath(path='a', foo=1)` and
    `os.listdir(path='.', foo=1)` match (`takes at most 1 keyword argument (2 given)`), but functions with keyword-only
    slots (`os.stat`, `os.mkdir`,
    `os.remove`, `os.rmdir`, `os.rename`) report the first unknown keyword
    (`stat() got an unexpected keyword argument 'foo'`) where CPython reports
    the arity (`stat() takes at most 3 keyword arguments (4 given)`).
- **No working directory.** `os.listdir()`'s default `'.'` (or any relative
    path) reaches the host unchanged; a mount table matches no mount and
    raises `PermissionError`.
- **`mode` arguments** are type-checked (`'str' object cannot be interpreted as an integer`) but otherwise ignored:
    Monty's filesystem
    backends do not model POSIX permission bits.
- **`os.replace` is an alias of `os.rename`** at the host boundary: both
    suspend with the same rename OS call, so overwrite semantics are whatever
    the host backend does (POSIX rename overwrites; a Windows host may
    refuse). CPython's `os.replace` guarantees overwrite on all platforms.
- **Hosts see pathlib-style call names.** `os.listdir` suspends as
    `Path.iterdir` (the interpreter reduces the returned paths to names),
    `os.stat` as `Path.stat`, `os.remove`/`os.unlink` as `Path.unlink`,
    `os.mkdir`/`os.makedirs` as `Path.mkdir`, `os.rename`/`os.replace` as
    `Path.rename`. A custom `os` callback cannot distinguish e.g. `os.listdir`
    from `Path.iterdir`.
- **`os.stat` results** print as `StatResult(...)`, not
    `os.stat_result(...)`, and carry only the 10 core fields, same as
    `Path.stat()` (see [filesystem.md](filesystem.md)).
- **Error side-effects differ slightly for `os.makedirs`**: Monty validates
    `mode` up front, while CPython only fails when it reaches the final
    `mkdir`, after creating parent directories.

## Not implemented

Everything else, including but not limited to: `os.path.*` (use
`pathlib.Path` instead), `os.getcwd`, `os.chdir`, `os.walk`, `os.scandir`,
`os.removedirs`, `os.renames`, `os.lstat`, `os.access`, `os.symlink`,
`os.readlink`, `os.link`, `os.chmod`, `os.chown`, `os.umask`, `os.truncate`,
`os.utime`, `os.system`, `os.popen`, `os.fork`, `os.exec*`, `os.spawn*`,
`os.kill`, `os.pipe`, `os.read`, `os.write`, `os.open`, `os.close`,
`os.dup`, `os.fsync`, `os.urandom`, `os.cpu_count`, `os.getpid`,
`os.getuid`, `os.getgid`, `os.uname`, `os.terminal_size`, `os.get_terminal_size`.

`subprocess`, `signal`, `socket`, `threading`, `multiprocessing` are not
importable either (see [modules.md](modules.md)).
