# `iter()` and iterators

- `iter(callable, sentinel)` runs `callable` synchronously, so one that calls an external/OS function cannot suspend and raises `NotImplementedError` — the same limitation as `map`/`filter`/`sorted(key=...)`.
- `iter(callable, sentinel)` compares `result == sentinel`, where CPython compares `sentinel == result`; only observable if the two sides have asymmetric `__eq__`.
- A `StopIteration` raised by `callable` propagates; CPython treats it as clean exhaustion and stops iterating.
- A user instance defining `__call__` is rejected as not callable, since `__call__` is not dispatched (see [classes.md](classes.md)).
- Lists have a distinct `list_iterator` type and `iter(callable, sentinel)` a `callable_iterator`; other built-in iterables currently use Monty's generic `iterator` type rather than CPython's type-specific iterator classes.
- Iterator `repr()` values omit CPython's process-local memory address, for example `<list_iterator object>` rather than `<list_iterator object at 0x...>`.
- Iterator-specific attributes such as `__length_hint__` are not exposed.
