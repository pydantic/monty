# `heapq` and `_heapq` modules

`_heapq` provides `heappush`, `heappop`, `heappushpop`, `heapreplace`, `heapify` and their `_max` forms.
`heapq` re-exports them and adds native `nlargest` and `nsmallest`.

- **`heapq.merge` is not available.**
    In CPython it is a generator function, which Monty cannot express yet, so accessing it raises `AttributeError`.
    It is expected to arrive with the vendored `heapq.py` ([#1275](https://github.com/pydantic/monty/issues/1275)).
- **Key functions cannot suspend to the host.**
    As with `sorted(key=...)`, a key function that calls an external or OS function raises `NotImplementedError` at
    the suspension point.
- **Custom `__lt__` methods are not dispatched** (see [classes.md](classes.md)).
    Use tuples such as `(priority, count, item)` for prioritised items.
- **`heapq` does not expose `__all__`, and `_heapq` does not expose `__about__`.**
