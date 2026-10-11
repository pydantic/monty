# Standard library modules

Monty provides a fixed set of stdlib modules.
Other imports request [host modules](#host-modules) and raise `ModuleNotFoundError` if none is provided.
There is no `sys.path` or site-packages; sandboxed code cannot load additional modules itself.

`os.path` (also importable as `posixpath`) is the only submodule.

Every bundled import builds a fresh module object; there is no `sys.modules` cache.
Thus `import math as a; import math as b` leaves `a is not b`, and changes such as `sys.argv.append(...)` do not
survive the next import.
Assigning a bundled module's attributes raises `AttributeError`, as in `sys.x = 1`.
[Host modules](#host-modules) follow host-object rules instead.
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
`heapq`, `hmac`, `http`, `inspect`, `io`,
`logging`, `multiprocessing`, `operator`, `pickle`, `queue`,
`socket`, `string`, `struct`, `subprocess`, `tempfile`, `threading`,
`traceback`, `unittest`, `urllib`, `uuid`, `warnings`, `weakref`,
`zipfile`, `zlib`.

`socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes` are
excluded because they would breach the sandbox. Others (`enum`, `operator`)
are unimplemented and may appear over time.

Some available modules cover only part of their CPython surface: `functools`
implements only `reduce` and `partial`, `copy` only `copy()` and `deepcopy()`,
`time` everything but `tzset` and the `clock_*` family, `hashlib` everything but `file_digest()` and `scrypt()`,
and `collections` only the four types above.
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
The host's answer becomes the imported value, with these differences from CPython:

- `type(m)`, `repr(m)` and `m.__class__` describe the host object.
    Other dunder attributes, including `m.__name__` and `m.__dict__`, raise `AttributeError`.
- Missing-attribute and not-callable errors name the host class (`'m' object`) rather than CPython's `module`.
- Every `import` requests the module again, including imports inside functions.
    Two imports bind distinct objects: `import m as a; import m as b` leaves `a is not b`.
- Assigned attributes affect only that binding.
    Python rereads host attributes on the next import; JavaScript reuses the first module value for the session.
    JavaScript therefore ignores later host changes to plain attributes, but both bindings reread attributes for function
    calls.
- Host exceptions are raised at the `import`.
- Host modules are treated as packages: a missing submodule raises `ModuleNotFoundError: No module named 'm.x'`.
    `import m.x` binds any attribute `x`, including plain values that CPython would reject as non-modules.
- `from m import x` reads eager or lazy attributes; missing names raise
    `ImportError: cannot import name 'x' from 'm' (unknown location)`.
- Python classes in module dicts become host functions: `m.X()` constructs on the host, but `isinstance(v, m.X)`
    raises `TypeError`.
    Wrap a class in `ClassType` to send it as a type.
- Module names and dict keys must be identifiers, so `getattr(m, 'a.b')` cannot find a dict module attribute.
    `ClassInstance` modules follow their wrapper's attribute policy.
- Imports with more than 32 dotted components raise `SyntaxError`; `import a.b.c` has three.

With no host to answer, `monty file.py` included, every unknown module raises `ModuleNotFoundError`.
