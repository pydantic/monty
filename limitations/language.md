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
- **`with` / `async with` statements** — no context manager protocol. This
  means no `with open(...) as f:` (call `f.close()` explicitly). See
  [open.md](open.md).
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
