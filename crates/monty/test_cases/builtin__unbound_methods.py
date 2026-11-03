from datetime import date, time, timedelta, timezone
import re

items = []
list.append(items, 2)
list.extend(items, [3, 1])
list.sort(items, reverse=True)
assert items == [3, 2, 1]
assert list.count(items, 2) == 1
assert tuple.index((1, 2, 3), 2) == 1
assert complex.conjugate(1 + 2j) == 1 - 2j
for text in ['foo', ''.join(['f', 'oo'])]:
    assert str.upper(text) == 'FOO'
    assert str.replace(text, 'o', 'a', 1) == 'fao'
assert str.split('a,b,c', ',', maxsplit=1) == ['a', 'b,c']
assert str.format('{0}:{value}', 'key', value=42) == 'key:42'
assert str.join(', ', ['a', 'b']) == 'a, b'
assert date.isoformat(date(2024, 1, 2)) == '2024-01-02'
assert date.replace(date(2024, 1, 2), year=2025) == date(2025, 1, 2)
assert time.isoformat(time(1, 2, 3), timespec='minutes') == '01:02'
assert timedelta.total_seconds(timedelta(seconds=3)) == 3.0
assert timezone.utcoffset(timezone.utc, None) == timedelta(0)
pattern = re.compile('(a)')
match = re.Pattern.search(pattern, 'a')
assert re.Match.group(match, 1) == 'a'
assert re.Pattern.sub(pattern, 'b', 'a') == 'b'
True

from datetime import date

for invalid in [
    lambda: str.upper(),
    lambda: str.upper(self=['foo']),
    lambda: str.upper([], [1]),
    lambda: str.upper('foo', [1]),
    lambda: str.upper('foo', extra=[1]),
    lambda: str.replace('foo'),
    lambda: list.append((), [1]),
    lambda: tuple.count([], [1]),
    lambda: date.isoformat(None),
]:
    try:
        invalid()
    except TypeError:
        pass
    else:
        raise AssertionError('invalid method call succeeded')
try:
    str.upper()
except TypeError as exc:
    assert str(exc) == 'unbound method str.upper() needs an argument'
try:
    str.upper(1)
except TypeError as exc:
    assert str(exc) == "descriptor 'upper' for 'str' objects doesn't apply to a 'int' object"
events = []


def argument():
    events.append(1)
    return [1]


try:
    str.upper(1, argument())
except TypeError:
    pass
assert events == [1]
assert str.upper('foo') == 'FOO'
True

from collections import namedtuple
from datetime import date, datetime, timezone

Point = namedtuple('Point', ['x', 'y'])
assert tuple.count(Point(1, 1), 1) == 2
assert tuple.index(Point(1, 2), 2) == 1
dt = datetime(2024, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
assert date.isoformat(dt) == '2024-01-02'
assert date.strftime(dt, '%H:%M:%S %z') == '03:04:05 +0000'
assert date.weekday(dt) == 1
assert date.isoweekday(dt) == 2
assert date.replace(dt, year=2025) == datetime(2025, 1, 2)

from collections import Counter, defaultdict, deque
from datetime import datetime
from functools import lru_cache
from hashlib import sha256, shake_128
from pathlib import Path
from random import Random

mapping = {}
dict.update(mapping, a=1)
assert dict.get(mapping, 'a') == 1
assert dict.pop(mapping, 'a') == 1
counter = Counter(a=1)
Counter.update(counter, a=2)
assert counter['a'] == 3
dict.update(counter, a=8)
assert counter['a'] == 8
assert Counter.total(counter) == 8
factory_mapping = defaultdict(list)
assert defaultdict.__missing__(factory_mapping, 'x') == []
assert type(factory_mapping.fromkeys(['y'])) is defaultdict
assert dict.fromkeys(['a']) == {'a': None}
assert type(defaultdict.fromkeys(['a'])) is defaultdict
for owner in [dict, defaultdict, Counter]:
    try:
        owner.get([], 'key')
    except TypeError:
        pass
    else:
        raise AssertionError('wrong receiver accepted')

values = set()
set.add(values, 1)
assert set.union(values, [2]) == {1, 2}
assert frozenset.union(frozenset([1]), [2]) == frozenset([1, 2])
mapping['a'] = 1
assert type(mapping.keys()).isdisjoint(mapping.keys(), ['a']) is False
assert type(mapping.items()).isdisjoint(mapping.items(), [('a', 1)]) is False
assert type(mapping.keys()).isdisjoint(mapping.keys(), ['b']) is True
assert type(mapping.items()).isdisjoint(mapping.items(), [('a', 2)]) is True
queue = deque([1, 2])
deque.appendleft(queue, 0)
assert deque.pop(queue) == 2
assert list(queue) == [0, 1]
for data in [b'foo', b''.join([b'f', b'oo'])]:
    assert bytes.upper(data) == b'FOO'
    assert bytes.replace(data, b'o', b'a', 1) == b'fao'
assert bytes.fromhex('666f6f') == b'foo'
assert b''.fromhex('666f6f') == b'foo'
assert datetime.isoformat(datetime(2024, 1, 2, 3, 4)) == '2024-01-02T03:04:00'
assert datetime.fromisoformat('2024-01-02T03:04:00') == datetime(2024, 1, 2, 3, 4)
assert Path.joinpath(Path('/a'), 'b') == Path('/a/b')
rng = Random(42)
assert Random.getrandbits(rng, 8) == 163
hash_value = sha256()
type(hash_value).update(hash_value, b'foo')
assert type(hash_value).digest(hash_value) == sha256(b'foo').digest()
xof = shake_128(b'foo')
assert type(xof).digest(xof, 4) == shake_128(b'foo').digest(4)


@lru_cache()
def cached(x):
    return x + 1


assert cached(1) == 2
assert type(cached).cache_info(cached).currsize == 1
assert cached.__wrapped__(1) == 2


shared = []


def make_factory(value):
    def factory():
        return value

    return factory


factory = make_factory(shared)
for source in [Counter(a=shared), defaultdict(factory, a=shared)]:
    plain = dict.copy(source)
    specialized = source.copy()
    assert type(plain) is dict
    assert type(specialized) is type(source)
    assert plain == source
    assert specialized == source
    assert plain['a'] is shared
    assert specialized['a'] is shared
    if type(source) is defaultdict:
        assert specialized.default_factory is factory
        assert source.default_factory is factory
assert list.__class_getitem__(int) == list[int]

for zone in [None, timezone(timedelta(hours=2), 'X')]:
    dt = datetime(2024, 1, 2, 3, 4, 5, 123456, tzinfo=zone)
    assert dt.time() == time(3, 4, 5, 123456)
    assert dt.time().tzinfo is None
    assert dt.timetz() == time(3, 4, 5, 123456, tzinfo=zone)
    assert dt.timetz().tzinfo is zone
    assert datetime.time(dt) == dt.time()
    assert datetime.timetz(dt) == dt.timetz()

True
