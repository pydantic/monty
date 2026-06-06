# Python language / parser

Monty parses Python source with Ruff's parser but rejects several constructs
at parse time. Anything listed below raises `NotImplementedError` (prefixed
with "The monty syntax parser does not yet support ") at compile time, before
any code runs.

## Statements rejected at parse time

- **`class` definitions** — bare `class Foo: ...` is not supported. There
  is no in-sandbox class factory: `@dataclass`, `typing.NamedTuple`, and
  `collections.namedtuple` are all unavailable inside the sandbox (and
  `collections` is not importable). Host-supplied dataclass / namedtuple
  values can be passed in and used; use a plain function or a host-defined
  type for new structured data. See [classes.md](classes.md).
- **`async with` statements** — not yet supported
- **`yield` / `yield from` expressions** — no generator functions. Generator
  *expressions* (`(x for x in ...)`) parse but currently materialize to a
  `list` rather than a lazy iterator (this is a known temporary divergence;
  see `iter__generator_expr_type.py`).
- **`match` statements** — structural pattern matching is not supported.
- **`del` statements** — neither `del x` nor `del d[k]` parse.
- **`try*` / `except*` exception groups** — PEP 654 syntax rejected.
- **`type` aliases** (PEP 695 `type Foo = int`).
- **`async for` loops** and **async comprehensions**.
- **Wildcard imports** (`from m import *`) — raises `ImportError:
  "Wildcard imports (\`from ... import *\`) are not supported"`.

## Expressions rejected at parse time

- **Starred expressions** in expression position (e.g. `[*xs, *ys]`,
  `f(*args)`). Function calls with `*args` unpacking are not supported.
- **Multiple `**kwargs` unpacking** in a single call (`f(**a, **b)`).
- **Complex number literals** (`1j`, `2+3j`).
- **Template strings (t-strings)** — PEP 750.
- **Walrus operator** (`:=`) — also rejected.

## Source nesting depth

- AST nesting is capped at 200 levels (30 in debug builds); exceeding it raises `SyntaxError: Source is too deeply nested`.
- The budget is shared across every nesting-producing construct (parens, calls, subscripts, attribute chains, operators, comprehensions, control-flow blocks, `with`, etc.), including the synthetic nesting from a flat multi-item `with` — see with.md.
- The message differs from CPython, which uses construct-specific wording (`too many nested parentheses`, `too many statically nested blocks`, …).

## Imports

- Only the bundled stdlib modules listed in [modules.md](modules.md) can be
  imported. Importing anything else raises `ModuleNotFoundError`.
- Relative imports (`from . import x`) raise `ImportError: "attempted
  relative import with no known parent package"` — there is no package
  system.
- `__import__` is not defined.

## Module-level dunder variables

Monty has no module object and no `globals()` dict, but it exposes a fixed set
of module-level dunders so common idioms (e.g. `if __name__ == '__main__':`)
work. They are resolved on read; there is no real namespace entry behind them.

| Name              | Monty value  | CPython (script run)         |
| ----------------- | ------------ | ---------------------------- |
| `__name__`        | `'__main__'` | `'__main__'`                 |
| `__debug__`       | `True`       | `True`                       |
| `__doc__`         | `None`       | `None` or docstring `str`    |
| `__spec__`        | `None`       | `None`                       |
| `__package__`     | `None`       | `None`                       |
| `__annotations__` | empty `dict` | `NameError` (no annotations) |

In Monty `__doc__` is always `None` — module docstrings are never extracted —
and `__annotations__` is always an empty `dict` because module-level annotations
are not stored (see [typing.md](typing.md)); CPython 3.14 instead raises
`NameError` when a module has no annotations (PEP 649).

These names are **read-only**: assigning one at module or global scope (including
via `global __name__` inside a function, and augmented assignment like
`__name__ += ...`) is rejected at compile time with
`NotImplementedError: cannot reassign read-only module attribute '<name>'`.
CPython instead *allows* rebinding most of them (it is how you set a module
docstring), and rejects only `__debug__` — with a `SyntaxError`.

Binding one of these names as a **function local** is allowed (it is an
ordinary local in a separate namespace), matching CPython — except `__debug__`,
which CPython rejects everywhere with `SyntaxError` but Monty permits as a local.

Other module dunders CPython defines (`__loader__`, `__file__`, `__builtins__`,
`__cached__`, `__dict__`) are not exposed; reading them falls through to the host
name lookup and ultimately raises `NameError` if unresolved. `__loader__` is
omitted because CPython always binds it to a loader *object* (never `None`), so
exposing `None` would diverge on type — and a real loader is neither available
nor safe to surface in the sandbox. `__file__` is omitted so no host path can
leak into the sandbox.

## What *does* work

- Functions (`def`, `async def`), nested functions, closures, decorators.
- List / dict / set comprehensions (generator comprehensions degrade to
  lists — see above).
- `try` / `except` / `else` / `finally`, `raise ... from ...`.
- `for` / `while` / `if` / `elif` / `else`, `break`, `continue`, `pass`,
  `assert`, `global`, `nonlocal`, `return`.
- `import x`, `import x.y`, `from x import y, z as w`.
- f-strings including `=` debug specifier, `!r`/`!s`/`!a` conversions, and
  format specs.
