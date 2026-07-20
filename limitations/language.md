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
- **Decorators** (`@deco`) — supported on classes, taking any callable in scope,
  evaluated in the enclosing scope and applied bottom-up. Rejected at parse time
  on functions and methods, so `@classmethod`, `@staticmethod`, `@property` and
  any decorator on a `def` are unavailable. See [classes.md](classes.md).
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

- **Multiple `**kwargs` unpacking** in a single call (`f(**a, **b)`).
- **Complex number literals** (`1j`, `2+3j`).
- **Template strings (t-strings)** — PEP 750.
- **Walrus operator** (`:=`) — also rejected.

## Starred unpacking

Anything Monty can iterate may follow a `*`, matching CPython — `[*xs]`,
`(*xs,)`, `{*xs}`, `f(*xs)`, `a, b = xs` and `a, *b = xs` all accept whatever
`list(xs)` accepts.

One message divergence: passing a non-iterable to a call, `f(*1)`, reports
`TypeError: Value after * must be an iterable, not int` — the same wording as a
list literal. CPython instead names the callable by its module-qualified
`__qualname__`: `__main__.f() argument after * must be an iterable, not int`,
and correspondingly `__main__.C.m()`, `__main__.<lambda>()` or
`__main__.outer.<locals>.inner()`. Monty has neither function `__qualname__`
nor module-qualified names (see the class-name note in
[collections.md](collections.md)), so it reports the generic form. Every other
unpacking form matches CPython exactly.

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

## `__future__` imports

`from __future__ import ...` is a compiler directive, not a real import: it
binds nothing and is accepted as a no-op. Of CPython's ten features, eight
became mandatory in Python 3.7 or earlier and so are inert there too, and
`annotations` is a no-op here because Monty already stringizes annotations
(see [typing.md](typing.md)). Divergences:

- **`barry_as_FLUFL`** (PEP 401) raises `NotImplementedError: "The monty
  syntax parser does not yet support the 'barry_as_FLUFL' future feature"`.
  CPython accepts it, making `<>` the inequality operator and `!=` a
  `SyntaxError`; Monty parses neither differently, so the import is rejected
  rather than silently ignored.
- **Aliasing is rejected.** `from __future__ import annotations as ann` raises
  `NotImplementedError: "The monty syntax parser does not yet support aliasing
  a \`__future__\` feature"`. CPython binds `ann` to a `__future__._Feature`
  object; a no-op would bind nothing and surface as a `NameError` far from the
  import, so it is rejected at the import instead.
- **Position is not enforced.** CPython requires `__future__` imports to
  precede all other statements (`SyntaxError: "from __future__ imports must
  occur at the beginning of the file"`); Monty accepts them anywhere.
- `import __future__` (as opposed to `from __future__ import ...`) raises
  `ModuleNotFoundError` — there is no `__future__` module object.

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

## Ordering comparisons

`<`, `<=`, `>`, `>=` on operands with no defined ordering raise
`TypeError: '<' not supported between instances of '{a}' and '{b}'`, matching
CPython (int vs str, `None` vs `None`, user-class instances without comparison
dunders, etc.). Lists and tuples order lexicographically as in CPython. A `NaN`
operand is *unordered* rather than incomparable, so `float('nan') < 1` (and
every operator/direction, including two NaNs) returns `False` without raising —
also matching CPython, and likewise inside `sorted`/`min`/`max`.

One message divergence: when a **list or tuple** compares unequal only because
an *inner element* pair is unorderable (e.g. `(1, 2) < (1, 'a')`), Monty names
the outer container types (`'tuple' and 'tuple'`) where CPython names the inner
element pair (`'int' and 'str'`). Both raise `TypeError`; only the message text
differs.

One value divergence: CPython's sequence comparison shortcuts equality by
*object identity* (`x is x` ⇒ equal) before falling back to `==`, so a shared
`NaN` element in a prefix position makes the shorter sequence compare less
(`x = float('nan'); [1, x] < [1, x, 3]` is `True`). Monty has no object identity
for immediate floats, so it treats the two `NaN`s as a differing pair and yields
`False`. Distinct `NaN` objects (`[1, float('nan')] < [1, float('nan'), 3]`)
give `False` on both.

## What *does* work

- Functions (`def`, `async def`), nested functions, closures (but not
  decorators — see above).
- List / dict / set comprehensions (generator comprehensions degrade to
  lists — see above).
- `try` / `except` / `else` / `finally`, `raise ... from ...`.
- `for` / `while` / `if` / `elif` / `else`, `break`, `continue`, `pass`,
  `assert`, `global`, `nonlocal`, `return`.
- `import x`, `import x.y`, `from x import y, z as w`.
- f-strings including `=` debug specifier, `!r`/`!s`/`!a` conversions, and
  format specs.
