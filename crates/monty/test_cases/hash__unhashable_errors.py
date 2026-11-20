# Errors raised when a set element or dict key will not hash. CPython wraps an
# exact `TypeError` from hashing as `cannot use 'T' as a set element (...)` /
# `... as a dict key (...)`; `hash()` and a few C paths raise it bare.
import functools
from collections import Counter, defaultdict, namedtuple
from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True)
class Frozen:
    x: object


class RaisesType:
    def __hash__(self):
        raise TypeError('boom')


class RaisesValue:
    def __hash__(self):
        raise ValueError('v')


class CountsHashes:
    def __init__(self):
        self.calls = 0

    def __hash__(self):
        self.calls += 1
        return 1


class FailsRehash:
    def __init__(self):
        self.calls = 0

    def __hash__(self):
        self.calls += 1
        if self.calls > 1:
            raise TypeError('rehashed')
        return 1


class ThenRaise:
    def __init__(self, *items):
        self.items = list(items)

    def __iter__(self):
        return self

    def __next__(self):
        if self.items:
            return self.items.pop(0)
        return 1 / 0


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

# === Dict view & and isdisjoint probe the dict, wrapping as a dict key ===
d = {1: 2, 3: [4]}
k = d.keys()
i = d.items()
assert err(lambda: k & [[1]]) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: [[1]] & k) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: k & [RaisesType()]) == "TypeError: cannot use 'RaisesType' as a dict key (boom)"
# each item is probed as it is read, so an unhashable one raises before the iterator fails
assert err(lambda: k & ThenRaise([1])) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: k & ThenRaise(5)) == 'ZeroDivisionError: division by zero'
assert (
    err(lambda: k.isdisjoint(ThenRaise([1]))) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
)
assert err(lambda: k.isdisjoint([[1]])) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: k.isdisjoint(d.values())) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: i & [([1], 1)]) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: i.isdisjoint([([1], 1)])) == "TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"
assert err(lambda: k & i) == "TypeError: cannot use 'tuple' as a dict key (unhashable type: 'list')"

# === A matched item is added to the result set, which hashes it as a set element ===
assert err(lambda: i & [(3, [4])]) == "TypeError: cannot use 'tuple' as a set element (unhashable type: 'list')"
assert err(lambda: [(3, [4])] & i) == "TypeError: cannot use 'tuple' as a set element (unhashable type: 'list')"
assert err(lambda: i & {3: [4]}.items()) == "TypeError: cannot use 'tuple' as a set element (unhashable type: 'list')"

# === An exact set at least as large as the view runs set.intersection(view) ===
assert err(lambda: i & {(1, 2), (5, 6), (7, 8)}) == "TypeError: unhashable type: 'list'"
assert i & frozenset({(1, 2), (5, 6), (7, 8)}) == {(1, 2)}
assert i & {(1, 2)} == {(1, 2)}

# === Probing stops at the first match and never hashes the view's values ===
assert k.isdisjoint([1, [1]]) is False
assert i.isdisjoint([(3, [4])]) is False
assert i.isdisjoint({(1, 2), (5, 6), (7, 8)}) is False
assert i.isdisjoint({(9, 9)}) is True
assert k.isdisjoint(frozenset({7, 8, 9})) is True
assert k.isdisjoint(frozenset({1, 8, 9})) is False
assert i & [[1]] == set()
assert i & [(1, 2)] == {(1, 2)}
assert i & k == set()
assert k.isdisjoint(k) is False
assert {}.keys().isdisjoint({}.keys()) is True
assert err(lambda: k & 5) == "TypeError: 'int' object is not iterable"
assert err(lambda: k.isdisjoint(5)) == "TypeError: 'int' object is not iterable"

# === A failing hash runs each __hash__ once ===
counted = CountsHashes()
assert err(lambda: {(counted, [1])}) == "TypeError: cannot use 'tuple' as a set element (unhashable type: 'list')"
assert counted.calls == 1
counted = CountsHashes()
assert err(lambda: hash((counted, [1]))) == "TypeError: unhashable type: 'list'"
assert counted.calls == 1
counted = CountsHashes()
assert err(lambda: {NT(counted, [1]): 1}) == "TypeError: cannot use 'NT' as a dict key (unhashable type: 'list')"
assert counted.calls == 1
counted = CountsHashes()
assert err(lambda: cached((counted, [1]))) == "TypeError: unhashable type: 'list'"
assert counted.calls == 1

# === A frozenset hashes from its stored element hashes, never re-hashing a member ===
counted = CountsHashes()
fs = frozenset([counted])
assert hash(fs) == hash(frozenset([1]))
assert counted.calls == 1
rehash = FailsRehash()
fs = frozenset([rehash])
assert {fs} == {fs}
assert rehash.calls == 1

# === Named tuples hash by element, so Literal drops a repeated one ===
assert len(Literal[NT(1, 2), NT(1, 2)].__args__) == 1

# === Counter hashes each counted element once ===
counted = CountsHashes()
assert Counter([counted, counted])[counted] == 2
assert counted.calls == 3
counted = CountsHashes()
assert err(lambda: Counter([(counted, [1])])) == "TypeError: unhashable type: 'list'"
assert counted.calls == 1

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
