# Python language / parser

Monty parses Python source with Ruff's parser but rejects several constructs
at parse time. Anything listed below raises `NotImplementedError` (prefixed
with "The monty syntax parser does not yet support ") at compile time, before
any code runs.

## Statements rejected at parse time

- **`class` definitions** — simple classes are supported (instance methods,
    `__init__`/`__repr__`/`__str__`, class variables of arbitrary expressions).
    Rejected at parse time: base classes / metaclasses (`class Foo(Bar):`) and
    class-body statements other than `def`, a simple `name [: T] = <expr>`
    assignment, `pass`, or a docstring. There is no inheritance and no general
    dunder protocol. See [classes.md](classes.md).
- **Decorators** (`@deco`) — supported on classes and on top-level or nested
    `def`/`async def`, taking any callable in scope, evaluated in the enclosing
    scope and applied bottom-up. Rejected at parse time on **methods**, so
    `@classmethod`, `@staticmethod`, `@property` and any decorator on a `def`
    inside a class body are unavailable. See [classes.md](classes.md).
- **`async with` statements** — not yet supported.
- **`yield` / `yield from` expressions** — no generator functions. Generator
    *expressions* (`(x for x in ...)`) parse but currently materialize to a
    `list` rather than a lazy iterator, a known temporary divergence; see
    `iter__generator_expr_type.py`.
- **`match` statements** — structural pattern matching is not supported.
- **`del` statements** — neither `del x` nor `del d[k]` parse.
- **`try*` / `except*` exception groups** — PEP 654 syntax rejected.
- **`type` aliases** (PEP 695 `type Foo = int`).
- **`async for` loops** and **async comprehensions**.
- **Wildcard imports** (`from m import *`) — raises
    `` NotImplementedError: "Wildcard imports (`from ... import *`) are not supported" ``.

## Expressions rejected at parse time

- **Complex number literals** (`1j`, `2+3j`) —
    `NotImplementedError: The monty syntax parser does not yet support complex constants`.
- **Template strings (t-strings)** — PEP 750.

## Starred unpacking

Anything Monty can iterate may follow a `*`, matching CPython: `[*xs]`,
`(*xs,)`, `{*xs}`, `f(*xs)`, `a, b = xs` and `a, *b = xs` all accept whatever
`list(xs)` accepts.

One message divergence: passing a non-iterable to a call, `f(*1)`, reports
`TypeError: Value after * must be an iterable, not int`, the same wording as a
list literal. CPython instead names the callable by its module-qualified
`__qualname__`: `__main__.f() argument after * must be an iterable, not int`,
and correspondingly `__main__.C.m()`, `__main__.<lambda>()` or
`__main__.outer.<locals>.inner()`. Monty has neither function `__qualname__`
nor module-qualified names (see the class-name note in [collections.md](collections.md)), so
it reports the generic form. Every other unpacking form matches CPython
exactly.

## Source nesting depth

- AST nesting is capped at 200 levels (30 in debug builds); exceeding it raises
    `SyntaxError: Source is too deeply nested`.
- The budget is shared across every nesting-producing construct (parens, calls, subscripts, attribute chains, operators,
    comprehensions, control-flow blocks, `with`, etc.), including the synthetic nesting from a flat multi-item `with`;
    see [with.md](with.md).
- The message differs from CPython, which uses construct-specific wording (`too many nested parentheses`,
    `too many statically nested blocks`, …).
- Class-body annotations count against the budget even though they are stringized rather than evaluated (see
    [typing.md](typing.md)), as do class-variable values and method parameter defaults; all three are walked before
    being parsed. CPython imposes no comparable limit on a stringized annotation.

## Imports

- Only the bundled stdlib modules listed in [modules.md](modules.md) can be
    imported. Importing anything else raises `ModuleNotFoundError`.
- Relative imports (`from . import x`) raise `ImportError: "attempted relative import with no known parent package"`;
    there is no package
    system.
- `__import__` is not defined.

## `__future__` imports

`from __future__ import ...` is a compiler directive, not a real import: it
binds nothing and is accepted as a no-op. Of CPython's ten features, eight
became mandatory in Python 3.7 or earlier and so are inert there too, and
`annotations` is a no-op here because Monty already stringizes annotations
(see [typing.md](typing.md)). Divergences:

- **`barry_as_FLUFL`** (PEP 401) raises
    `NotImplementedError: "The monty syntax parser does not yet support the 'barry_as_FLUFL' future feature"`.
    CPython accepts it, making `<>` the inequality operator and `!=` a
    `SyntaxError`; Monty parses neither differently, so the import is rejected
    rather than silently ignored.
- **Aliasing is rejected.** `from __future__ import annotations as ann` raises
    `` NotImplementedError: "The monty syntax parser does not yet support aliasing a `__future__` feature" ``.
    CPython binds `ann` to a `__future__._Feature` object; a no-op would bind nothing and surface as a `NameError`
    far from the import, so it is rejected at the import instead.
- **Position is not enforced.** CPython requires `__future__` imports to
    precede all other statements (`SyntaxError: "from __future__ imports must occur at the beginning of the file"`);
    Monty accepts them anywhere.
- `import __future__` (as opposed to `from __future__ import ...`) raises
    `ModuleNotFoundError`; there is no `__future__` module object.

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

In Monty `__doc__` is always `None`, since module docstrings are never
extracted, and `__annotations__` is always an empty `dict` because
module-level annotations are not stored (see [typing.md](typing.md)); CPython 3.14
instead raises `NameError` when a module has no annotations (PEP 649).

These names are **read-only**: assigning one at module or global scope (including
via `global __name__` inside a function, and augmented assignment like
`__name__ += ...`) is rejected at compile time with
`NotImplementedError: cannot reassign read-only module attribute '<name>'`.
CPython instead *allows* rebinding most of them (it is how you set a module
docstring), and rejects only `__debug__`, with a `SyntaxError`.

Binding one of these names as a **function local** is allowed (it is an
ordinary local in a separate namespace), matching CPython, except `__debug__`,
which CPython rejects everywhere with `SyntaxError` but Monty permits as a
local.

Other module dunders CPython defines (`__loader__`, `__file__`, `__builtins__`,
`__cached__`, `__dict__`) are not exposed; reading them falls through to the host
name lookup and ultimately raises `NameError` if unresolved. `__loader__` is
omitted because CPython always binds it to a loader *object* (never `None`), so
exposing `None` would diverge on type, and a real loader is neither available
nor safe to surface in the sandbox. `__file__` is omitted so no host path can
leak into the sandbox.

## Function objects

A function exposes **no** attributes: `__name__`, `__doc__`, `__qualname__` and
`__module__` all raise `AttributeError: 'function' object has no attribute '<name>'`, and new ones cannot be set —
`fn.tag = True` raises
`AttributeError: 'function' object has no attribute 'tag' and no __dict__ for setting new attributes`. CPython supports
all of these.

This bounds what a decorator can do: it can call, wrap, store or replace the
function it receives, but cannot ask the function about itself, so
`functools.wraps`-style metadata copying, registries keyed by `fn.__name__`, and
attribute tagging for later discovery all have no equivalent.

## Ordering comparisons

`<`, `<=`, `>`, `>=` on operands with no defined ordering raise
`TypeError: '<' not supported between instances of '{a}' and '{b}'`, matching
CPython (int vs str, `None` vs `None`, user-class instances without comparison
dunders, etc.). Lists and tuples order lexicographically as in CPython. A `NaN`
operand is *unordered* rather than incomparable, so `float('nan') < 1` (and
every operator/direction, including two NaNs) returns `False` without raising,
also matching CPython, and likewise inside `sorted`/`min`/`max`.

One message divergence: when a **list or tuple** compares unequal only because
an *inner element* pair is unorderable (e.g. `(1, 2) < (1, 'a')`), Monty names
the outer container types (`'tuple' and 'tuple'`) where CPython names the inner
element pair (`'int' and 'str'`). Both raise `TypeError`; only the message text
differs.

One value divergence: whenever CPython compares elements *inside* a container it
shortcuts equality by **object identity** (`x is x` ⇒ equal) before falling back
to `==`. Monty has no object identity for immediate floats, so it asks `==`, and
a `NaN` is never equal to itself. Every container operation built on element
comparison therefore differs when the *same* `NaN` object appears on both sides:

| with `x = float('nan')`      | CPython | Monty        |
| ---------------------------- | ------- | ------------ |
| `(x,) == (x,)`, `[x] == [x]` | `True`  | `False`      |
| `x in [x]`                   | `True`  | `False`      |
| `[x].count(x)`               | `1`     | `0`          |
| `[x].index(x)`               | `0`     | `ValueError` |
| `[1, x] < [1, x, 3]`         | `True`  | `False`      |

`NaN` is the only practical way to reach this, being the one built-in value
unequal to itself. *Distinct* `NaN` objects agree on both
(`[1, float('nan')] < [1, float('nan'), 3]` is `False` either way), as does a
direct `x == x` (`False` on both). Named tuples inherit all of this from `tuple`.

## What *does* work

- Functions (`def`, `async def`), nested functions, closures, and decorators on
    them, but not on methods (see above).
- List / dict / set comprehensions; generator comprehensions degrade to
    lists (see above).
- `try` / `except` / `else` / `finally`, `raise ... from ...`.
- `for` / `while` / `if` / `elif` / `else`, `break`, `continue`, `pass`,
    `assert`, `global`, `nonlocal`, `return`.
- `import x`, `import x.y`, `from x import y, z as w`.
- f-strings including `=` debug specifier, `!r`/`!s`/`!a` conversions, and
    format specs.
