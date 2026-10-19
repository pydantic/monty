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
True
