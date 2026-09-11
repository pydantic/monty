# Built-in functions

Monty implements a subset of CPython's builtins. Referencing any name not
listed here raises `NameError` at runtime; there is no fallback to a host
Python.

## Implemented builtin functions

`abs`, `all`, `any`, `bin`, `chr`, `divmod`, `enumerate`, `filter`,
`format`, `getattr`, `hasattr`, `hash`, `hex`, `id`, `isinstance`, `iter`, `len`,
`map`, `max`, `min`, `next`, `oct`, `open`, `ord`, `pow`, `print`, `repr`,
`reversed`, `round`, `setattr`, `sorted`, `sum`, `type`, `zip`.

## Implemented type constructors (also builtins)

`bool`, `bytes`, `dict`, `float`, `frozenset`, `int`, `list`, `range`,
`set`, `slice`, `str`, `tuple`. Exception classes (`ValueError`,
`TypeError`, etc.) are also names in the builtin namespace.

## Builtins that are NOT implemented

These raise `NameError`:

- **Code execution**: `eval`, `exec`, `compile`, `__import__`. Deliberate:
    sandboxed code must not be able to compile new code at runtime.
- **Namespace introspection**: `globals`, `locals`, `vars`, `dir`.
- **Interactive**: `input`, `breakpoint`, `help`.
- **Decorators / descriptors**: `classmethod`, `staticmethod`, `property`,
    `super`. (`@property` on functions is not recognized; use a method.)
- **Construction / coercion**: `bytearray`, `complex`, `memoryview`,
    `object`, `ascii`.
- **Other**: `callable`, `delattr`, `issubclass`, `aiter`, `anext`.

`super()` is the biggest practical omission: with no class inheritance either
(see [classes.md](classes.md)), there is no inheritance mechanism at all.

## Behavioural divergences

- **`repr` of a dict being mutated by its own elements** — Monty iterates the
    live entries like CPython, but deletion compacts Monty's dense entry storage
    where CPython leaves a tombstone in place: a key deleted from inside a user
    `__repr__` running *during that dict's repr* shifts later entries down, so
    the entry after the deleted one can be skipped from the output where CPython
    would still print it. Insertions during repr match CPython (appended and
    printed), as do list (live length, mid-repr pops truncate / appends extend),
    `set`, `collections.deque` and `collections.Counter` (all snapshot, like
    CPython).
- **dict/set lookups under a mutating `__eq__`** — like CPython, a lookup
    (`in`, `d[k]`, `set.remove`, …) whose user `__eq__` mutates the container
    keeps probing rather than raising, and a mutation that only *adds* colliding
    keys never makes it repeat a comparison. Monty re-reads the colliding
    candidates after each comparison where CPython walks the live probe chain, so
    an `__eq__` that moves the entry being compared restarts the probe and
    re-compares the other candidates from scratch — extra `__eq__` calls CPython
    would not make (CPython restarts and re-compares too, but only after a resize
    or when that entry's own slot changed). An `__eq__` that adds a colliding key
    on *every* comparison never finishes in either engine; under `max_duration`
    Monty raises `TimeoutError`. No mutation pattern can panic or corrupt either
    engine.
- **Set algebra under a mutating `__eq__`** — `-`, `&`, `^` and their method
    forms walk one of the two sets while user `__eq__` code can run. Monty
    raises `RuntimeError: Set changed size during iteration` if that code adds
    or removes an element from the set being walked, where CPython carries on
    over its live table and returns a result. The operand Monty is not walking
    is snapshotted before the operation begins, so mutating that one is never
    observed at all — `s | t` and `t.update(s)` see all of `s`'s original
    elements even when `__eq__` clears `s` partway through, where CPython's
    merge stops early.
- **Dict-view set operators re-hash the view's own keys** — `d.keys() - s`,
    `|`, `^`, `isdisjoint` and the reflected forms collect those keys through a
    live, resize-checked walk that calls `__hash__` on each one. CPython probes
    with each key's stored hash and mostly does not call it at all. So a
    `__hash__` — or a colliding `__eq__` — that resizes the dict raises
    `RuntimeError: dictionary changed size during iteration` in Monty where
    CPython completes, as in `d.keys() - s`; CPython raises too wherever its own
    walk observes the change, as in `s - d.keys()` and `d.items() - s`.
    `d.keys() & s` is the exception on both engines, walking the other operand
    and probing the live dict instead, so the view's keys are never re-hashed.
    The operand that is *not* the view is snapshotted before the operation
    begins, so mutating that one is never observed. Absent mutation every result
    agrees. Set-to-set operators do not re-hash at all, and dict-view equality
    (`d.keys() == s`) raises exactly where CPython does.
- **`enumerate`, `zip`, `map`, `filter` and `reversed` are eager, not lazy** —
    each drains its source and returns a `list`, so `type(enumerate(x)).__name__`
    is `'list'` rather than `'enumerate'`. Observable several ways: a
    side-effecting callable runs for every item at the call itself rather than as
    the result is consumed; the whole result is held in memory at once, so an
    infinite iterator (e.g. `map(f, itertools.count())`) never returns and runs
    until a resource limit trips; the result can be indexed and re-iterated, which
    CPython forbids; and mutating the source from inside the loop body is never
    observed, so containers that detect mutation during iteration (`dict`, `set`,
    `collections.deque`) will not raise when looped over via one of these. `zip`
    and multi-iterable `map` stop at the shortest input, so pairing an infinite
    iterable with a finite or empty one stays bounded. A plain `for x in container` is lazy and does detect mutation.
    See [itertools.md](itertools.md).
- **Arity-error wording for some str/bytes methods** — a handful of
    keyword-accepting methods (e.g. `str.split`, `str.rsplit` and the `bytes`
    equivalents) report too-many-arguments as `split expected at most 2 arguments, got 3`, where CPython 3.14's Argument
    Clinic pre-counts
    positionals *plus* kwargs and says `split() takes at most 2 arguments (3 given)`. Methods audited against CPython
    (`encode`, `decode`,
    `expandtabs`, `splitlines`, `replace`, …) already match; the remainder
    need a per-function `at_most_total` audit.
- **`getattr(obj, name)`** — if the resolved attribute would be an async
    coroutine, external function, or OS call, raises `TypeError: "getattr(): attribute is not a simple value"` rather
    than returning a
    bound method object. Use direct attribute access (`obj.name(...)`) for
    these.
- **`int(x, base=10)`** — string/bytes parsing accepts ASCII digits only;
    CPython also accepts non-ASCII Unicode decimal digits (`int('١٢')` == 12),
    which Monty rejects with `invalid literal for int() with base 10`.
- **`bytes(source)`** — an iterable of ints is not supported: CPython's
    `bytes([65, 66])` == `b'AB'`, Monty raises `TypeError: cannot convert 'list' object to bytes`. The int /
    str-with-encoding / bytes source forms
    all work. A count above `i64` (`bytes(2**70)`) raises the same shape of `TypeError`
    (`cannot convert 'int' object to bytes`), not CPython's
    `OverflowError: cannot fit 'int' into an index-sized integer`.
- **`isinstance(obj, T)`** — `T` must be a built-in type (`int`, `str`,
    `list`, ...), a built-in exception class, a sandbox-defined class (see
    [classes.md](classes.md)), or a tuple of those. Passing a host-supplied
    dataclass / namedtuple as the second argument raises `TypeError`.
- **`iter()`** — see [iter.md](iter.md) for iterator and `iter(callable, sentinel)` divergences.
- **`pow(base, exp, mod)`** — the three-argument form requires all integers and
    rejects negative exponents with `ValueError` instead of computing a modular
    inverse. Non-modular exponents whose result cannot be materialized raise
    `OverflowError` (see [resource_limits.md](resource_limits.md)).
- **`sorted(iterable, *, key=None, reverse=False)`** — `key` and `reverse`
    must be passed by keyword; positional forms raise `TypeError`.
- **`round(n, ndigits)`** — `ndigits` values outside the i64 range are
    clamped by sign. For floats this matches CPython (which clamps to
    `Py_ssize_t`); for an int `n` with a hugely negative `ndigits`, CPython
    tries to materialise `10**-ndigits` and dies with `MemoryError` where
    Monty returns `0` immediately.
- **`print`** — writes via the host print callback. `file=`, `flush=` are
    not honoured; `sep=` and `end=` are.
- **Identity of host-supplied callables** — host functions passed in as inputs
    (`MontyObject::Function`) lose their host object identity at the sandbox
    boundary. Live external functions are identified by lookup name, so distinct
    host callables with the same name share `is`, equality, `id()`, and `hash()`
    results. Once the last sandbox reference is dropped, a later conversion of
    that name may create a new function object.
- **Type objects across the host boundary** — a `type` object (a class, not an
    instance) round-trips in both directions.
    - *Sandbox → host* (external/OS-call argument, or a `.run()` return value): the
        type is reconstructed as the corresponding host class. Genuine builtins
        (`int`, `str`, `type`, `bytes`, `list`, `dict`, `property`, …) resolve to the
        real builtin; Monty's modeled stdlib types map to their host stdlib class:
        `datetime`/`date`/`timedelta`/`timezone` → `datetime.*`,
        `re.Pattern`/`re.Match` → `re.*`, the binary/text file types → `io.*`. The
        `pathlib.Path` class maps to `pathlib.PurePosixPath`, consistent with how Path
        *instances* round-trip, and instantiable on every host OS. A type with no
        faithful host class (e.g. an internal function or cell type) cannot be
        reconstructed and surfaces as an `AttributeError` from the host call.
    - *Host → sandbox* (input, or an external-call return value): the same recognized
        builtins and modeled stdlib types are preserved as type objects, so
        `isinstance(x, the_type)` works inside the sandbox. Recognition is by
        type-object **identity**, not class name/module, so a class that forges
        `__name__`/`__module__` to impersonate a builtin is *not* treated as one. Every
        `pathlib` path class collapses to `PurePosixPath` (it re-emerges as
        `PurePosixPath`). A host class Monty does **not** model (e.g. a user-defined
        class) is not preserved as a type; it degrades to a callable, appearing inside
        the sandbox as a `function` rather than a `type`.
