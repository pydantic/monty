import functools
import sys

# === cache: values and counters ===
calls = []


@functools.cache
def double(n):
    calls.append(n)
    return n * 2


assert double(2) == 4
assert double(2) == 4
assert double(3) == 6
assert calls == [2, 3]
assert double.cache_info() == (1, 2, None, 2)
assert repr(double.cache_info()) == 'CacheInfo(hits=1, misses=2, maxsize=None, currsize=2)'
assert double.cache_info().hits == 1
assert double.cache_info().misses == 2
assert double.cache_info().maxsize is None
assert double.cache_info().currsize == 2
assert double.cache_parameters() == {'maxsize': None, 'typed': False}
# `__wrapped__` is the undecorated function, so calling it skips the cache
assert double.__wrapped__(5) == 10
assert calls == [2, 3, 5]
assert double.cache_info() == (1, 2, None, 2)
assert type(double) is type(functools.cache(lambda: 1))
assert repr(type(double)) == "<class 'functools._lru_cache_wrapper'>"
# the dotted `tp_name` shows in reprs, the bare one in `__name__`, as CPython does it
assert type(double).__name__ == '_lru_cache_wrapper'

# a cleared cache forgets both the results and the counters
double.cache_clear()
assert double.cache_info() == (0, 0, None, 0)
assert double(2) == 4
assert calls == [2, 3, 5, 2]

# === keys ===
seen = []


@functools.cache
def record(a, b=0, **kwargs):
    seen.append((a, b, kwargs))
    return len(seen)


# a value passed positionally is a different call from the same value by keyword
assert record(1) == 1
assert record(1, 0) == 2
assert record(1, b=0) == 3
assert record(1, b=0) == 3

# keyword order is part of the key, as it is in CPython
assert record(2, x=1, y=2) == 4
assert record(2, y=2, x=1) == 5
assert record(2, x=1, y=2) == 4

# a lone `int` or `str` argument is its own key, so an equal `float` is a
# separate entry even without `typed`
assert record(1.0) == 6
assert record(1.0) == 6
# equal arguments do share a key once the call takes more than one
assert record(True, 0) == 2
assert record.cache_info() == (4, 6, None, 6)

# an unhashable argument is rejected, naming the argument rather than the key
try:
    record([1])
    assert False, 'expected the call to fail'
except TypeError as exc:
    assert str(exc) == "unhashable type: 'list'"
try:
    record(1, k=[1])
    assert False, 'expected the call to fail'
except TypeError as exc:
    assert str(exc) == "unhashable type: 'list'"

# === a key comparison that mutates the cache ===
# Comparing keys runs `__eq__` on the arguments, which is free to empty the very
# cache being searched; the lookup must cope rather than reuse a stale position.


class Clearing:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        clearing.cache_clear()
        return False


@functools.cache
def clearing(x):
    return 1


assert clearing(Clearing()) == 1
assert clearing(Clearing()) == 1
assert clearing(Clearing()) == 1
assert clearing.cache_info() == (0, 1, None, 1)


@functools.lru_cache(maxsize=2)
def bounded_clearing(x):
    return 2


class ClearingBounded:
    def __hash__(self):
        return 7

    def __eq__(self, other):
        bounded_clearing.cache_clear()
        return False


assert bounded_clearing(ClearingBounded()) == 2
assert bounded_clearing(ClearingBounded()) == 2
assert bounded_clearing(ClearingBounded()) == 2
assert bounded_clearing.cache_info() == (0, 1, 2, 1)

# === typed ===
typed_calls = []


@functools.lru_cache(typed=True)
def identity(x):
    typed_calls.append(x)
    return x


assert identity(1) == 1
assert identity(1.0) == 1.0
assert identity(1) == 1
assert typed_calls == [1, 1.0]
assert identity.cache_info() == (1, 2, 128, 2)
assert identity.cache_parameters() == {'maxsize': 128, 'typed': True}

# === eviction is by least recent use ===
evict_calls = []


@functools.lru_cache(maxsize=2)
def small(n):
    evict_calls.append(n)
    return n


small(1)
small(2)
small(1)  # makes 2 the least recently used
small(3)  # evicts 2
assert evict_calls == [1, 2, 3]
assert small(1) == 1  # still cached
assert small(2) == 2  # recomputed
assert evict_calls == [1, 2, 3, 2]
assert small.cache_info() == (2, 4, 2, 2)

# `maxsize=0` caches nothing; a negative size means the same
zero_calls = []


@functools.lru_cache(maxsize=0)
def uncached(n):
    zero_calls.append(n)
    return n


uncached(1)
uncached(1)
assert zero_calls == [1, 1]
assert uncached.cache_info() == (0, 2, 0, 0)
assert functools.lru_cache(maxsize=-5)(lambda n: n).cache_info() == (0, 0, 0, 0)

# === decorator forms ===
# bare, parameterized, and `None` for unbounded
assert functools.lru_cache(lambda n: n * 2)(3) == 6
assert functools.lru_cache(lambda n: n * 2).cache_info() == (0, 0, 128, 0)
assert functools.lru_cache()(lambda n: n * 2)(3) == 6
assert functools.lru_cache(None)(lambda n: n).cache_info() == (0, 0, None, 0)
assert functools.lru_cache(maxsize=3, typed=True)(lambda n: n).cache_parameters() == {'maxsize': 3, 'typed': True}

# === recursion ===


@functools.cache
def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)


assert fib(30) == 832040
assert fib.cache_info() == (28, 31, None, 31)

# === a call that raises is not cached ===
boom_calls = []


@functools.cache
def boom(n):
    boom_calls.append(n)
    raise ValueError('no')


for _ in range(2):
    try:
        boom(1)
        assert False, 'expected the call to fail'
    except ValueError as exc:
        assert str(exc) == 'no'
assert boom_calls == [1, 1]
assert boom.cache_info() == (0, 2, None, 0)

# === cached callables other than functions ===
assert functools.cache(int)('10') == 10
assert functools.cache(functools.partial(pow, 2))(3) == 8

# a cached function stored on a class binds the instance, so `self` is part of
# the key


def _describe(self, k):
    return (self.tag, k)


class Tagged:
    describe = functools.cache(_describe)

    def __init__(self, tag):
        self.tag = tag


first = Tagged('a')
second = Tagged('b')
assert first.describe(1) == ('a', 1)
assert second.describe(1) == ('b', 1)
assert first.describe(1) == ('a', 1)
assert Tagged.describe.cache_info() == (1, 2, None, 2)

# === stacked wrappers ===
# both caches tag the same call, so both store its result


def _double_raw(x):
    stacked_calls.append(x)
    return x * 2


stacked_calls = []
stacked_inner = functools.cache(_double_raw)
stacked_outer = functools.cache(stacked_inner)

assert stacked_outer(1) == 2
assert stacked_outer(1) == 2
assert stacked_outer(2) == 4
assert stacked_calls == [1, 2]
assert stacked_outer.cache_info() == (1, 2, None, 2)
assert stacked_inner.cache_info() == (0, 2, None, 2)
# the inner wrapper filled its own cache, so calling it directly is a hit
assert stacked_inner(1) == 2
assert stacked_inner.cache_info() == (1, 2, None, 2)
assert stacked_calls == [1, 2]

# a bounded inner cache still evicts on its own terms
bounded_inner = functools.lru_cache(maxsize=1)(_double_raw)
bounded_outer = functools.cache(bounded_inner)
assert bounded_outer(3) == 6
assert bounded_outer(4) == 8
assert bounded_inner.cache_info() == (0, 2, 1, 1)
assert bounded_inner(3) == 6
assert bounded_inner.cache_info() == (0, 3, 1, 1)


# === stacked wrappers are bounded, not a stack overflow ===
# Each layer dispatches on the interpreter's own call stack without pushing a
# Python frame, so Monty caps a chain of them at its native re-entry depth;
# CPython runs until its C stack gives out (see ./limitations/functools.md).
def stack_wrappers(depth):
    wrapped = _double_raw

    for _ in range(depth):
        wrapped = functools.cache(wrapped)
    return wrapped


assert stack_wrappers(5)(5) == 10

if sys.platform == 'monty':
    try:
        stack_wrappers(30)(6)
        assert False, 'expected the deep wrapper chain to raise'
    except RecursionError as exc:
        assert str(exc) == 'maximum recursion depth exceeded'
else:
    assert stack_wrappers(30)(6) == 12

# === errors ===
try:
    functools.lru_cache(5.0)
    assert False, 'expected lru_cache to fail'
except TypeError as exc:
    assert str(exc) == 'Expected first argument to be an integer, a callable, or None'

try:
    functools.lru_cache('x')
    assert False, 'expected lru_cache to fail'
except TypeError as exc:
    assert str(exc) == 'Expected first argument to be an integer, a callable, or None'

try:
    functools.lru_cache(1, 2, 3)
    assert False, 'expected lru_cache to fail'
except TypeError as exc:
    assert str(exc) == 'lru_cache() takes from 0 to 2 positional arguments but 3 were given'

try:
    functools.lru_cache(bogus=1)
    assert False, 'expected lru_cache to fail'
except TypeError as exc:
    assert str(exc) == "lru_cache() got an unexpected keyword argument 'bogus'"

try:
    functools.cache()
    assert False, 'expected cache to fail'
except TypeError as exc:
    assert str(exc) == "cache() missing 1 required positional argument: 'user_function'"

try:
    functools.cache(5)
    assert False, 'expected cache to fail'
except TypeError as exc:
    assert str(exc) == 'the first argument must be callable'

try:
    functools.cache(int, 5)
    assert False, 'expected cache to fail'
except TypeError as exc:
    assert str(exc) == 'cache() takes 1 positional argument but 2 were given'

try:
    functools.lru_cache(maxsize=2)()
    assert False, 'expected the decorator to fail'
except TypeError as exc:
    assert (
        str(exc) == "lru_cache.<locals>.decorating_function() missing 1 required positional argument: 'user_function'"
    )

try:
    functools.lru_cache(maxsize=2)(5)
    assert False, 'expected the decorator to fail'
except TypeError as exc:
    assert str(exc) == 'the first argument must be callable'

try:
    functools.lru_cache(maxsize=2)(int, str)
    assert False, 'expected the decorator to fail'
except TypeError as exc:
    assert str(exc) == 'lru_cache.<locals>.decorating_function() takes 1 positional argument but 2 were given'

try:
    double.cache_info(1)
    assert False, 'expected cache_info to fail'
except TypeError as exc:
    assert str(exc) == '_lru_cache_wrapper.cache_info() takes no arguments (1 given)'

try:
    double.bogus()
    assert False, 'expected the attribute lookup to fail'
except AttributeError as exc:
    assert str(exc) == "'functools._lru_cache_wrapper' object has no attribute 'bogus'"
