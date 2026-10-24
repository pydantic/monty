# Standard library modules

Monty provides a fixed set of stdlib modules.
Other imports request [host modules](#host-modules) and raise `ModuleNotFoundError` if none is provided.
There is no `sys.path` or site-packages; sandboxed code cannot load additional modules itself.

`os.path` (also importable as `posixpath`) is the only submodule.

Every bundled import builds a fresh module object; there is no `sys.modules` cache.
Thus `import math as a; import math as b` leaves `a is not b`, and changes such as `sys.argv.append(...)` do not
survive the next import.
Assigning a bundled module's attributes raises `AttributeError`, as in `sys.x = 1`.
[Host modules](#host-modules) are module objects too, bound afresh by every import.
The one exception is `random`'s module-level generator, which is session
state: a `random.seed(...)` is still in effect after a later `import random`,
in the next feed, and after a dump (see [random.md](random.md)).

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

`abc`, `enum`, `types`, `typing_extensions`, `_collections_abc` and `_typeshed` support the vendored stubs, so their
imports pass type checking.
At runtime, the host must provide them or the import raises `ModuleNotFoundError`.
The worker never requests host stubs for vendored typeshed modules: a host-provided `abc` is checked against
typeshed's `abc` declarations.

## Host modules

Unknown imports request the top-level name through the `import` OS call; `__import__` is undefined.
Dotted imports such as `import a.b.c` request `a`, then read `b` and `c` as successive attributes.
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

With no host to answer, `monty file.py` included, every unknown module raises `ModuleNotFoundError`.
