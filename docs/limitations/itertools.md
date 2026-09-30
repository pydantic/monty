# `itertools` module

Monty implements the whole of `itertools`, matching CPython 3.14 for
arguments, values, `repr()` and error messages apart from the notes below.

## Implemented

Every name CPython's `itertools` exports, including the private
`_grouper`, `_tee` and `_tee_dataobject`. As in CPython, all of them are type
objects except `tee`, which is a plain function — so `isinstance(x, count)`,
`type(x) is chain` and `chain[int]` all work.

## Behavioural divergences

- **`repeat.__length_hint__()` raises `AttributeError`.** CPython exposes the
    number of remaining yields through it (`repeat(9, 3).__length_hint__() == 3`).
    Monty uses the remaining count internally to size the target of `list()` /
    `tuple()`, but does not expose it as a Python-visible attribute.
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
- **A callable that suspends is rejected, not paused.** `takewhile`,
    `dropwhile`, `filterfalse`, `starmap`, `accumulate` and `groupby` apply their callable
    through the synchronous `evaluate_function` path, which runs a frame to
    completion and cannot yield to the host. A callable that reaches an external
    function, an `os` operation, or a host method call therefore raises
    `NotImplementedError`, naming the adaptor that called it —
    `takewhile(): external function 'f' is not yet supported in this context`
    from `takewhile`, `groupby(): ...` from `groupby`, and so on.
    CPython would simply call it. This is the same restriction that applies to `__init__`,
    `__next__` and `__repr__` (see [classes.md](classes.md)); ordinary
    sandbox-defined functions and lambdas are unaffected.
- **`zip_longest` names the rejected keyword.** A bad keyword raises
    `zip_longest() got an unexpected keyword argument 'bogus'`, where CPython
    raises `zip_longest() got an unexpected keyword argument` with no name —
    CPython hand-rolls that check rather than going through a parser family, and
    is alone in omitting it. Every other adaptor's wording matches.
- **A re-entrant `zip_longest` stops instead of padding forever.** A source
    that steps the same `zip_longest` from inside its own `__next__` can exhaust
    the remaining slots before the outer round reaches them. Both agree on the
    row that outer round yields; from the next one on Monty raises
    `StopIteration`, having seen every slot go spent, while CPython drives its
    `numactive` count below zero and so yields an all-`fillvalue` tuple on every
    later `next()`, without end. Only re-entrancy reaches this: an ordinary
    source cannot run while the adaptor that owns it is mid-round.
- **Crossing the host boundary loses the repr.** A `count` / `repeat` object
    returned to the host arrives as `<itertools.count object>` /
    `<itertools.repeat object>` rather than its in-sandbox `repr()`
    (`count(0)`, `repeat(7, 3)`). Monty represents all iterators this way rather
    than recursing into what they hold.
- **`batched`'s `n` is bounded by the worker's pointer width**, so the wasm
    worker raises `OverflowError: Python int too large to convert to C ssize_t`
    for `n` at or above `2**31`, where CPython accepts it.
- **`batched`'s type cannot cross to a host below Python 3.12**, which is where
    `itertools.batched` was added. Returning `type(itertools.batched(...))` to
    such a host raises
    `TypeError: Cannot convert itertools.batched to a host type: this Python does not define it`.
    Every other adaptor's type resolves on all supported hosts.
- **`chain.from_iterable` is a plain function, not a bound classmethod.** CPython
    builds a new bound method object per attribute access, so
    `repr(itertools.chain.from_iterable)` is
    `<built-in method from_iterable of type object at 0x...>` and
    `itertools.chain.from_iterable is itertools.chain.from_iterable` is `False`.
    Monty resolves the attribute to one function value, so the `repr` reads
    `<function from_iterable at 0x...>` and the identity check is `True`. The
    attribute is also reachable only through the `chain` type itself: an
    instance does not carry it, so `itertools.chain([1]).from_iterable([[2]])`
    raises `AttributeError: 'itertools.chain' object has no attribute 'from_iterable'`
    where CPython accepts it. Its element type is also lost in type checking,
    since the vendored typeshed stub declares it as a classmethod.
- **The private types cannot be constructed.** `_grouper`, `_tee` and
    `_tee_dataobject` are exposed under their CPython names, so `type()` and
    `isinstance()` work, but calling one raises
    `TypeError: cannot create 'itertools._tee' instances`. CPython builds them
    from the arguments its internals use (`_tee([1, 2])` gives a working
    iterator). They are only ever handed out by `groupby` and `tee` here.
- **`groupby` never releases its source.** This matches CPython, but note that
    the [resource limits](resource_limits.md) apply to the skip between groups:
    a source whose key never changes (`groupby(repeat(1))`) makes the second
    `next()` scan forever, and is stopped by a duration limit rather than
    running to completion.
- **The combinatoric iterators collect their input at construction.** CPython
    does the same (its `pool` is a tuple built by `PySequence_Tuple`), so
    `combinations`, `combinations_with_replacement`, `permutations` and
    `product` all consume the whole iterable before the first `next()` and
    raise there for a non-iterable or a raising source. The consequence worth
    naming is that an infinite input never returns: `permutations(count())`
    runs until a resource limit trips on both engines.
- **`product`'s `repeat` and `combinations_with_replacement`'s `r` are
    preflighted against `max_memory`.** Each sizes a result wider than the input
    it was given, so under a memory limit a large value
    (`product('ab', repeat=10**9)`) raises `MemoryError` at construction, where
    CPython raises only once an allocation actually fails. An empty pool is
    exempt, since it empties the product before anything is sized.
- **An `r` too large to build indices for yields nothing instead of raising.**
    `combinations('a', 2**62)` and `permutations('a', 2**62)` are empty
    iterators in Monty, because an `r` past the pool is known to yield nothing
    before any vector is sized. CPython allocates the vector first and so
    raises `MemoryError`. The two calls that genuinely need the vector agree
    with CPython: `combinations_with_replacement('a', 2**62)` raises
    `MemoryError`, and `product('ab', repeat=2**62)` raises
    `OverflowError: repeat argument too large`. All four are 64-bit worker
    behaviour: like `batched`'s `n`, `r` and `repeat` are bounded by the
    worker's pointer width, so the wasm worker raises
    `OverflowError: Python int too large to convert to C ssize_t` for anything
    at or above `2**31` before reaching any of it.
- **`product` names a rejected keyword instead of counting keywords.** Two
    keywords where one is unknown (`product([1], repeat=2, bogus=1)`) raise
    `product() got an unexpected keyword argument 'bogus'`, where CPython
    raises `product() takes at most 1 keyword argument (2 given)`. A single
    unknown keyword reads the same on both.
- **A `groupby` key comparison that re-enters its own `groupby` keeps going.**
    A user `__eq__` that steps the same `groupby` and consumes the pair being
    compared leaves CPython reading through freed state, where it segfaults.
    Monty reads the next pair instead, which is what CPython's own loop
    condition intends, so the call ends in an ordinary `StopIteration`.

## Infinite iterators and the eager builtins

`map()`, `filter()` and `enumerate()` are **eager** in Monty: each drains its
source into a list and returns a concrete result, rather than the lazy iterator
CPython returns. Applied to an infinite `itertools` iterator they therefore
never return, where CPython yields lazily:

```python test="skip"
import itertools

map(str, itertools.count())  # CPython: lazy. Monty: runs until a limit trips.
filter(bool, itertools.repeat(1))  # likewise
enumerate(itertools.count())  # likewise
```

`zip()` stops at the shortest input, so `zip(itertools.count(), 'ab')` behaves
as in CPython, as does slicing an infinite iterator by hand via `next()`. This
is a pre-existing property of those builtins rather than something `itertools`
introduces, but `count()`/`repeat()` are the first easy way for sandboxed code
to reach it.

## Resource limits

`count()` and `repeat(x)` are infinite, so consuming one without a bound
(`list(itertools.count())`) only terminates if the host has configured a memory
or duration limit, and then raises `MemoryError` rather than exhausting. Under
`ResourceLimits::default()`, which sets neither (only a recursion depth), it
runs until the host itself runs out of memory. This is the same exposure as a
`while True:` loop, not something specific to `itertools`.

The adaptors that discard items without yielding — `dropwhile` and
`filterfalse` before their first accepted item, `compress` past a falsy run,
`islice` skipping to `start`, `chain` crossing an exhausted source — poll
the duration limits themselves while looping, so a discarding pass over an
infinite source raises `TimeoutError` instead of spinning. The poll is amortized (once
per 64 items), so the limit can be overshot by up to that much work. CPython
has no duration limit at all and would loop forever.

`batched(iterable, n)` fills a whole batch inside one `next()`, so a large `n`
over a long source is the same kind of non-yielding loop and polls the
duration limits the same way. It also preflights one batch against `max_memory`
from the source's size hint, capped at `n` — an exact-hint source
(`batched(range(10**9), 10**9)`) therefore raises `MemoryError` up front rather
than while filling. A source with no size hint gets no preflight, so the fill
loop polls `max_memory` as well as the duration limits on the same amortized
cadence — `batched(count(), 10**9)` raises `MemoryError` while filling.

`cycle(iterable)` must buffer every item it has seen so far in order to replay
them, and that buffer is charged against `max_memory` as it grows, so cycling
over a very long source raises `MemoryError` at the limit rather than at the
point the source is exhausted. CPython buffers the same items with no such
ceiling.

Nesting the source-driving adaptors — everything except `count` and `repeat` —
is bounded by `max_recursion_depth`: an adaptor charges one recursion level
while delegating `next()` to its wrapped iterator, so a nest deeper than the
limit raises `RecursionError` when consumed. An adaptor answering from its own
state charges nothing, since it touches no source: a spent `batched`, a latched
`takewhile`, an `accumulate` yielding its `initial`. CPython imposes no
comparable per-adaptor bound; deep nesting there is limited only by the C
stack.
