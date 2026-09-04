# Limitations

Monty is not a Python implementation aiming for completeness.
It implements enough Python for a model to express what it wants to do, and deliberately stops there.
Everything it does implement is meant to behave exactly like CPython 3.14; everywhere it does not, the divergence is
written down.

This page gives you the shape of the subset.
The other pages in this section are the exhaustive record, one per builtin, module or construct.
They list every known divergence, including the ones that feel obvious, so a behaviour missing from them can be
assumed to match CPython 3.14.
They exist for development and for agents debugging code that runs on Monty; most users need only this page.

!!! tip

    Turn on [type checking](../type-checking.md) rather than memorising this page.
    Unsupported APIs generally fail before they run; see [the caveats](../type-checking.md#caveats) for the exceptions.

## Language features

**Supported:**

- `def`, `async def`, nested functions, closures, `lambda`
- Decorators on functions and classes
- Simple classes: instance methods, `__init__`, `__repr__`/`__str__`, `__eq__`/`__hash__`, `__iter__`/`__next__`,
    `__contains__`, `__index__`, class variables
- `@dataclass`, with the `eq=` and `frozen=` options only (every other option raises `NotImplementedError`, and
    there is no `field()`, `fields()` or `asdict()`), plus host class instances passed in and out (and host classes the
    sandbox may instantiate when granted)
- List, dict and set comprehensions
- `try` / `except` / `else` / `finally`, `raise ... from ...`
- `for`, `while`, `if` / `elif` / `else`, `break`, `continue`, `pass`, `assert`, `global`, `nonlocal`, `return`
- `with` statements, for files and for classes implementing `__enter__` / `__exit__`
- f-strings, including `=`, `!r` / `!s` / `!a` and format specs
- `async` / `await`, and `asyncio.run` / `asyncio.gather`
- `import x`, `import x.y`, `from x import y, z as w`
- Starred unpacking everywhere CPython allows it

**Rejected at parse time**, with `NotImplementedError` before any code runs:

- Class inheritance and metaclasses (`class Foo(Bar):`)
- Decorators on methods — so no `@classmethod`, `@staticmethod`, `@property`
- `yield` / `yield from` — there are no generator functions.
    Generator *expressions* parse, but currently materialise to a `list`
- `match` statements
- `del`, both `del x` and `del d[k]`
- `try*` / `except*` exception groups
- PEP 695 `type` aliases
- `async with`, `async for` and async comprehensions
- Wildcard imports (`from m import *`)
- Complex literals (`1j`) and t-strings

**Missing in other ways:**

- User-defined exception classes.
    The built-in exception types are a fixed set, and without inheritance you cannot add to it.
- Function attributes.
    `fn.__name__`, `fn.__doc__` and friends raise `AttributeError`, and new attributes cannot be set — so
    `functools.wraps`-style metadata copying and registries keyed on `fn.__name__` have no equivalent.
- `eval`, `exec`, `compile`, `globals`, `locals`, `__import__` and `super` — all raise `NameError`.
- Third-party packages.
    There is no `sys.path` and no site-packages.

## Standard library

The following modules are present:

| Module        | Divergences                      |
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

Each covers only part of its CPython surface — often a small part.
The absent names are missing from the module namespace rather than stubbed, so they fail type checking as well as
raising `AttributeError` at runtime.

Notably absent: `enum`, `contextlib`, `random`, `time`, `io`, `copy`, `string`, `struct`, `operator`,
`inspect`, `logging`, `traceback`, `hashlib`, `uuid`, `urllib`.
Some of those are absent by design — `socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes` would breach
the sandbox — and others are simply not implemented yet.

The authoritative list is [modules.md](modules.md).

## Things that work but not quite like CPython

A few divergences are worth knowing up front because they change how code behaves rather than whether it runs.
Each links to the page that owns it, which is where the full account lives:

- **`assert` failures get pytest-style messages.** `assert 2 == 5` raises `AssertionError: assert 2 == 5`, not CPython's
    empty `AssertionError`.
    Turn it off with `assert_message_annotations=False` on `checkout()`
    ([assert.md](assert.md)).
- **`enumerate`, `zip`, `map`, `filter` and `reversed` are eager**, not lazy.
    So `map(f, itertools.count())` runs until a resource limit trips
    ([builtins.md](builtins.md)).
- **`re` is backed by Rust's `fancy-regex`**, not CPython's engine: no `bytes` patterns, no `VERBOSE` flag, and some
    error messages differ ([re.md](re.md)).
- **Only the class dunders listed above are dispatched.** `__lt__`, `__len__`, `__getitem__`, `__call__` and the
    arithmetic dunders raise `TypeError` as if undefined, while `__bool__` and the `__getattr__` family are ignored
    silently, so an instance is always truthy ([classes.md](classes.md)).
- **There is no event loop inside the sandbox.** `async` / `await` work, and `asyncio` exposes exactly two functions:
    `run` and `gather`, the latter running host calls concurrently.
    `create_task`, `sleep` and everything else do not exist
    ([asyncio.md](asyncio.md)).
- **`str.format()` and `%`-formatting are not implemented.** Use f-strings
    ([format.md](format.md)).
- **Only UTF-8, ASCII, UTF-16 and UTF-32 codecs exist.** `latin-1` and friends raise `LookupError`
    ([encoding.md](encoding.md)).

## How to go deeper

For a specific feature, open the page in this section named after the builtin, module or construct.
The pages are the `limitations/` directory of the [repository](https://github.com/pydantic/monty), published
verbatim.
If you hit something that is neither in the subset nor on these pages, [open an
issue](https://github.com/pydantic/monty/issues).
