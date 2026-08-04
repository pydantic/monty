# The Python Subset

Monty is not a Python implementation aiming for completeness. It implements enough Python
for a model to express what it wants to do, and deliberately stops there. Everything it
does implement is meant to behave exactly like CPython 3.14; everywhere it does not, the
divergence is written down.

That last part is the important one. This page gives you the shape of the subset. The
exhaustive, per-feature list lives in
[`limitations/`](https://github.com/pydantic/monty/tree/main/limitations) in the
repository, and that directory — not this page — is the source of truth.

!!! tip
    You do not have to memorise any of this. Turn on [type checking](type-checking.md):
    Monty checks against a typeshed trimmed to exactly what it implements, so code
    reaching for something unsupported fails before it runs.

## Language features

**Supported:**

- `def`, `async def`, nested functions, closures, `lambda`
- Decorators on functions and classes
- Simple classes: instance methods, `__init__`, `__repr__`/`__str__`, `__eq__`/`__hash__`,
  class variables
- `@dataclass` (basic form), and host dataclass instances passed in and out
- List, dict and set comprehensions
- `try` / `except` / `else` / `finally`, `raise ... from ...`
- `for`, `while`, `if` / `elif` / `else`, `break`, `continue`, `pass`, `assert`, `global`,
  `nonlocal`, `return`
- `with` statements, for files and for classes implementing `__enter__` / `__exit__`
- f-strings, including `=`, `!r` / `!s` / `!a` and format specs
- `async` / `await`, and `asyncio.run` / `asyncio.gather`
- `import x`, `import x.y`, `from x import y, z as w`
- Starred unpacking everywhere CPython allows it

**Rejected at parse time**, with `NotImplementedError` before any code runs:

- Class inheritance and metaclasses (`class Foo(Bar):`), and therefore `super()`
- Decorators on methods — so no `@classmethod`, `@staticmethod`, `@property`
- `yield` / `yield from` — there are no generator functions. Generator *expressions*
  parse, but currently materialise to a `list`
- `match` statements
- `del`, both `del x` and `del d[k]`
- `try*` / `except*` exception groups
- PEP 695 `type` aliases
- `async with`, `async for` and async comprehensions
- Wildcard imports (`from m import *`)
- Complex literals (`1j`) and t-strings

**Missing in other ways:**

- User-defined exception classes. The built-in exception types are a fixed set, and
  without inheritance you cannot add to it.
- Function attributes. `fn.__name__`, `fn.__doc__` and friends raise `AttributeError`, and
  new attributes cannot be set — so `functools.wraps`-style metadata copying and
  registries keyed on `fn.__name__` have no equivalent.
- `eval`, `exec`, `compile`, `globals`, `locals`, `__import__`.
- Third-party packages. There is no `sys.path` and no site-packages.

## Standard library

Thirteen modules are importable. Anything else raises `ModuleNotFoundError`.

| Module | Divergences |
| --- | --- |
| `asyncio` | [asyncio.md](https://github.com/pydantic/monty/blob/main/limitations/asyncio.md) |
| `collections` | [collections.md](https://github.com/pydantic/monty/blob/main/limitations/collections.md) |
| `dataclasses` | [dataclasses.md](https://github.com/pydantic/monty/blob/main/limitations/dataclasses.md) |
| `datetime` | [datetime.md](https://github.com/pydantic/monty/blob/main/limitations/datetime.md) |
| `itertools` | [itertools.md](https://github.com/pydantic/monty/blob/main/limitations/itertools.md) |
| `json` | [json.md](https://github.com/pydantic/monty/blob/main/limitations/json.md) |
| `math` | [math.md](https://github.com/pydantic/monty/blob/main/limitations/math.md) |
| `os` | [os.md](https://github.com/pydantic/monty/blob/main/limitations/os.md) |
| `pathlib` | [pathlib.md](https://github.com/pydantic/monty/blob/main/limitations/pathlib.md) |
| `re` | [re.md](https://github.com/pydantic/monty/blob/main/limitations/re.md) |
| `sys` | [sys.md](https://github.com/pydantic/monty/blob/main/limitations/sys.md) |
| `typing` | [typing.md](https://github.com/pydantic/monty/blob/main/limitations/typing.md) |
| `unicodedata` | [unicodedata.md](https://github.com/pydantic/monty/blob/main/limitations/unicodedata.md) |

Each covers only part of its CPython surface — often a small part. The absent names are
missing from the module namespace rather than stubbed, so they fail type checking as well
as raising `AttributeError` at runtime.

Notably absent: `functools`, `enum`, `contextlib`, `random`, `time`, `io`, `copy`,
`string`, `struct`, `operator`, `inspect`, `logging`, `traceback`, `base64`, `hashlib`,
`uuid`, `urllib`. Some of those are absent by design — `socket`, `subprocess`,
`multiprocessing`, `threading` and `ctypes` would breach the sandbox — and others are
simply not implemented yet.

The authoritative list is
[`limitations/modules.md`](https://github.com/pydantic/monty/blob/main/limitations/modules.md).

## Things that work but not quite like CPython

A few divergences are worth knowing up front because they change how code behaves rather
than whether it runs. Each links to the `limitations/` file that owns it, which is where
the full account lives:

- **`assert` failures get pytest-style messages.** `assert 2 == 5` raises
  `AssertionError: assert 2 == 5`, not CPython's empty `AssertionError`. Turn it off with
  `assert_message_annotations=False` on `checkout()`
  ([assert.md](https://github.com/pydantic/monty/blob/main/limitations/assert.md)).
- **`enumerate`, `zip`, `map`, `filter` and `reversed` are eager**, not lazy. So
  `map(f, itertools.count())` runs until a resource limit trips
  ([builtins.md](https://github.com/pydantic/monty/blob/main/limitations/builtins.md)).
- **`re` is backed by Rust's `fancy-regex`**, not CPython's engine: no `bytes` patterns,
  no `VERBOSE` flag, and some error messages differ
  ([re.md](https://github.com/pydantic/monty/blob/main/limitations/re.md)).
- **There is no event loop inside the sandbox.** `async` / `await` work, and `asyncio`
  exposes exactly two functions: `run` and `gather`, the latter running host calls
  concurrently. `create_task`, `sleep` and everything else do not exist
  ([asyncio.md](https://github.com/pydantic/monty/blob/main/limitations/asyncio.md)).
- **`str.format()` and `%`-formatting are not implemented.** Use f-strings
  ([format.md](https://github.com/pydantic/monty/blob/main/limitations/format.md)).
- **Only UTF-8, ASCII, UTF-16 and UTF-32 codecs exist.** `latin-1` and friends raise
  `LookupError`
  ([encoding.md](https://github.com/pydantic/monty/blob/main/limitations/encoding.md)).

## How `limitations/` works

Every pull request that adds, changes or removes user-visible behaviour must land or
update a document in
[`limitations/`](https://github.com/pydantic/monty/tree/main/limitations). One file per
builtin, module or construct, structured around what a Python user would actually try:
arguments that are rejected or ignored, attributes that raise `AttributeError`, behaviour
that differs even where the API exists, and error types or messages that differ.

The rule is deliberately strict — a divergence that is not written down is one future
readers will assume does not exist — and reviewers reject PRs that change behaviour
without updating it.

These docs do not duplicate that content. When you need to know exactly how a feature
diverges, go to the file. When you need to know the shape of what Monty implements, stay
here.

## Reporting a gap

If you hit something that is neither in the subset nor in `limitations/`, that is a bug in
the documentation as much as in the code. Please
[open an issue](https://github.com/pydantic/monty/issues).
