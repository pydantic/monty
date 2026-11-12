# Errors raised when a set element or dict key will not hash. CPython wraps an
# exact `TypeError` from hashing as `cannot use 'T' as a set element (...)` /
# `... as a dict key (...)`; `hash()` and a few C paths raise it bare.
import functools
from collections import Counter, defaultdict, namedtuple
from dataclasses import dataclass


@dataclass(frozen=True)
class Frozen:
    x: object


class RaisesType:
    def __hash__(self):
        raise TypeError('boom')


class RaisesValue:
    def __hash__(self):
        raise ValueError('v')


NT = namedtuple('NT', 'a b')


def err(f):
    try:
        f()
    except Exception as e:
        return f'{type(e).__name__}: {e}'
    return 'no error'


def setitem(d, k):
    d[k] = 1


# === Unhashable value as a set element ===
assert err(lambda: {[1]}) == "TypeError: cannot use 'list' as a set element (unhashable type: 'list')"
assert err(lambda: {([1],)}) == "TypeError: cannot use 'tuple' as a set element (unhashable type: 'list')"
assert err(lambda: {(1, (2, [3]))}) == "TypeError: cannot use 'tuple' as a set element (unhashable type: 'list')"
assert err(lambda: {NT(1, [1])}) == "TypeError: cannot use 'NT' as a set element (unhashable type: 'list')"

# === Unhashable value as a dict key names the innermost culprit ===
assert err(lambda: {[1]: 1}) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: {([1],): 1}) == "TypeError: cannot use 'tuple' as a dict key (unhashable type: 'list')"
assert err(lambda: {((1, [1]),): 1}) == "TypeError: cannot use 'tuple' as a dict key (unhashable type: 'list')"
assert err(lambda: {NT(1, [1]): 1}) == "TypeError: cannot use 'NT' as a dict key (unhashable type: 'list')"
assert err(lambda: dict.fromkeys([([1],)])) == "TypeError: cannot use 'tuple' as a dict key (unhashable type: 'list')"

# === A raised TypeError is wrapped ===
assert err(lambda: {Frozen([1])}) == "TypeError: cannot use 'Frozen' as a set element (unhashable type: 'list')"
assert err(lambda: {Frozen([1]): 1}) == "TypeError: cannot use 'Frozen' as a dict key (unhashable type: 'list')"
assert err(lambda: {RaisesType()}) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: {RaisesType(): 1}) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: {(RaisesType(),)}) == "TypeError: cannot use 'tuple' as a set element (boom)"

# === Other exceptions pass through ===
assert err(lambda: {RaisesValue()}) == 'ValueError: v'
assert err(lambda: {RaisesValue(): 1}) == 'ValueError: v'

# === Every set insertion and lookup wraps ===
s = {1}
assert err(lambda: frozenset([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: set([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: {x for x in [RaisesType()]}) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: s.add(RaisesType())) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: s.add([1])) == "TypeError: cannot use 'list' as a set element (unhashable type: 'list')"
assert err(lambda: s.discard(RaisesType())) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: s.remove(RaisesType())) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: RaisesType() in s) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: Frozen([1]) in s) == "TypeError: cannot use 'Frozen' as a set element (unhashable type: 'list')"
assert err(lambda: s.update([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: s.union([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: s.difference([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: s.symmetric_difference([RaisesType()])) == (
    "TypeError: cannot use 'RaisesType' as a set element (boom)"
)
assert err(lambda: s.issuperset([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a set element (boom)"
assert err(lambda: s.isdisjoint([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a set element (boom)"

# === intersection and issubset hash an iterable's items bare ===
assert err(lambda: s.intersection([RaisesType()])) == 'TypeError: boom'
assert err(lambda: s.intersection([([1],)])) == "TypeError: unhashable type: 'list'"
assert err(lambda: s.issubset([[1]])) == "TypeError: unhashable type: 'list'"
assert err(lambda: frozenset(s).issubset([RaisesType()])) == 'TypeError: boom'

# === Every dict insertion and lookup wraps ===
d = {1: 2}
assert err(lambda: setitem(d, RaisesType())) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: d[RaisesType()]) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: d[Frozen([1])]) == "TypeError: cannot use 'Frozen' as a dict key (unhashable type: 'list')"
assert err(lambda: d.get(RaisesType())) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: d.get([1])) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: d.setdefault(RaisesType(), 1)) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: d.pop(RaisesType(), None)) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: RaisesType() in d) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: ([1],) in d) == "TypeError: cannot use 'tuple' as a dict key (unhashable type: 'list')"
assert err(lambda: dict.fromkeys([RaisesType()])) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: dict([(RaisesType(), 1)])) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: {**{RaisesType(): 1}}) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: {x: 1 for x in [RaisesType()]}) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: {}.update([(RaisesType(), 1)])) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: defaultdict(int)[RaisesType()]) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"

# === pop on an empty dict returns before hashing ===
assert {}.pop(RaisesType(), 'fallback') == 'fallback'

# === hash() raises bare, naming the innermost culprit ===
assert err(lambda: hash(Frozen([1]))) == "TypeError: unhashable type: 'list'"
assert err(lambda: hash(([1],))) == "TypeError: unhashable type: 'list'"
assert err(lambda: hash((1, (2, [3])))) == "TypeError: unhashable type: 'list'"
assert err(lambda: hash(Frozen(([1],)))) == "TypeError: unhashable type: 'list'"

# === lru_cache keys hash bare ===
cached = functools.lru_cache()(lambda x: x)
assert err(lambda: cached([1])) == "TypeError: unhashable type: 'list'"
assert err(lambda: cached(([1],))) == "TypeError: unhashable type: 'list'"
assert err(lambda: cached(x=[1])) == "TypeError: unhashable type: 'list'"
assert err(lambda: cached(RaisesType())) == 'TypeError: boom'

# === Counter counts elements with a bare hash, but subtract wraps ===
assert err(lambda: Counter([RaisesType()])) == 'TypeError: boom'
assert err(lambda: Counter([[1]])) == "TypeError: unhashable type: 'list'"
assert err(lambda: Counter().update([RaisesType()])) == 'TypeError: boom'
assert err(lambda: Counter()[RaisesType()]) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
assert err(lambda: Counter().subtract([[1]])) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"

# === The wrapped error chains to the exception being handled, not the hash error ===
try:
    {RaisesType()}
except TypeError as e:
    assert e.__context__ is None, 'unexpected context'
try:
    try:
        raise KeyError('outer')
    except KeyError:
        {RaisesType()}
except TypeError as e:
    assert repr(e.__context__) == "KeyError('outer')"
