# `functools` module

Monty implements a small subset of `functools`.
The implemented callables match CPython 3.14 for arguments, values, `repr()` and error messages, apart from the notes
below.

## Implemented

`reduce(function, iterable, /[, initial])`.

## Not implemented

Everything else: `partial`, `cache`, `lru_cache`, `cached_property`, `wraps`, `update_wrapper`, `partialmethod`,
`total_ordering`, `singledispatch`, `singledispatchmethod`, `cmp_to_key`, `get_cache_token`, `Placeholder`,
`WRAPPER_ASSIGNMENTS`, `WRAPPER_UPDATES`.

These names are absent from the module namespace rather than stubbed, so they are rejected at type-check time (`Module
'functools' has no member 'lru_cache'`) and raise `AttributeError` at runtime.

## Behavioural divergences

- **`reduce()` cannot call a host function.** The reduction function runs through the same synchronous path as
  `map()`'s, which cannot suspend the VM. An external function raises `NotImplementedError: reduce(): external
  function 'f' is not yet supported in this context`; one that touches the filesystem raises the same error naming
  the OS function it maps to, e.g. `reduce(): OS function 'Path.iterdir' is not yet supported in this context` for
  `os.listdir()`.
