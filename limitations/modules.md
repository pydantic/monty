# Standard library modules

Monty ships a fixed set of built-in stdlib modules. `import` of anything
else raises `ModuleNotFoundError`: there is no `sys.path`, no site-packages,
and no way for sandboxed code to load additional modules.

## Modules available

| Module         | See              |
| -------------- | ---------------- |
| `asyncio`      | ./asyncio.md     |
| `collections`  | ./collections.md |
| `dataclasses`  | ./dataclasses.md |
| `datetime`     | ./datetime.md    |
| `itertools`    | ./itertools.md   |
| `json`         | ./json.md        |
| `math`         | ./math.md        |
| `os`           | ./os.md          |
| `pathlib`      | ./pathlib.md     |
| `re`           | ./re.md          |
| `sys`          | ./sys.md         |
| `typing`       | ./typing.md      |
| `unicodedata`  | ./unicodedata.md |

`collections` is importable and exposes `deque`, `Counter`, `defaultdict`,
and `namedtuple`; `OrderedDict`, `ChainMap`, and the `UserDict` / `UserList`
/ `UserString` wrappers are missing (see ./collections.md).

A `gc` module exposing `collect()` / `enable()` / `disable()` is compiled
in only under the `test-hooks` Cargo feature, for Monty's own test suite;
production sandboxes never see it.

## Notable modules NOT available

Common modules that are *not* importable in Monty (non-exhaustive):
`abc`, `argparse`, `array`, `base64`, `bisect`, `contextlib`, `copy`, `csv`,
`ctypes`, `decimal`, `enum`, `fractions`, `functools`,
`hashlib`, `heapq`, `hmac`, `http`, `inspect`, `io`,
`logging`, `multiprocessing`, `operator`, `pickle`, `queue`, `random`,
`socket`, `string`, `struct`, `subprocess`, `tempfile`, `threading`,
`time`, `traceback`, `unittest`, `urllib`, `uuid`, `warnings`, `weakref`,
`zipfile`, `zlib`.

`socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes` are
excluded because they would breach the sandbox. Others (`functools`, `enum`)
are unimplemented and may appear over time.

Some available modules cover only part of their CPython surface: `itertools`
implements just `count` and `repeat` so far, and `collections` only the four
types above. The absent names are missing from the module namespace rather than
stubbed, so they fail type checking as well as raising `AttributeError` at
runtime; see each module's page for the specifics.

## Modules the type checker resolves but the runtime does not

`abc`, `types`, `typing_extensions`, `_collections_abc` and `_typeshed` back
the vendored stubs (e.g. `@abstractmethod` on protocol members), so they have
to resolve during type checking. Importing them therefore type-checks clean but
still raises `ModuleNotFoundError` at runtime.
