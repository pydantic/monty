# `iter()` and iterators

- `iter(callable, sentinel)` runs `callable` synchronously, so one that calls an external/OS function cannot suspend and raises `NotImplementedError` — the same limitation as `map`/`filter`/`sorted(key=...)`.
- `iter(callable, sentinel)` compares `result == sentinel`, where CPython compares `sentinel == result`; only observable if the two sides have asymmetric `__eq__`.
- A `StopIteration` raised by `callable` propagates; CPython treats it as clean exhaustion and stops iterating.
- A user instance defining `__call__` is rejected as not callable, since `__call__` is not dispatched (see [classes.md](classes.md)).
- The vendored type stub is upstream typeshed's verbatim, so `-t` accepts `iter(obj)` for an object with only `__getitem__`; Monty has no `__getitem__` iteration fallback and raises `TypeError` at runtime.
- `-t` accepts `for x in obj` (though not `a, b = obj`) for a class that opts out of iteration with `__iter__ = None`, which raises `TypeError` at runtime as it does in CPython.
- Iterator `repr()` values omit CPython's process-local memory address, for example `<list_iterator object>` rather than `<list_iterator object at 0x...>`.
- Iterator-specific attributes such as `__length_hint__` are not exposed.
