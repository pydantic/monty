# Standard library modules

Monty ships a fixed set of built-in stdlib modules. `import` of anything
else raises `ModuleNotFoundError`: there is no `sys.path`, no site-packages,
and no way for sandboxed code to load additional modules.

Every `import` builds a fresh module object; there is no `sys.modules` cache.
So two imports of the same module are not the same object
(`import math as a; import math as b` leaves `a is not b`), and a mutable
attribute reverts on the next import — `sys.argv.append(...)` is not seen by a
later `import sys`. Module attributes cannot be set at all
(`sys.x = 1` raises `AttributeError`), so there is no way to share state
through a module.
The one exception is `random`'s module-level generator, which is session
state: a `random.seed(...)` is still in effect after a later `import random`,
in the next feed, and after a dump (see [random.md](random.md)).

## Modules available

| Module        | See                              |
| ------------- | -------------------------------- |
| `asyncio`     | [asyncio.md](asyncio.md)         |
| `base64`      | [base64.md](base64.md)           |
| `binascii`    | [base64.md](base64.md)           |
| `collections` | [collections.md](collections.md) |
| `copy`        | [copy.md](copy.md)               |
| `dataclasses` | [dataclasses.md](dataclasses.md) |
| `datetime`    | [datetime.md](datetime.md)       |
| `functools`   | [functools.md](functools.md)     |
| `itertools`   | [itertools.md](itertools.md)     |
| `json`        | [json.md](json.md)               |
| `math`        | [math.md](math.md)               |
| `os`          | [os.md](os.md)                   |
| `pathlib`     | [pathlib.md](pathlib.md)         |
| `random`      | [random.md](random.md)           |
| `re`          | [re.md](re.md)                   |
| `sys`         | [sys.md](sys.md)                 |
| `time`        | [time.md](time.md)               |
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
`abc`, `argparse`, `array`, `bisect`, `contextlib`, `csv`,
`ctypes`, `decimal`, `enum`, `fractions`,
`hashlib`, `heapq`, `hmac`, `http`, `inspect`, `io`,
`logging`, `multiprocessing`, `operator`, `pickle`, `queue`,
`socket`, `string`, `struct`, `subprocess`, `tempfile`, `threading`,
`traceback`, `unittest`, `urllib`, `uuid`, `warnings`, `weakref`,
`zipfile`, `zlib`.

`socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes` are
excluded because they would breach the sandbox. Others (`enum`, `operator`)
are unimplemented and may appear over time.

Some available modules cover only part of their CPython surface: `functools`
implements only `reduce` and `partial`, `copy` only `copy()` and `deepcopy()`,
`time` everything but `tzset` and the `clock_*` family, and `collections` only the four types above.
The absent names are missing from
the module namespace rather than stubbed, so they fail type checking as well as
raising `AttributeError` at runtime; see each module's page for the specifics.

## Modules the type checker resolves but the runtime does not

`abc`, `types`, `typing_extensions`, `_collections_abc` and `_typeshed` back
the vendored stubs (e.g. `@abstractmethod` on protocol members), so they have
to resolve during type checking. Importing them therefore type-checks clean but
still raises `ModuleNotFoundError` at runtime.
