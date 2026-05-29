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

- **`getattr(obj, name)`** — if the resolved attribute would be an async
  coroutine, external function, or OS call, raises `TypeError:
  "getattr(): attribute is not a simple value"` rather than returning a
  bound method object. Use direct attribute access (`obj.name(...)`) for
  these.
- **`isinstance(obj, T)`** — `T` must be a built-in type (`int`, `str`,
  `list`, ...), a built-in exception class, or a tuple of those. Passing a
  user-defined dataclass / namedtuple as the second argument raises
  `TypeError`.
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
