# Standard library modules

Monty ships a fixed set of built-in stdlib modules. `import` of anything
else asks the host for the module (see [Host modules](#host-modules)) and
raises `ModuleNotFoundError` when the host provides none: there is no
`sys.path`, no site-packages, and no way for sandboxed code to load additional
modules itself.

`os.path` (also importable as `posixpath`) is the only submodule.

Every `import` of a bundled module builds a fresh module object; there is no
`sys.modules` cache. So two imports of the same module are not the same object
(`import math as a; import math as b` leaves `a is not b`), and a mutable
attribute reverts on the next import — `sys.argv.append(...)` is not seen by a
later `import sys`. A bundled module's attributes cannot be set at all
(`sys.x = 1` raises `AttributeError`), so there is no way to share state
through one. A host module is a host object, with the rules of the
[Host modules](#host-modules) section instead.
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

`abc`, `enum`, `types`, `typing_extensions`, `_collections_abc` and `_typeshed`
back the vendored stubs (e.g. `@abstractmethod` on protocol members), so they
have to resolve during type checking. Importing them type-checks clean; at
runtime they are host modules like any other name the sandbox lacks, so the
host may serve them, and a not-found answer raises `ModuleNotFoundError`.
A module stub cannot be declared under these names, nor under any other module
of the vendored typeshed, so the checker checks a host-served `abc` against
typeshed's `abc`, not against what the host serves.

## Host modules

An `import` of any module the runtime does not ship, the checker-only ones above included, asks the host for it as
the external function call `__import__` with the top-level module name as its argument.
A dotted import (`import a.b.c`, `from a.b import c`) asks for `a` and reads each further component as an attribute of
the one before, as CPython reads a submodule out of its package; below a bundled module that is not a package
(`import os.x`) the attribute read raises `ModuleNotFoundError: No module named 'os.x'; 'os' is not a package`
without asking the host.
The value the host answers with (in the bindings, the matching `external_modules` entry) is bound as the module, so:

- the value is a host object, not a module: `type(m)` is its host class, `repr(m)` its host repr and `m.__class__`
    that class, while `m.__name__` and `m.__dict__` raise `AttributeError`, as every other dunder attribute of a host
    object does;
- a missing attribute raises `AttributeError: 'm' object has no attribute 'x'`, naming the host class rather than
    CPython's `module 'm' has no attribute 'x'`, and calling the module raises `TypeError: 'm' object is not callable`
    where CPython says `'module' object is not callable`;
- every `import` statement asks again, since there is no `sys.modules` cache, so two imports of one module bind two
    objects (`import m as a; import m as b` leaves `a is not b`), and an import inside a function asks on each call;
- an attribute can be assigned, as on any host object, but only that binding sees it: the next `import` starts
    from the host's attributes again;
- a not-found answer raises `ModuleNotFoundError: No module named 'm'`, as in CPython, and an exception raised by the
    host is raised at the `import`;
- a submodule the answer lacks raises `ModuleNotFoundError: No module named 'm.x'`, the wording CPython uses for a
    package without that submodule, since a host module is never told apart from a package;
- `from m import x` reads `x` from the answered value, whether sent with it or looked up lazily, and raises
    `ImportError: cannot import name 'x' from 'm' (unknown location)` when it has no such attribute;
- in the Python binding a plain class in a module dict crosses as a host function named `m.X` (calling it constructs
    on the host), not as a type, so `isinstance(v, m.X)` raises `TypeError`; wrap it in `ClassType` to send a type;
- module names and the keys of a module dict are identifiers, since a host function is named by its dotted path, so
    `getattr(m, 'a.b')` has nothing to find.

With no host to answer, `monty file.py` included, every unknown module raises `ModuleNotFoundError`.
