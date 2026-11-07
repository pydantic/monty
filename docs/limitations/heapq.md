# `heapq` module

`heappush`, `heappop`, `heappushpop`, `heapreplace`, `heapify`, their `_max` forms (`heappush_max`, `heappop_max`,
`heappushpop_max`, `heapreplace_max`, `heapify_max`), `merge`, `nlargest` and `nsmallest` are available. Heap
operations follow CPython's `_heapq` algorithms, so the order of a heap's list, the tie-breaking of `nlargest` and
`nsmallest`, and error messages match.

- **`merge` is eager and returns a list.** CPython returns a lazy generator. As with `map`, `filter` and `zip` (see
    [builtins.md](builtins.md)), Monty consumes every input when `merge` is called, so an infinite input does not
    terminate, side effects and errors from the inputs or `key` happen at the call rather than during iteration, and
    the result supports `next()` only after wrapping it in `iter()`. Iterating it or passing it to `list()` works as
    usual.
- **Heaps must be exact lists.** Anything else raises CPython's `TypeError`; subclasses of `list` cannot be defined
    in Monty (see [classes.md](classes.md)).
- **`n` must be an integer.** A non-integer `n` raises `TypeError: 'float' object cannot be interpreted as an integer`,
    where CPython may report a different message. An `n` outside the signed 64-bit range is clamped.
- **Key functions cannot suspend to the host.** As with `sorted(key=...)`, a key function that calls an external or
    OS function raises `NotImplementedError` at the suspension point.
- **User-defined ordering follows Monty's comparison support.** Comparisons use the normal rich comparison, so
    custom `__lt__` methods are not dispatched; see [classes.md](classes.md). Use tuples (for example
    `(priority, count, item)`) for prioritised items.
- **A list that changes size during a comparison raises `RuntimeError`**, as in CPython; Monty cannot currently
    trigger this from user code.
- **Function metadata follows other built-in Monty module functions.** The module does not expose CPython's
    `__all__` list or its private `_heapq` accelerator module.
