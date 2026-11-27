import copy
import csv
import json
import math
import random
import statistics
import tomllib
from collections import Counter, deque, namedtuple
from decimal import Decimal

# === Scalar comparisons and conversions ===
a = float('nan')
b = float('nan')
assert a != a
assert a != b
assert float(a) is a
assert +a is a
assert copy.copy(a) is a
assert copy.deepcopy(a) is a
assert math.nan is math.nan
assert type(a) is float
assert isinstance(a, float)
assert bool(a) is True
assert a.is_integer() is False


class FloatResult:
    def __float__(self):
        return a


assert float(FloatResult()) is a

# === Shared NaN equality and distinct NaN ordering ===
One = namedtuple('One', ['value'])
for container in (list, tuple, deque, lambda items: One(items[0])):
    left = container([a])
    shared = container([a])
    separate = container([b])
    assert left == shared
    assert (left < separate) is False
    assert (left <= separate) is False
    assert (left > separate) is False
    assert (left >= separate) is False

assert a in [a]
assert [a].count(a) == 1
assert [a].index(a) == 0
assert ([a, 0] < [b, 1]) is False


class EqualToNaN:
    def __eq__(self, other):
        return math.isnan(other)


assert [a, 0] < [EqualToNaN(), 1]
assert [EqualToNaN(), 0] < [a, 1]

nan_keys = {a: 'value'}
assert nan_keys[a] == 'value'
assert len({a, a}) == 1
assert len(frozenset([a, a])) == 1
assert hash(a) == hash(a)
assert Counter(first=a, second=a, third=a).most_common(2) == [('first', a), ('second', a)]

# === NaN-producing paths ===
produced = [
    float(b'nan'),
    float('-nan'),
    float.fromhex('nan'),
    float.fromhex('NaN'),
    float.fromhex('-NaN'),
    a.fromhex('nan'),
    -a,
    abs(a),
    round(a, 3),
    a + 1,
    1 + a,
    a - 1,
    1 - a,
    a * 2,
    2 * a,
    a / 2,
    2 / a,
    a // 2,
    2 // a,
    a % 2,
    2 % a,
    a**2,
    2**a,
    a + 2**100,
    2**100 + a,
    a * 2**100,
    2**100 * a,
    a + True,
    True + a,
    float('inf') - float('inf'),
    float('inf') * 0.0,
    0.0 * float('inf'),
    float('inf') / float('inf'),
    *divmod(a, 2),
    *divmod(2, a),
    math.sqrt(a),
    math.sin(a),
    math.nextafter(a, 1),
    math.ulp(a),
    math.ldexp(a, 1),
    *math.modf(a),
    math.frexp(a)[0],
    math.fsum([a]),
    math.sumprod([a], [1]),
    sum([a, 1]),
    statistics.mean([a]),
    statistics.fmean([a]),
    statistics.fmean([a], [1]),
    statistics.pvariance([a]),
    float(Decimal('NaN')),
    Decimal('NaN').__float__(),
    complex(a, 0).real,
    complex(0, a).imag,
    abs(complex(a, 0)),
    json.loads('NaN'),
    tomllib.loads('value = nan')['value'],
    next(csv.reader(['nan'], quoting=csv.QUOTE_NONNUMERIC))[0],
    random.Random(0).uniform(a, 1),
]
for value in produced:
    assert type(value) is float
    assert math.isnan(value) is True

# === Numeric consumers ===
for other in (0, True, 1.0, 2**100, Decimal(1), complex(1, 0)):
    assert a != other
    assert other != a
for other in (0, True, 1.0, 2**100):
    assert (a < other) is False
    assert (other < a) is False
    assert (a <= other) is False
    assert (other <= a) is False
assert math.copysign(1, float('-nan')) == -1.0
assert str(a) == 'nan'
assert repr(a) == 'nan'
assert f'{a:.2f}' == 'nan'
assert '%f' % a == 'nan'
assert json.dumps(a) == 'NaN'
assert a not in range(3)
