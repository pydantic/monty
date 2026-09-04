# Standard library modules

Monty ships a fixed set of built-in stdlib modules. `import` of anything
else raises `ModuleNotFoundError`: there is no `sys.path`, no site-packages,
and no way for sandboxed code to load additional modules.

## Modules available

| Module        | See                              |
| ------------- | -------------------------------- |
| `asyncio`     | [asyncio.md](asyncio.md)         |
| `base64`      | [base64.md](base64.md)           |
| `binascii`    | [base64.md](base64.md)           |
| `collections` | [collections.md](collections.md) |
| `dataclasses` | [dataclasses.md](dataclasses.md) |
| `datetime`    | [datetime.md](datetime.md)       |
| `functools`   | [functools.md](functools.md)     |
| `itertools`   | [itertools.md](itertools.md)     |
| `json`        | [json.md](json.md)               |
| `math`        | [math.md](math.md)               |
| `os`          | [os.md](os.md)                   |
| `pathlib`     | [pathlib.md](pathlib.md)         |
| `re`          | [re.md](re.md)                   |
| `sys`         | [sys.md](sys.md)                 |
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
`abc`, `argparse`, `array`, `bisect`, `contextlib`, `copy`, `csv`,
`ctypes`, `decimal`, `enum`, `fractions`,
`hashlib`, `heapq`, `hmac`, `http`, `inspect`, `io`,
`logging`, `multiprocessing`, `operator`, `pickle`, `queue`, `random`,
`socket`, `string`, `struct`, `subprocess`, `tempfile`, `threading`,
`time`, `traceback`, `unittest`, `urllib`, `uuid`, `warnings`, `weakref`,
`zipfile`, `zlib`.

`socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes` are
excluded because they would breach the sandbox. Others (`enum`, `operator`)
are unimplemented and may appear over time.

Some available modules cover only part of their CPython surface: `itertools`
implements eleven of its callables, `functools` only `reduce`, `partial`,
`lru_cache` and `cache`, `collections` only the four types above, and
`binascii` everything except the uuencode and quoted-printable conversions.
The absent names are missing from the module namespace rather than stubbed, so
they fail type checking as well as raising `AttributeError` at runtime; see
each module's page for the specifics.

## Modules the type checker resolves but the runtime does not

`abc`, `types`, `typing_extensions`, `_collections_abc` and `_typeshed` back
the vendored stubs (e.g. `@abstractmethod` on protocol members), so they have
to resolve during type checking. Importing them therefore type-checks clean but
still raises `ModuleNotFoundError` at runtime.
