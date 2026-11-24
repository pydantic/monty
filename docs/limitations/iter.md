# `iter()` and iterators

- A user `__next__` method can suspend (call an external or OS function) when driven by `for`, a comprehension,
    `list()`, `tuple()`, `sum()`, a direct `next()` (including its default form) or `functools.reduce()`.
    A user `__iter__` method can suspend under `for`, comprehensions and a direct `iter()`.

- Other consumers run these methods synchronously: `set()`, `dict()`, `sorted()`, `min()`/`max()`, `any()`/`all()`,
    `str.join()`, unpacking, `in` and the `itertools` adaptors.
    So do `list()`, `tuple()` and `sum()` in three cases: in `__iter__`, when the iterator is a `map` or `filter`
    object or another native adaptor, and when they are themselves called from a callback that cannot suspend, such
    as `sorted(key=...)`.
    Callbacks reached through `map` and `filter` suspend only when a `for`, a comprehension or `next()` advances them.
    Elsewhere an external or OS call raises `NotImplementedError` inside the method or callback, where it can be
    caught.

- `iter(callable, sentinel)` runs `callable` synchronously, so one that calls an external/OS function cannot suspend and
    raises `NotImplementedError`, like `sorted(key=...)`.

- `iter(callable, sentinel)` compares `result == sentinel`, where CPython compares `sentinel == result`; only observable
    if the two sides have asymmetric `__eq__`.

- The vendored type stub is upstream typeshed's verbatim, so `-t` accepts `iter(obj)` for an object with only
    `__getitem__`; Monty has no `__getitem__` iteration fallback and raises `TypeError` at runtime.

- `-t` accepts `for x in obj` (though not `a, b = obj`) for a class that opts out of iteration with `__iter__ = None`,
    which raises `TypeError` at runtime as it does in CPython.

- Built-in iterators do not expose their dunders as attributes: `hasattr(iter([1]), '__iter__')` is `False`, where
    CPython reports `True`. Iteration itself works; only attribute lookup of the dunder differs. This covers every
    built-in iterator, including `map`, `filter` and the `itertools` adaptors.

- Iterator-specific attributes such as `__length_hint__` are not exposed.
