# Built-in functions

Monty implements a deliberate subset of CPython's builtins. Referencing any
name not listed here raises `NameError` at runtime — there is no fallback to
a host Python.

## Implemented builtin functions

`abs`, `all`, `any`, `bin`, `chr`, `divmod`, `enumerate`, `filter`,
`getattr`, `hasattr`, `hash`, `hex`, `id`, `isinstance`, `len`, `map`,
`max`, `min`, `next`, `oct`, `open`, `ord`, `pow`, `print`, `repr`,
`reversed`, `round`, `setattr`, `sorted`, `sum`, `type`, `zip`.

## Implemented type constructors (also builtins)

`bool`, `bytes`, `dict`, `float`, `frozenset`, `int`, `list`, `range`,
`set`, `slice`, `str`, `tuple`. Exception classes (`ValueError`,
`TypeError`, etc.) are also names in the builtin namespace.

## Builtins that are NOT implemented

These raise `NameError`:

- **Code execution**: `eval`, `exec`, `compile`, `__import__`. Deliberate —
  sandboxed code must not be able to compile new code at runtime.
- **Namespace introspection**: `globals`, `locals`, `vars`, `dir`.
- **Interactive**: `input`, `breakpoint`, `help`.
- **Decorators / descriptors**: `classmethod`, `staticmethod`, `property`,
  `super`. (`@property` on functions is not recognized; use a method.)
- **Construction / coercion**: `bytearray`, `complex`, `memoryview`,
  `object`, `iter`, `format`, `ascii`.
- **Other**: `callable`, `delattr`, `issubclass`, `aiter`, `anext`.

`super()` is the biggest practical omission — combined with the lack of
`class` statements (see [language.md](language.md)) there is no inheritance
mechanism beyond dataclass field inheritance.

## Behavioural divergences

- **Arity-error wording for some str/bytes methods** — a handful of
  keyword-accepting methods (e.g. `str.split`, `str.rsplit` and the `bytes`
  equivalents) report too-many-arguments as `split expected at most 2
  arguments, got 3`, where CPython 3.14's Argument Clinic pre-counts
  positionals *plus* kwargs and says `split() takes at most 2 arguments (3
  given)`. Methods audited against CPython (`encode`, `decode`,
  `expandtabs`, `splitlines`, `replace`, …) already match; the remainder
  need a per-function `at_most_total` audit.
- **`getattr(obj, name)`** — if the resolved attribute would be an async
  coroutine, external function, or OS call, raises `TypeError:
  "getattr(): attribute is not a simple value"` rather than returning a
  bound method object. Use direct attribute access (`obj.name(...)`) for
  these.
- **`isinstance(obj, T)`** — `T` must be a built-in type (`int`, `str`,
  `list`, ...), a built-in exception class, a sandbox-defined class (see
  [classes.md](classes.md)), or a tuple of those. Passing a host-supplied
  dataclass / namedtuple as the second argument raises `TypeError`.
- **`pow(base, exp, mod)`** — three-argument form requires all integers and
  rejects negative exponents with `ValueError`. Exponents greater than
  `u32::MAX` raise `OverflowError` (see [resource_limits.md](resource_limits.md)).
- **`sorted(iterable, *, key=None, reverse=False)`** — `key` and `reverse`
  must be passed by keyword; positional forms raise `TypeError`.
- **`round(x, n)`** — `n` must be an integer; CPython accepts and truncates
  floats.
- **`print`** — writes via the host print callback. `file=`, `flush=` are
  not honoured; `sep=` and `end=` are.
- **`id(f)` / `hash(f)` / `f is g` / `f == g` for host-supplied callables**
  — host functions passed in as inputs (`MontyObject::Function`) lose their
  host object identity at the sandbox boundary. Inside Monty they are
  compared by `__name__` alone: two distinct host callables with the same
  `__name__` satisfy `a is b`, `a == b`, `id(a) == id(b)`, and
  `hash(a) == hash(b)`. In CPython those would be four separate objects
  and all four checks would return `False` / unequal. Conversely, the same
  callable passed in twice is guaranteed identical regardless of whether
  its name was interned in source. This applies only to external functions
  — `def`-defined functions inside the sandbox retain per-definition
  identity.
- **Type objects across the host boundary** — a `type` object (a class, not an
  instance) round-trips in both directions.
  - *Sandbox → host* (external/OS-call argument, or a `.run()` return value): the
    type is reconstructed as the corresponding host class. Genuine builtins
    (`int`, `str`, `type`, `bytes`, `list`, `dict`, `property`, …) resolve to the
    real builtin; Monty's modeled stdlib types map to their host stdlib class:
    `datetime`/`date`/`timedelta`/`timezone` → `datetime.*`,
    `re.Pattern`/`re.Match` → `re.*`, the binary/text file types → `io.*`. The
    `pathlib.Path` class maps to `pathlib.PurePosixPath` (consistent with how Path
    *instances* round-trip, and instantiable on every host OS). A type with no
    faithful host class (e.g. an internal function or cell type) cannot be
    reconstructed and surfaces as an `AttributeError` from the host call.
  - *Host → sandbox* (input, or an external-call return value): the same recognized
    builtins and modeled stdlib types are preserved as type objects, so
    `isinstance(x, the_type)` works inside the sandbox. Recognition is by
    type-object **identity**, not class name/module, so a class that forges
    `__name__`/`__module__` to impersonate a builtin is *not* treated as one. Every
    `pathlib` path class collapses to `PurePosixPath` (it re-emerges as
    `PurePosixPath`). A host class Monty does **not** model (e.g. a user-defined
    class) is not preserved as a type — it degrades to a callable, appearing inside
    the sandbox as a `function` rather than a `type`.

- **Iterator delegation** — consuming an existing iterator (`for x in it`,
  `list(it)`) wraps it in a delegating iterator that shares its position, where
  CPython iterates it directly. Not observable from Python: `iter(it)` returns
  `it` unchanged, so no chain deeper than 1 can be built. Two `RuntimeError`s
  guard malformed snapshot data, which can craft what Python cannot —
  `iterator delegation nested too deeply` past 1000 links, and
  `iterator delegates to a non-iterator` for a link pointing elsewhere. CPython
  has neither.
- **`reversed(x)`** — the `TypeError` for a non-reversible argument names Monty's
  single `iterator` type rather than CPython's specific one, e.g.
  `'iterator' object is not reversible` where CPython says `'list_iterator'`.
