import copy
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
assert copy.copy(double) is double
assert copy.deepcopy(double) is double
assert functools.lru_cache()(user_function=int)('12') == 12

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

# Recency survives repeated eviction and hash-table compaction.
turnover_calls = []


@functools.lru_cache(maxsize=3)
def turnover(n):
    turnover_calls.append(n)
    return n


for n in range(3):
    assert turnover(n) == n
for n in range(3, 40):
    assert turnover(0) == 0
    assert turnover(n - 1) == n - 1
    assert turnover(n) == n
assert turnover_calls == list(range(40))
assert turnover.cache_info() == (74, 40, 3, 3)

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

# === eviction runs no user code ===
# Monty drops the least recently used entry by position, so a key's `__eq__`
# cannot re-enter the cache under an eviction already in flight. CPython pops
# that entry by key, so a colliding `__eq__` does run there (see
# ./limitations/functools.md).
reentrant_calls = []
reentrant_armed = [False]


class Colliding:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return 1

    def __eq__(self, other):
        # Only evicting Colliding(2) compares a stored Colliding(1) against it,
        # so this fills the cache mid-eviction or not at all.
        if reentrant_armed[0] and self.v == 1 and other.v == 2:
            reentrant_armed[0] = False
            reentrant(Colliding(5))
        return self.v == other.v


@functools.lru_cache(maxsize=3)
def reentrant(k):
    reentrant_calls.append(k.v)
    return k.v


reentrant(Colliding(1))
reentrant(Colliding(2))
reentrant(Colliding(3))
reentrant(Colliding(1))  # Colliding(2) is now the least recently used
reentrant_armed[0] = True
reentrant(Colliding(4))  # evicts Colliding(2)
# whatever ran while it was evicting, the cache never outgrows `maxsize`
assert reentrant.cache_info().currsize == 3
if sys.platform == 'monty':
    assert reentrant_calls == [1, 2, 3, 4]
    assert reentrant.cache_info() == (1, 4, 3, 3)
else:
    assert reentrant_calls == [1, 2, 3, 4, 5]
    assert reentrant.cache_info() == (1, 5, 3, 3)

# === decorator forms ===
# bare, parameterized, and `None` for unbounded
assert functools.lru_cache(lambda n: n * 2)(3) == 6
assert functools.lru_cache(lambda n: n * 2).cache_info() == (0, 0, 128, 0)
assert functools.lru_cache()(lambda n: n * 2)(3) == 6
assert functools.lru_cache(None)(lambda n: n).cache_info() == (0, 0, None, 0)
assert functools.lru_cache(maxsize=3, typed=True)(lambda n: n).cache_parameters() == {'maxsize': 3, 'typed': True}
assert functools.lru_cache(2**32)(int).cache_info().maxsize == 2**32

# === recursion ===


@functools.cache
def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)


assert fib(20) == 6765
assert fib.cache_info() == (18, 21, None, 21)


# A recursive miss may populate the same key before the outer call returns.
def recurse_once(maxsize):
    entered = []

    @functools.lru_cache(maxsize)
    def recursive(key):
        if not entered:
            entered.append(True)
            recursive(key)
            return 'outer'
        return 'inner'

    return recursive(1), recursive(1), recursive.cache_info()


assert recurse_once(None) == ('outer', 'outer', (1, 2, None, 1))
assert recurse_once(2) == ('outer', 'inner', (1, 2, 2, 1))

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

# A raising cached call must not disturb the caller's live operands.


def surrounding(start, step):
    total = start
    for n in range(2):
        try:
            total = total + boom(n) + step
        except ValueError:
            total = total + 10
    return total


assert surrounding(1, 100) == 21

# === cached callables other than functions ===
assert functools.cache(int)('10') == 10
assert functools.cache(functools.partial(pow, 2))(3) == 8

# a class caches the instance the call produced, not `__init__`'s `None`


class Boxed:
    def __init__(self, n):
        self.n = n


boxed_class = functools.cache(Boxed)
first_box = boxed_class(1)
assert first_box.n == 1
assert boxed_class(1) is first_box
assert boxed_class(2) is not first_box
assert boxed_class.cache_info() == (1, 2, None, 2)


class Empty:
    pass


empty_class = functools.cache(Empty)
only_empty = empty_class()
assert empty_class() is only_empty
assert empty_class.cache_info() == (1, 1, None, 1)


class Fussy:
    def __init__(self):
        raise ValueError('no')


fussy_class = functools.cache(Fussy)
for _ in range(2):
    try:
        fussy_class()
        assert False, 'expected the constructor to fail'
    except ValueError as exc:
        assert str(exc) == 'no'
assert fussy_class.cache_info() == (0, 2, None, 0)


class Returning:
    def __init__(self):
        return 1


# `__init__` returning non-`None` raises every time; nothing is cached
returning_class = functools.cache(Returning)
for _ in range(2):
    try:
        returning_class()
        assert False, 'expected the constructor to fail'
    except TypeError as exc:
        assert str(exc) == "__init__() should return None, not 'int'"
assert returning_class.cache_info() == (0, 2, None, 0)

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
# Each wrapper stores the result returned through its own continuation.


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


# === stacked wrappers use ordinary frames, not nested native dispatch ===
def stack_wrappers(depth):
    wrapped = _double_raw

    for _ in range(depth):
        wrapped = functools.cache(wrapped)
    return wrapped


assert stack_wrappers(5)(5) == 10

assert stack_wrappers(30)(6) == 12

# === Private helpers do not resolve through user globals or wrapper attributes ===
helpers = None
store = None
__monty_cache_miss = None
assert functools.cache(int)('13') == 13
try:
    double.store
    assert False, 'expected private helper lookup to fail'
except AttributeError as exc:
    assert str(exc) == "'functools._lru_cache_wrapper' object has no attribute 'store'"


# === Namespace-sensitive builtins see the caller, not the frozen continuation ===
def caller_namespace():
    marker = 42
    cached_locals = functools.cache(functools.cache(locals))
    assert cached_locals() == {'marker': marker, 'cached_locals': cached_locals}
    assert functools.cache(eval)('marker') == marker
    assert functools.reduce(eval, ['marker', None]) == marker
    return marker


assert caller_namespace() == 42
functools.cache(exec)('frozen_exec_result = 19')
assert eval('frozen_exec_result') == 19
scope = {'marker': 64}
exec("from functools import cache\nresult = cache(eval)('marker')", scope)
assert scope['result'] == 64

# === Disabled caches skip hashing and use ordinary frames ===
disabled = functools.lru_cache(0)(len)
assert disabled([1, 2]) == 2
assert disabled([1, 2]) == 2
assert disabled.cache_info() == (0, 2, 0, 0)
for _ in range(30):
    disabled = functools.lru_cache(0)(disabled)
assert disabled([]) == 0

# === errors ===
try:
    functools.cache(user_function=int)
    assert False, 'expected cache to reject keyword arguments'
except TypeError as exc:
    assert str(exc) == "cache() got some positional-only arguments passed as keyword arguments: 'user_function'"

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
