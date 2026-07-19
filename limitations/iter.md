# `iter()` and iterators

- The two-argument `iter(callable, sentinel)` form is not implemented.
- Lists have a distinct `list_iterator` type; other built-in iterables currently use Monty's generic `iterator` type rather than CPython's type-specific iterator classes.
- Iterator `repr()` values omit CPython's process-local memory address, for example `<list_iterator object>` rather than `<list_iterator object at 0x...>`.
- Iterator-specific attributes such as `__length_hint__` are not exposed.
