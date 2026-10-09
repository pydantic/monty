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
- `os.urandom(size)` — yields to the host, which must return exactly `size` bytes; any other length, or a
    non-`bytes` value, raises `RuntimeError`.
    Sandboxed code chooses `size`, so a host handler that allocates must cap it.
    Python's `AbstractOS.urandom()` raises `MemoryError` before allocating when `size` exceeds `max_urandom_bytes`,
    1 MiB by default; `OSAccess(max_urandom_bytes=...)` sets it, and zero rejects every nonempty request.
    The `random` module makes this call only under `random_start='call_host'`; by default an unseeded generator
    seeds itself from OS entropy inside the sandbox (see [random.md](random.md)).
- `os.fspath(path)` — pure, no host involvement.
- `os.getcwd()` — pure: the sandbox's virtual working directory.
- `os.getcwdb()` — pure: the same directory as UTF-8 bytes (virtual paths
    are always UTF-8, so no filesystem encoding is involved).
- `os.chdir(path)` — validated through a `Path.stat` host call (see below).
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
- **The working directory is virtual and belongs to the session.** A
    session's first feed sets it (an explicit `cwd`, else that feed's first
    mount's virtual path, else `/`); it then persists across feeds, including
    any `os.chdir()`, until a feed passes `cwd` again. `os.getcwd()` reports
    it and relative paths are resolved against it inside the interpreter, so
    a mount or `os` callback only ever sees absolute paths. Host errors
    therefore name the resolved path
    (`open('missing')` raises `[Errno 2] No such file or directory: '/data/missing'`) where CPython names the argument as written. Absolute
    paths reach mounts as written.
    Joining preserves `.` and `..` so mounts can validate NUL bytes and path limits before normalization.
    Python and JavaScript `os` callbacks receive lexically normalized paths, including both rename arguments.
    The interpreter rejects NUL bytes before dispatch, even in components cancelled by `..`.
    Existence predicates return `False` for these paths; other operations raise `ValueError`.
    Length and depth limits are mount policy: a feed with any mount applies them to every path before the callback
    sees it, including paths no mount covers, while a feed with no mounts passes paths of any length to the callback.
- **`os.chdir(path)` suspends as `Path.stat`** on the resolved target: hosts
    cannot observe a directory change, and without a mount or `os` handler it
    raises `PermissionError`. The interpreter raises `NotADirectoryError`
    when the reply is not a directory, naming the argument as written like
    CPython; `FileNotFoundError` comes from the host and names the resolved
    path. `os.chdir('')` raises `FileNotFoundError` without consulting the
    host. Only after the host accepts the target is the stored directory lexically normalized
    (`..` collapses without consulting symlinks). Integer file descriptors are refused with
    the `path_t` `TypeError`; CPython would `fchdir`. A Rust host that
    answers the stat with a future gets `RuntimeError` instead of a silently
    unchanged directory.
- **`mode` arguments** are type-checked (`'str' object cannot be interpreted as an integer`) but otherwise ignored:
    Monty's filesystem
    backends do not model POSIX permission bits.
- **`os.replace` is an alias of `os.rename`** at the host boundary: both
    suspend with the same rename OS call, so overwrite semantics are whatever
    the host backend does (POSIX rename overwrites; a Windows host may
    refuse). CPython's `os.replace` guarantees overwrite on all platforms.
- **Hosts see pathlib-style call names.** `os.listdir` suspends as
    `Path.iterdir` (the interpreter reduces the returned paths to names),
    `os.stat` and `os.chdir` as `Path.stat`, `os.remove`/`os.unlink` as `Path.unlink`,
    `os.mkdir`/`os.makedirs` as `Path.mkdir`, `os.rename`/`os.replace` as
    `Path.rename`. A custom `os` callback cannot distinguish e.g. `os.listdir`
    from `Path.iterdir`.
- **`os.stat` results** print as `StatResult(...)`, not
    `os.stat_result(...)`, and carry only the 10 core fields, same as
    `Path.stat()` (see [filesystem.md](filesystem.md)).
- **Error side-effects differ slightly for `os.makedirs`**: Monty validates
    `mode` up front, while CPython only fails when it reaches the final
    `mkdir`, after creating parent directories.

## `os.path`

`os.path` is CPython's `posixpath` on every host, matching the sandbox's POSIX-only path model; `import posixpath`
yields the same module. `import os.path` binds `os` like CPython, `import os.path as p` binds the module, and
`from os.path import join` works.

Implemented, pure (no host involvement), accepting `str`, `bytes` and `pathlib.Path` like CPython:
`join`, `split`, `splitext`, `splitdrive`, `splitroot`, `basename`, `dirname`, `normpath`, `normcase`, `isabs`,
`abspath`, `relpath`, `commonpath`, `commonprefix`, `samestat`, `isjunction`, `isdevdrive`, and the constants
`sep`, `altsep`, `extsep`, `curdir`, `pardir`, `pathsep`, `defpath`, `devnull` (`os.pathsep` and `os.defpath` too).
`abspath` and `relpath` use the session's virtual working directory (see above), exactly as CPython's use
`os.getcwd()`.

Implemented through the host, with the same `str`/`Path`-only rule as the other `os` functions:

- `exists`, `isfile`, `isdir`, `islink` suspend as `Path.exists`, `Path.is_file`, `Path.is_dir`, `Path.is_symlink`.
    The empty path and a path containing a NUL byte answer `False` without consulting the host.
- `lexists` suspends as `Path.exists` and, when that answers `False`, as `Path.is_symlink`, so a dangling symlink
    counts as existing like CPython's `lstat`.
- `ismount` suspends as `Path.exists`: every existing path is reported as a mount point. The sandbox cannot see
    where the host's mounts begin, and `True` is what keeps "walk up until a mount point" loops terminating.
- `samefile` suspends as `Path.stat` on each path in turn (the second path's errors are raised after the first
    stat succeeds, as CPython orders them) and compares `(st_ino, st_dev)` like `samestat`. When neither reply
    carries an identity, which is the case for every mount, the two normalized virtual paths decide instead, so
    hard links are not detected there.
- `getsize`, `getmtime`, `getatime`, `getctime` suspend as `Path.stat` and return one field of the reply; a host
    answering with something other than a stat result raises `RuntimeError`.
- `realpath` suspends as `Path.resolve` and returns the reply as `str`. Mounts resolve lexically (see
    [filesystem.md](filesystem.md)), so a missing path does not raise unless `strict` is true, which adds a
    `Path.exists` call on the result and raises `FileNotFoundError` naming the resolved path when it is missing.
    The `NotADirectoryError` and symlink-loop `OSError` of CPython's strict mode never occur, and `ALLOW_MISSING`
    does not exist.
- `expanduser` suspends as `os.getenv('HOME')` only for a path starting with `~` or `~/`. When the host answers
    `None` the path is returned unchanged: CPython would fall back to the password database, which the sandbox
    cannot read, and for the same reason `~user` is always returned unchanged.
- `expandvars` suspends as `os.environ` only for a path containing `$`; entries whose key or value is not `str` are
    ignored. A `bytes` path is matched against the same `str` environment encoded as UTF-8 (CPython reads
    `os.environb`).

Divergences:

- `samestat` compares `st_ino` and `st_dev`, which mounts report as `0` for every file, so two mount stat results
    always compare equal.
- `commonprefix` on lists whose elements cannot be ordered reports the lists
    (`'<' not supported between instances of 'list' and 'list'`) where CPython names the elements; this is Monty's
    general list-comparison wording.
- `sameopenfile` is not implemented: there are no file descriptors to `fstat`.
- `supports_unicode_filenames` is always `False` (CPython sets it on macOS only).

## Not implemented

Everything else, including but not limited to: `os.fchdir`, `os.walk`, `os.scandir`,
`os.removedirs`, `os.renames`, `os.lstat`, `os.access`, `os.symlink`,
`os.readlink`, `os.link`, `os.chmod`, `os.chown`, `os.umask`, `os.truncate`,
`os.utime`, `os.system`, `os.popen`, `os.fork`, `os.exec*`, `os.spawn*`,
`os.kill`, `os.pipe`, `os.read`, `os.write`, `os.open`, `os.close`,
`os.dup`, `os.fsync`, `os.cpu_count`, `os.getpid`,
`os.getuid`, `os.getgid`, `os.uname`, `os.terminal_size`, `os.get_terminal_size`.

`subprocess`, `signal`, `socket`, `threading`, `multiprocessing` are not
importable either (see [modules.md](modules.md)).
