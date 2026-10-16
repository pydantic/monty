# `functools` module

Monty implements a small subset of `functools`.
The implemented callables match CPython 3.14 for arguments, values, `repr()` and error messages, apart from the notes
below.

## Implemented

`reduce(function, iterable, /[, initial])`, `partial(func, /, *args, **keywords)`,
`lru_cache(maxsize=128, typed=False)`, and `cache(user_function)`.

## Not implemented

Everything else: `cached_property`, `wraps`, `update_wrapper`, `partialmethod`, `total_ordering`,
`singledispatch`, `singledispatchmethod`, `cmp_to_key`, `get_cache_token`, `WRAPPER_ASSIGNMENTS`, `WRAPPER_UPDATES`.

`functools.Placeholder`, added in 3.14 to skip a positional slot when binding (`partial(f, Placeholder, 2)`), is also
absent, so every bound positional fills a leading slot.

These names are absent from the module namespace rather than stubbed, so they are rejected at type-check time
(`Module 'functools' has no member 'wraps'`) and raise `AttributeError` at runtime.

## Behavioural divergences

- **Calling a `partial` charges the native re-entry budget.** A partial stored as a class attribute binds as a bound
    method whose `__func__` is another partial, so a chain of them nests on the interpreter's own call stack without
    pushing a Python frame. Monty bounds that chain at the fixed native re-entry depth (see
    [resource_limits.md](resource_limits.md)), raising `RecursionError: maximum recursion depth exceeded` beyond roughly
    a dozen
    levels; CPython runs such a chain into the thousands before its own C stack gives out. Ordinary use is unaffected —
    nested `partial(partial(f, 1), 2)` is flattened at construction, and a partial passed to `map()` or `sorted(key=)`
    costs one level.
- **`partial` objects have no `__dict__`.** CPython allows arbitrary attributes on one (`p.x = 1`), which Monty rejects
    with `AttributeError: 'functools.partial' object has no attribute 'x' and no __dict__ for setting new attributes`.
    Assigning to `func`, `args` or `keywords` fails in both, but CPython words it `AttributeError: readonly attribute`.
- **`args` and `keywords` are rebuilt on each access.** `p.args is p.args` is `False`, where CPython returns the same
    objects every time.
    Mutating the dict from `p.keywords` therefore has no effect on what the partial passes on; CPython returns the live
    dict, so mutating it changes later calls.
- **`partial` objects carry no dunder attributes.** `p.__call__` and `p.__doc__` both raise `AttributeError`, where
    CPython has a method-wrapper and the type's docstring respectively.
- **A `partial` crossing the host boundary marshals as its `repr`.** Python and JavaScript hosts receive
    `MontyObject::repr("functools.partial(...)")` rather than a callable, since neither side can call back into a value
    that only exists inside the sandbox.

## Caching divergences

- Cached functions have no `__dict__` and do not copy the wrapped function's metadata
    (`__name__`, `__qualname__`, `__module__`, `__doc__`, or annotations).
    `__wrapped__` is readable but not writable.
- `cache_info()`, `cache_clear()`, and `cache_parameters()` work as calls, but retrieving those methods as values
    raises `AttributeError`, as with other [native methods](language.md).
    Explicit `__call__` and `__get__` attribute access is also unsupported.
- `lru_cache(maxsize=...)` returns a configured wrapper rather than CPython's decorating closure, so its type and
    `repr()` differ.
    The module does not expose CPython's private `_lru_cache_wrapper` constructor or `_CacheInfo` class.
- Integer `maxsize` values outside the signed 64-bit range are rejected with `TypeError`.
    CPython accepts arbitrarily negative sizes as zero and raises `OverflowError` when decorating with an oversized
    positive size.
    `cache_parameters()` reports normalized integer/boolean settings rather than the original argument objects.
- Cache misses consume an additional recursion frame, including for `maxsize=0`.
    Hits do not consume a frame.
    Consequently recursive cached functions can reach the [recursion limit](resource_limits.md) earlier than CPython.
- Eviction removes an entry by position without running user hashing or equality methods.
    CPython can compare colliding keys during eviction, so comparison side effects can differ.
    The number of hash and equality calls during lookup and insertion can also differ.
- An unhashable argument is diagnosed by hashing arguments again after hashing the complete key fails.
    A user `__hash__` can therefore run twice on a failing call.
    For nested unhashable values, the error can name the enclosing tuple instead of its unhashable element.
- Applying a caching decorator to a method with `@` syntax is rejected along with other
    [method decorators](classes.md).
    Assigning `method = functools.cache(function)` in the class body is supported.
- A cached function crossing the host boundary is represented by its `repr`, not a host-callable function.
