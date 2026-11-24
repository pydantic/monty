# `iter()` and iterators

- Collection constructors and synchronous native consumers such as `sum`, `sorted` and `itertools` adaptors cannot
    suspend in user `__iter__` or `__next__` methods, or in callbacks reached through `map` and `filter`.
    An external or OS call raises `NotImplementedError` inside the method or callback, where it can be caught.
    This restriction does not apply to `for`, comprehensions, direct `iter()`/`next()`, or `functools.reduce()`.

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
