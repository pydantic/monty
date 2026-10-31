# Standard library modules

Monty provides a fixed set of stdlib modules.
Other imports ask the host, which answers with a [host module](#host-modules) or the module's
[`.py` file](#modules-from-files) from the working directory, and raise `ModuleNotFoundError` if it has neither.
There is no `sys.path` or site-packages.

`os.path` (also importable as `posixpath`) is the only submodule.

Every bundled import builds a fresh module object; there is no `sys.modules` cache, except for
[modules from files](#modules-from-files).
Thus `import math as a; import math as b` leaves `a is not b`, and changes such as `sys.argv.append(...)` do not
survive the next import.
Assigning a bundled module's attributes raises `AttributeError`, as in `sys.x = 1`, except for
`secrets.DEFAULT_ENTROPY`, which is mutable.
[Host modules](#host-modules) are module objects too, bound afresh by every import.

Although each import creates a new module object, `random`'s module-level generator and `secrets.DEFAULT_ENTROPY` are
session state.
A `random.seed(...)` remains in effect after a later `import random`, in the next feed, and after a dump
(see [random.md](random.md)).
Changes to `secrets.DEFAULT_ENTROPY` persist across imports, REPL snippets and session dump/load (see [secrets.md](secrets.md)).

## Modules available

| Module        | See                              |
| ------------- | -------------------------------- |
| `asyncio`     | [asyncio.md](asyncio.md)         |
| `base64`      | [base64.md](base64.md)           |
| `binascii`    | [base64.md](base64.md)           |
| `bisect`      | [bisect.md](bisect.md)           |
| `collections` | [collections.md](collections.md) |
| `copy`        | [copy.md](copy.md)               |
| `csv`         | [csv.md](csv.md)                 |
| `dataclasses` | [dataclasses.md](dataclasses.md) |
| `datetime`    | [datetime.md](datetime.md)       |
| `decimal`     | [decimal.md](decimal.md)         |
| `fnmatch`     | [fnmatch.md](fnmatch.md)         |
| `functools`   | [functools.md](functools.md)     |
| `hashlib`     | [hashlib.md](hashlib.md)         |
| `itertools`   | [itertools.md](itertools.md)     |
| `json`        | [json.md](json.md)               |
| `math`        | [math.md](math.md)               |
| `os`          | [os.md](os.md)                   |
| `os.path`     | [os.md](os.md)                   |
| `pathlib`     | [pathlib.md](pathlib.md)         |
| `posixpath`   | [os.md](os.md)                   |
| `random`      | [random.md](random.md)           |
| `re`          | [re.md](re.md)                   |
| `secrets`     | [secrets.md](secrets.md)         |
| `sys`         | [sys.md](sys.md)                 |
| `time`        | [time.md](time.md)               |
| `tomllib`     | [tomllib.md](tomllib.md)         |
| `typing`      | [typing.md](typing.md)           |
| `unicodedata` | [unicodedata.md](unicodedata.md) |

`collections` is importable and exposes `deque`, `Counter`, `defaultdict`,
and `namedtuple`; `OrderedDict`, `ChainMap`, and the `UserDict` / `UserList`
/ `UserString` wrappers are missing (see [collections.md](collections.md)).

A `gc` module exposing `collect()` / `enable()` / `disable()` is compiled
in only under the `test-hooks` Cargo feature, for Monty's own test suite;
production sandboxes never see it.

## Notable modules NOT available

Common modules that are *not* importable in Monty (non-exhaustive):
`abc`, `argparse`, `array`, `contextlib`,
`ctypes`, `enum`, `fractions`,
`heapq`, `hmac`, `http`, `inspect`, `io`,
`logging`, `multiprocessing`, `operator`, `pickle`, `queue`,
`socket`, `string`, `struct`, `subprocess`, `tempfile`, `threading`,
`traceback`, `unittest`, `urllib`, `uuid`, `warnings`, `weakref`,
`zipfile`, `zlib`.

`socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes` are
excluded because they would breach the sandbox. Others (`enum`, `operator`)
are unimplemented and may appear over time.

Some available modules cover only part of their CPython surface: `functools`
implements only `reduce`, `partial`, `lru_cache`, and `cache`, `copy` only `copy()` and `deepcopy()`,
`time` everything but `tzset` and the `clock_*` family, `hashlib` everything but `file_digest()` and `scrypt()`,
and `collections` only the four types above; `tomllib` all three of its names, with the argument and exception differences listed in [tomllib.md](tomllib.md).
The absent names are missing from
the module namespace rather than stubbed, so they fail type checking as well as
raising `AttributeError` at runtime; see each module's page for the specifics.

## Modules the type checker resolves but the runtime does not

`abc`, `enum`, `io`, `_io`, `types`, `typing_extensions`, `_collections_abc` and `_typeshed` support the vendored
stubs, so their imports pass type checking.
`io` and `_io` declare the file objects `open()` returns.
At runtime, the host must provide them or the import raises `ModuleNotFoundError`.
The worker never requests host stubs for vendored typeshed modules: a host-provided `abc` is checked against
typeshed's `abc` declarations.

## Host modules

Unknown imports request the top-level name through the `import` OS call; `__import__` is undefined.
The same call carries the module's search path, so a host with no module of that name can answer with its
[file](#modules-from-files) instead; an exception the host raises is the import's error.
Dotted imports such as `import a.b.c` request `a`, then read `b` and `c` as successive attributes of a host module.
Bundled modules that are not packages never request host submodules: `import os.x` raises
`ModuleNotFoundError: No module named 'os.x'; 'os' is not a package`.
The host's answer becomes the imported `module` object, with these differences from CPython:

- Dunder attributes, including `m.__name__` and `m.__dict__`, raise `AttributeError`.
- Every `import` requests the module again, including imports inside functions.
    Two imports bind distinct objects: `import m as a; import m as b` leaves `a is not b`.
- Assigning an attribute raises `AttributeError`, as on a bundled module; a function call runs the host's current
    attribute, and the next import binds the module afresh.
- Host exceptions are raised at the `import`.
- Host modules are packages: a missing submodule, or `import m.x` for an attribute `x` that is not a module, raises
    `ModuleNotFoundError: No module named 'm.x'`.
- `from m import x` reads the module's attributes; missing names raise
    `ImportError: cannot import name 'x' from 'm' (unknown location)`.
- Python classes among a module's attrs become host functions: `m.X()` constructs on the host, but
    `isinstance(v, m.X)` raises `TypeError`.
    Wrap a class in `ClassType` to send it as a type.
- Imports with more than 32 dotted components raise `SyntaxError`; `import a.b.c` has three.

With no host or mount to answer, every unknown module raises `ModuleNotFoundError`.
`monty file.py` answers imports from its `--mount` directories only (see [files](#modules-from-files)).

## Modules from files

An `import` the host has no module for runs the module's file, which the host finds on the search path the `import`
call carries: `<dir>/name/__init__.py`, then `<dir>/name.py`, for each directory.
The bindings read each candidate through the `os` handler and the feed's mounts (see
[importing modules](../filesystem.md#importing-modules)).
`FileNotFoundError`, `NotADirectoryError`, `IsADirectoryError` and `PermissionError` (including the answer outside
any mount) move on to the next file; any other error raises at the `import`.
Differences from CPython:

- The working directory at the time of the `import` is the only search location.
    There is no `sys.path`, `PYTHONPATH`, `.pth` file or site-packages.
- Bundled modules are never shadowed: a `json.py` in the working directory is not read by `import json`, where
    CPython would import it.
- An unreadable module file is skipped rather than raising `PermissionError`, so the import raises
    `ModuleNotFoundError` when no later candidate exists.
- Each module the cache lacks costs one `import` call, one suspension against
    [`max_suspensions`](resource_limits.md#suspensions), however many files the host reads.
- A worker or binding from a release before file imports leaves the search out, so a mixed pairing raises
    `ModuleNotFoundError` for every module file.
- A file module is cached for the session by its dotted name, but there is no `sys.modules` or `importlib`, so nothing
    clears the cache: a file edited after its first import is not read again, even in later feeds or after `os.chdir`,
    and later imports of the name bind the cached module without asking the host.
    A module whose body raises is dropped from the cache, so the next import runs it again.
- Each submodule of a file package is its own `import` call for the dotted name (`pkg.sub`), searching the package's
    `__path__` directories, joined to the working directory.
    Relative imports (`from . import x`) are rejected, so a package that uses them raises
    `ImportError: attempted relative import with no known parent package` when imported.
    Directories without an `__init__.py` (namespace packages) are not found.
- Module globals are `__name__`, `__file__`, `__package__` and, for a package, `__path__`; `__spec__`, `__loader__`,
    `__cached__`, `__builtins__` and `__doc__` are not defined.
    `m.__dict__` raises `AttributeError`, and `vars` and `dir` are not implemented (see [builtins.md](builtins.md)).
- Module code looks names up in its globals and the builtins, never on the host: calling a
    [host function](../host-functions.md) by name raises `NameError` unless the importer binds it on the module first,
    as in `helpers.fetch = fetch`.
- A `SyntaxError` in a module file shows the file's frame and line, but `str(exc)` lacks CPython's
    `(name.py, line N)` suffix.
- No bytecode is cached: each session compiles every module it imports.
    The source of an imported module stays allocated for the rest of the session, counted against `max_memory`.
- The type checker does not read module files, so a type-checked feed that imports a file module fails with
    `unresolved-import`.
    [Module stubs](../type-checking.md) come only with host modules, which would shadow the file; feed such code with
    `skip_type_check=True`.
