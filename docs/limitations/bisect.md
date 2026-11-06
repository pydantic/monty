# `bisect` module

`bisect_left`, `bisect_right`, `insort_left`, `insort_right`, and the aliases `bisect` and `insort` are available.

- **Sequences must be built-in containers.** Searches accept lists, tuples, strings, bytes, ranges and deques;
    integer-indexed `Counter` objects also support searches. Insertion accepts lists and deques. User-defined
    sequences cannot supply `__len__` and `__getitem__`, following the general [class limitations](classes.md).
- **Index width is fixed at 64 bits.** Bounds and implicit range lengths use signed 64-bit indices even on
    32-bit WebAssembly; CPython uses its platform index width.
- **Key functions cannot suspend to the host.** As with `sorted(key=...)`, a key function that calls an external or
    OS function raises `NotImplementedError` at the suspension point. Ordinary sandbox functions and built-in
    callables work.
- **User-defined ordering follows Monty's comparison support.** Custom comparison dunders have the same
    restrictions as ordinary comparisons; see [classes.md](classes.md).
- **Function metadata follows other built-in Monty module functions.** Their representation uses `<function ...>`
    rather than CPython's `<built-in function ...>`, and CPython's function metadata is not exposed.
    The module does not expose CPython's `__all__` list or its private `_bisect` accelerator module.
