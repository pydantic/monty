# `iter()` and iterators

- A user `__next__` method can suspend (call an external or OS function) when driven by a `for` loop, a comprehension,
    `list()`, `tuple()`, `sum()`, `next()` (including its default form) or `functools.reduce()`.
    Other consumers run it synchronously: `set()`, `dict()`, `sorted()`, `min()`/`max()`, `any()`/`all()`, `str.join()`,
    unpacking, `in`, and native iterator adaptors such as `map`, `filter`, `zip` and `enumerate`.
    There an external or OS call raises `NotImplementedError` inside the method, where it can be caught.
    The same happens to `list()`, `tuple()` and `sum()` when they are themselves called from a callback that cannot
    suspend, such as `sorted(key=...)`.

- A user `__iter__` method can suspend only through a direct `iter()` call.

- `iter(callable, sentinel)` runs `callable` synchronously, so one that calls an external/OS function cannot suspend and
    raises `NotImplementedError`. Same limitation as `map`/`filter`/`sorted(key=...)`.

- `iter(callable, sentinel)` compares `result == sentinel`, where CPython compares `sentinel == result`; only observable
    if the two sides have asymmetric `__eq__`.

- A `StopIteration` raised by `callable` propagates; CPython treats it as clean exhaustion and stops iterating.

- The vendored type stub is upstream typeshed's verbatim, so `-t` accepts `iter(obj)` for an object with only
    `__getitem__`; Monty has no `__getitem__` iteration fallback and raises `TypeError` at runtime.

- `-t` accepts `for x in obj` (though not `a, b = obj`) for a class that opts out of iteration with `__iter__ = None`,
    which raises `TypeError` at runtime as it does in CPython.

- Built-in iterators do not expose their dunders as attributes: `hasattr(iter([1]), '__iter__')` is `False`, where
    CPython reports `True`. Iteration itself works; only attribute lookup of the dunder differs. This covers every
    built-in iterator, including the `itertools` adaptors.

- Iterator-specific attributes such as `__length_hint__` are not exposed.
