# `functools` module

Monty implements a small subset of `functools`.
The implemented callables match CPython 3.14 for arguments, values, `repr()` and error messages, apart from the notes
below.

## Implemented

`reduce(function, iterable, /[, initial])`, `partial(func, /, *args, **keywords)`,
`lru_cache(maxsize=128, typed=False)` and `cache(user_function)`.

The `lru_cache` wrapper supports `__wrapped__`, `cache_info()`, `cache_clear()` and `cache_parameters()`, evicts the
least recently used entry when full, and keys calls the way CPython's `_make_key` does, including the fast path that
makes a lone `int` or `str` argument its own key (so `f(1)` and `f(1.0)` are separate entries even without `typed`).

## Not implemented

Everything else: `cached_property`, `wraps`, `update_wrapper`, `partialmethod`, `total_ordering`, `singledispatch`,
`singledispatchmethod`, `cmp_to_key`, `get_cache_token`, `WRAPPER_ASSIGNMENTS`, `WRAPPER_UPDATES`.

`functools.Placeholder`, added in 3.14 to skip a positional slot when binding (`partial(f, Placeholder, 2)`), is also
absent, so every bound positional fills a leading slot.

These names are absent from the module namespace rather than stubbed, so they are rejected at type-check time
(`Module 'functools' has no member 'wraps'`) and raise `AttributeError` at runtime.

## Behavioural divergences

- **`reduce()` cannot call a host function.** The reduction function runs through the same synchronous path as
    `map()`'s, which cannot suspend the VM. An external function raises
    `NotImplementedError: reduce(): external function 'f' is not yet supported in this context`; one that touches the
    filesystem raises the same error naming
    the OS function it maps to, e.g. `reduce(): OS function 'Path.iterdir' is not yet supported in this context` for
    `os.listdir()`.
    This covers a `partial` that wraps one.
    Calling a `partial` anywhere else is unaffected, since that goes through the ordinary call path.
- **Calling a `partial` charges the native re-entry budget.** A partial stored as a class attribute binds as a bound
    method whose `__func__` is another partial, so a chain of them nests on the interpreter's own call stack without
    pushing a Python frame. Monty bounds that chain at the fixed native re-entry depth (see
    [resource_limits.md](resource_limits.md)), raising `RecursionError: maximum recursion depth exceeded` beyond roughly
    a dozen
    levels; CPython runs such a chain into the thousands before its own C stack gives out. Ordinary use is unaffected —
    nested `partial(partial(f, 1), 2)` is flattened at construction, and a partial passed to `map()` or `sorted(key=)`
    costs one level.
- **Calling a cached function charges the native re-entry budget too.** Stacked wrappers — `cache(cache(f))`, or a
    cached function whose `__wrapped__` is another one — dispatch once per layer on the interpreter's own call stack
    without pushing a Python frame, so Monty bounds the chain the same way, raising
    `RecursionError: maximum recursion depth exceeded` beyond roughly a dozen layers where CPython goes on into the
    thousands. Each layer keeps its own cache, so a stacked call that does run is stored by every wrapper it passed
    through. A cached function calling *itself* is unaffected: recursion pushes ordinary Python frames and is bounded
    by `max_recursion_depth` as usual.
- **A cached host function is never cached.** `cache(ext_fn)` calls the host on every call and counts every one as a
    miss: the result comes back from the host after the frame that would have stored it is gone.
    A cached *Python* function that suspends part-way through — because it calls a host function or performs an `os`
    call — stores its result normally.
- **`partial` objects have no `__dict__`, and neither do cached functions.** CPython allows arbitrary attributes on
    both (`p.x = 1`), which Monty rejects with `AttributeError: 'functools.partial' object has no attribute 'x' and no   __dict__ for setting new attributes`.
    Assigning to `func`, `args` or `keywords` fails in both, but CPython words it `AttributeError: readonly attribute`.
- **A cached function has none of the attributes `update_wrapper` copies.** CPython's `lru_cache` copies `__name__`,
    `__qualname__`, `__doc__` and `__module__` from the wrapped function onto the wrapper; Monty functions carry none
    of those to begin with (see [language.md](language.md)), so the wrapper has none either.
- **The decorator `lru_cache(maxsize=…)` returns is a wrapper, not a function.** CPython returns a closure, so `type()`
    reports `function` and `repr()` reads `<function lru_cache.<locals>.decorating_function at 0x…>`.
    Monty returns a `functools._lru_cache_wrapper` holding the settings until the function to cache arrives, so it
    reprs as one and answers `cache_info()`.
    Its argument errors match CPython's closure.
- **`cache_parameters()` reports normalized settings.** CPython echoes the objects given, so `lru_cache(True)` reports
    `{'maxsize': True, …}` and `lru_cache(2, 1)` reports `{…, 'typed': 1}`.
    Monty reports the `int` and `bool` it stored: `{'maxsize': 1, …}` and `{…, 'typed': True}`.
    `cache_info().maxsize` is the normalized `int` in both.
- **`args` and `keywords` are rebuilt on each access.** `p.args is p.args` is `False`, where CPython returns the same
    objects every time.
    Mutating the dict from `p.keywords` therefore has no effect on what the partial passes on; CPython returns the live
    dict, so mutating it changes later calls.
- **`type(...).__name__` is the dotted name.** `type(functools.partial(f)).__name__` is `'functools.partial'`, where
    CPython reports the bare `'partial'`.
    This is Monty's general treatment of types whose CPython `tp_name` is dotted (`re.Pattern` and `itertools.count`
    behave the same way); `str(type(p))` matches CPython's `"<class 'functools.partial'>"`, as do error messages naming
    the type.
- **The methods of a cached function can only be called, not referenced.** `f.cache_info()` works, `f.cache_info`
    alone raises `AttributeError`.
    This applies to the methods of every native type in Monty (`re.Match.group`, `deque.append`, …), not just these.
- **An unhashable argument nested inside a hashable one names the wrong type.** `f([1])` raises CPython's
    `TypeError: unhashable type: 'list'`, but `f(([1],))` says `'tuple'` where CPython says `'list'`.
    Monty's tuple hash reports the container rather than the element that refused to hash, which is visible without
    `functools` too (`hash(([1],))`).
- **`partial` objects carry no dunder attributes.** `p.__call__` and `p.__doc__` both raise `AttributeError`, where
    CPython has a method-wrapper and the type's docstring respectively.
- **`partial[int]` is not subscriptable at runtime.** CPython returns a `types.GenericAlias`; Monty raises
    `TypeError: 'type' object is not subscriptable`, as it does for `list[int]` — there are no runtime generic aliases
    at all (see [typing.md](typing.md)). The type checker accepts the expression, so this is one of the few divergences
    the stubs
    cannot reject up front. Annotations are unaffected, Monty stringizing them rather than evaluating them.
- **A `partial` or cached function crossing the host boundary marshals as its `repr`.** Python and JavaScript hosts
    receive `MontyObject::Repr("functools.partial(...)")` rather than a callable, since neither side can call back into
    a value that only exists inside the sandbox.

## Notes

`partial` and the `lru_cache` wrapper are descriptors, as they are in CPython 3.14: one stored as a class attribute
binds the instance as the next argument after those it already carries.
Reaching that through `@functools.cache` on a `def` in a class body does not work — Monty's parser rejects all method
decorators (see [classes.md](classes.md)) — so it needs `m = functools.cache(f)` in the class body instead.

Caching an `async def` caches the coroutine the call returns rather than the value it resolves to, so awaiting the
second call raises `RuntimeError: cannot reuse already awaited coroutine`.
CPython behaves the same way.
