# `itertools` module

Monty implements a small subset of `itertools`. The implemented callables match
CPython 3.14 for arguments, values, `repr()` and error messages, subject to the
notes below.

## Implemented

`count(start=0, step=1)`, `repeat(object, times=?)`, `pairwise(iterable)`,
`compress(data, selectors)`, `islice(iterable, [start,] stop[, step])`,
`chain(*iterables)`, `cycle(iterable)`.

## Not implemented

Everything else: `accumulate`, `batched`, `combinations`,
`combinations_with_replacement`, `dropwhile`, `filterfalse`, `groupby`,
`permutations`, `product`, `starmap`, `takewhile`, `tee`, `zip_longest`.

`chain.from_iterable` is also absent, even though `chain` itself is
implemented: it is a classmethod reached through an attribute on the `chain`
builtin, and Monty's module functions expose no attributes
(`itertools.chain.from_iterable` raises `AttributeError: 'builtin_function_or_method'
object has no attribute 'from_iterable'`). Use `chain(*iterables)` instead.

These names are absent from the module namespace rather than stubbed, so they
are rejected at type-check time (`Module 'itertools' has no member 'chain'`) and
raise `AttributeError` at runtime.

## Behavioural divergences

- **`repeat.__length_hint__()` raises `AttributeError`.** CPython exposes the
  number of remaining yields through it (`repeat(9, 3).__length_hint__() == 3`).
  Monty uses the remaining count internally to size the target of `list()` /
  `tuple()`, but does not expose it as a Python-visible attribute.
- **`type(...).__name__` is the dotted name.** `type(itertools.count()).__name__`
  is `'itertools.count'`, where CPython reports the bare `'count'`. This is
  Monty's general treatment of types whose CPython `tp_name` is dotted (`re.Pattern`
  behaves the same way); `str(type(itertools.count()))` matches CPython's
  `"<class 'itertools.count'>"`, as do error messages naming the type.
- **`count` and `repeat` objects are unhashable.** `hash(itertools.count())`
  raises `TypeError: unhashable type: 'itertools.count'`, where CPython falls
  back to identity hashing. This applies to Monty's iterators generally, not
  just these two.
- **`count` accepts only `int`, `float` and `bool`.** CPython accepts anything
  satisfying `PyNumber_Check` (e.g. `Decimal`, `Fraction`, complex). Monty has
  no other numeric types, so the same `TypeError: a number is required` covers
  them all.
- **Nested-cycle `repr()` unwinds one level earlier.** For a container that
  reaches back to the `repeat` holding it, Monty prints `repeat([...])` where
  CPython prints `repeat([repeat([...])])`. This is Monty's general cycle
  detection in `repr()`, not specific to `itertools`.
- **Adaptors without a custom `repr()` omit the address.** `repr(pairwise([]))`
  is `<itertools.pairwise object>`, where CPython appends ` at 0x...`. This is
  Monty's general iterator treatment (see `limitations/iter.md`), not specific
  to `itertools`.
- **Crossing the host boundary loses the repr.** A `count` / `repeat` object
  returned to the host arrives as `<itertools.count object>` /
  `<itertools.repeat object>` rather than its in-sandbox `repr()`
  (`count(0)`, `repeat(7, 3)`). Monty represents all iterators this way rather
  than recursing into what they hold.

## Infinite iterators and the eager builtins

`map()`, `filter()` and `enumerate()` are **eager** in Monty: each drains its
source into a list and returns a concrete result, rather than the lazy iterator
CPython returns. Applied to an infinite `itertools` iterator they therefore
never return, where CPython yields lazily:

```python
map(f, itertools.count())        # CPython: lazy. Monty: runs until a limit trips.
filter(p, itertools.repeat(1))   # likewise
enumerate(itertools.count())     # likewise
```

`zip()` is safe because it stops at the shortest input, so
`zip(itertools.count(), 'ab')` behaves as in CPython — as does slicing an
infinite iterator by hand via `next()`. This is a pre-existing property of those
builtins rather than something `itertools` introduces, but `count()`/`repeat()`
are the first easy way for sandboxed code to reach it.

## Resource limits

`count()` and `repeat(x)` are infinite, so consuming one without a bound
(`list(itertools.count())`) only terminates if the host has configured a memory
or duration limit — it then raises `MemoryError` rather than exhausting. Under
`ResourceLimits::default()`, which sets neither (only a recursion depth), it
runs until the host itself runs out of memory. This is the same exposure as a
`while True:` loop, not something specific to `itertools`.

`cycle(iterable)` must buffer every item it has seen so far in order to replay
them, and that buffer is charged against `max_memory` as it grows — so cycling
over a very long source raises `MemoryError` at the limit rather than at the
point the source is exhausted. CPython buffers the same items with no such
ceiling.
