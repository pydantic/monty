# `heapq` and `_heapq` modules

`_heapq` is CPython's native accelerator: `heappush`, `heappop`, `heappushpop`, `heapreplace`, `heapify` and their
`_max` forms (`heappush_max`, `heappop_max`, `heappushpop_max`, `heapreplace_max`, `heapify_max`). `heapq` re-exports
those same functions (`heapq.heappush is _heapq.heappush`) and adds `nlargest` and `nsmallest`, which are native
stand-ins for the pure-Python versions in `Lib/heapq.py`. Heap operations follow CPython's algorithms, so the order of
a heap's list, the tie-breaking of `nlargest` and `nsmallest`, and error messages match.

- **`heapq.merge` is not available.** In CPython it is a generator function, which Monty cannot express yet; accessing
    it raises `AttributeError`. It is expected to arrive with the vendored `heapq.py`
    ([#1275](https://github.com/pydantic/monty/issues/1275)).
- **Heaps must be exact lists.** Anything else raises CPython's `TypeError`; subclasses of `list` cannot be defined
    in Monty (see [classes.md](classes.md)).
- **`n` accepts integers and objects with `__index__`.** A float raises
    `TypeError: 'float' object cannot be interpreted as an integer`, where CPython may report a different message. An
    `n` outside the signed 64-bit range is clamped.
- **Key functions cannot suspend to the host.** As with `sorted(key=...)`, a key function that calls an external or
    OS function raises `NotImplementedError` at the suspension point.
- **User-defined ordering follows Monty's comparison support.** Comparisons use the normal rich comparison, so
    custom `__lt__` methods are not dispatched; see [classes.md](classes.md). Use tuples (for example
    `(priority, count, item)`) for prioritised items.
- **Module metadata follows other built-in Monty module functions.** `heapq` does not expose `__all__`, and `_heapq`
    does not expose `__about__`.
