# `functools` module

Monty implements a small subset of `functools`.
The implemented callables match CPython 3.14 for arguments, values, `repr()` and error messages, apart from the notes
below.

## Implemented

`reduce(function, iterable, /[, initial])` and `partial(func, /, *args, **keywords)`.

## Not implemented

Everything else: `cache`, `lru_cache`, `cached_property`, `wraps`, `update_wrapper`, `partialmethod`, `total_ordering`,
`singledispatch`, `singledispatchmethod`, `cmp_to_key`, `get_cache_token`, `WRAPPER_ASSIGNMENTS`, `WRAPPER_UPDATES`.

`functools.Placeholder`, added in 3.14 to skip a positional slot when binding (`partial(f, Placeholder, 2)`), is also
absent, so every bound positional fills a leading slot.

These names are absent from the module namespace rather than stubbed, so they are rejected at type-check time (`Module
'functools' has no member 'lru_cache'`) and raise `AttributeError` at runtime.

## Behavioural divergences

- **`reduce()` cannot call a host function.** The reduction function runs through the same synchronous path as
  `map()`'s, which cannot suspend the VM. An external function raises `NotImplementedError: reduce(): external
  function 'f' is not yet supported in this context`; one that touches the filesystem raises the same error naming
  the OS function it maps to, e.g. `reduce(): OS function 'Path.iterdir' is not yet supported in this context` for
  `os.listdir()`.
  This covers a `partial` that wraps one.
  Calling a `partial` anywhere else is unaffected, since that goes through the ordinary call path.
- **Calling a `partial` charges the native re-entry budget.** A partial stored as a class attribute binds as a bound
  method whose `__func__` is another partial, so a chain of them nests on the interpreter's own call stack without
  pushing a Python frame. Monty bounds that chain at the fixed native re-entry depth (see
  ./resource_limits.md), raising `RecursionError: maximum recursion depth exceeded` beyond roughly a dozen
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
- **`type(...).__name__` is the dotted name.** `type(functools.partial(f)).__name__` is `'functools.partial'`, where
  CPython reports the bare `'partial'`.
  This is Monty's general treatment of types whose CPython `tp_name` is dotted (`re.Pattern` and `itertools.count`
  behave the same way); `str(type(p))` matches CPython's `"<class 'functools.partial'>"`, as do error messages naming
  the type.
- **`partial` objects carry no dunder attributes.** `p.__call__` and `p.__doc__` both raise `AttributeError`, where
  CPython has a method-wrapper and the type's docstring respectively.
- **`partial[int]` is not subscriptable at runtime.** CPython returns a `types.GenericAlias`; Monty raises
  `TypeError: 'type' object is not subscriptable`, as it does for `list[int]` — there are no runtime generic aliases
  at all (see ./typing.md). The type checker accepts the expression, so this is one of the few divergences the stubs
  cannot reject up front. Annotations are unaffected, Monty stringizing them rather than evaluating them.
- **A `partial` crossing the host boundary marshals as its `repr`.** Python and JavaScript hosts receive
  `MontyObject::Repr("functools.partial(...)")` rather than a callable, since neither side can call back into a value
  that only exists inside the sandbox.

## Notes

`partial` is a descriptor, as it is in CPython 3.14: one stored as a class attribute binds the instance as the next
argument after those it already carries.
