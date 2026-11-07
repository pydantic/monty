import re
from annotationlib import ForwardRef
from collections import Counter, defaultdict, deque, namedtuple
from datetime import date, datetime, time, timedelta, timezone
from decimal import Context, Decimal, Inexact, getcontext, setcontext
from functools import lru_cache
from hashlib import sha256, shake_128
from pathlib import Path
from random import Random

# === Unbound native methods ===
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
for invalid, expected_error in [
    (lambda: str.upper(), 'unbound method str.upper() needs an argument'),
    (lambda: str.upper(self=['foo']), 'unbound method str.upper() needs an argument'),
    (lambda: str.upper([], [1]), "descriptor 'upper' for 'str' objects doesn't apply to a 'list' object"),
    (lambda: str.upper('foo', [1]), 'str.upper() takes no arguments (1 given)'),
    (lambda: str.upper('foo', extra=[1]), 'str.upper() takes no keyword arguments'),
    (lambda: str.replace('foo'), 'replace() takes at least 2 positional arguments (0 given)'),
    (lambda: list.append((), [1]), "descriptor 'append' for 'list' objects doesn't apply to a 'tuple' object"),
    (lambda: tuple.count([], [1]), "descriptor 'count' for 'tuple' objects doesn't apply to a 'list' object"),
    (
        lambda: date.isoformat(None),
        "descriptor 'isoformat' for 'datetime.date' objects doesn't apply to a 'NoneType' object",
    ),
]:
    try:
        invalid()
    except TypeError as exc:
        assert str(exc) == expected_error
    else:
        assert False, 'invalid method call succeeded'
try:
    str.upper()
except TypeError as exc:
    assert str(exc) == 'unbound method str.upper() needs an argument'
else:
    assert False, 'expected exception'
try:
    str.upper(1)
except TypeError as exc:
    assert str(exc) == "descriptor 'upper' for 'str' objects doesn't apply to a 'int' object"
else:
    assert False, 'expected exception'
events = []


def argument():
    events.append(1)
    return [1]


try:
    str.upper(1, argument())
except TypeError as exc:
    assert str(exc) == "descriptor 'upper' for 'str' objects doesn't apply to a 'int' object"
else:
    assert False, 'expected exception'
assert events == [1]
assert str.upper('foo') == 'FOO'
Point = namedtuple('Point', ['x', 'y'])
assert tuple.count(Point(1, 1), 1) == 2
assert tuple.index(Point(1, 2), 2) == 1
dt = datetime(2024, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
assert date.isoformat(dt) == '2024-01-02'
assert date.strftime(dt, '%H:%M:%S %z') == '03:04:05 +0000'
assert date.weekday(dt) == 1
assert date.isoweekday(dt) == 2
assert date.replace(dt, year=2025) == datetime(2025, 1, 2)
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
    except TypeError as exc:
        assert str(exc) == "descriptor 'get' for 'dict' objects doesn't apply to a 'list' object"
    else:
        assert False, 'wrong receiver accepted'
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

# === Bound methods and class methods (1) ===
items = [2, 1]
items.append(3)
items.sort()
assert items.count(3) == 1
assert (1, 2).index(2) == 1
assert complex(1, 2).conjugate() == complex(1, -2)
assert date(2024, 1, 2).isoformat() == '2024-01-02'
assert time(1, 2, 3).isoformat() == '01:02:03'
assert timedelta(seconds=2).total_seconds() == 2.0
assert timezone.utc.utcoffset(None) == timedelta(0)
pattern = re.compile('(a)')
match = pattern.search('a')
assert match.group(1) == 'a'
assert pattern.sub('b', 'a') == 'b'
assert re.compile('a').split('a') == ['', '']
try:
    items.append()
except TypeError as exc:
    assert str(exc) == 'list.append() takes exactly one argument (0 given)'
else:
    assert False, 'append should require an argument'
assert pattern.search('a').group() == 'a'
items

# === Bound methods and class methods (2) ===
assert date.fromisoformat('2024-01-02') == date(2024, 1, 2)
assert time.fromisoformat('01:02:03') == time(1, 2, 3)
assert complex.from_number(2) == complex(2)
try:
    date.today(1)
except TypeError as exc:
    assert str(exc) == 'date.today() takes no arguments (1 given)'
else:
    assert False, 'today should reject arguments'

# === String storage representations (1) ===
for text in ['AbC def', ''.join(['AbC ', 'def'])]:
    assert text.upper() == 'ABC DEF'
    assert text.lower() == 'abc def'
    assert text.split() == ['AbC', 'def']
    assert text.find('def') == 4
    assert text.replace('def', 'ghi') == 'AbC ghi'
    assert text.encode() == b'AbC def'
    assert text.isascii()
    assert text.title() == 'Abc Def'
for separator in [', ', ''.join([',', ' '])]:
    assert separator.join(['one', 'two']) == 'one, two'
for template in ['{}: {name}', ''.join(['{}: ', '{name}'])]:
    assert template.format('key', name='value') == 'key: value'
assert 'Straße'.casefold() == 'strasse'
assert ''.join(['Stra', 'ße']).casefold() == 'strasse'

# === String storage representations (2) ===
for text in ['abc', ''.join(['a', 'bc'])]:
    try:
        text.upper([1])
    except TypeError as exc:
        assert str(exc) == 'str.upper() takes no arguments (1 given)'
    else:
        assert False, 'upper accepted an argument'
    try:
        text.split([1])
    except TypeError as exc:
        assert str(exc) == 'must be str or None, not list'
    else:
        assert False, 'split accepted a list'
    try:
        text.rsplit([1])
    except TypeError as exc:
        assert str(exc) == 'must be str or None, not list'
    else:
        assert False, 'rsplit accepted a list'
    try:
        text.missing_method([1])
    except AttributeError as exc:
        assert str(exc) == "'str' object has no attribute 'missing_method'"
    else:
        assert False, 'missing method succeeded'
    try:
        text.append([1])
    except AttributeError as exc:
        assert str(exc) == "'str' object has no attribute 'append'"
    else:
        assert False, 'non-string method succeeded'
    assert text.upper() == 'ABC'

# === Decimal and annotation methods ===
setcontext(Context())
value = Decimal('12.5')
assert value.adjusted() == Decimal.adjusted(value) == 1
assert Decimal.quantize(value, Decimal('1')) == Decimal('12')
assert Decimal.from_float(0.5) == Decimal('0.5')
assert value.from_float(0.5) == Decimal('0.5')
assert Decimal.from_number(5) == Decimal(5)
assert Decimal.as_tuple(value) == value.as_tuple()
assert value.__copy__() is value
context = Context(prec=4)
assert Context.add(context, value, Decimal('1.5')) == Decimal('14')
assert context.create_decimal('1.234567') == Decimal('1.235')
assert context.Etiny() == Context.Etiny(context)
assert context.Etop() == Context.Etop(context)
assert context.__copy__().prec == context.prec
assert Context.copy(context).prec == 4
context.clear_flags()
assert not context.flags[Inexact]
context.divide(Decimal(1), Decimal(3))
assert context.flags[Inexact]
assert context.flags.get(Inexact)
assert type(context.flags).get(context.flags, Inexact)
assert len(context.flags.copy()) == len(context.flags)
assert len(context.flags.keys()) == len(context.flags.values())
try:
    context.flags.pop(Inexact)
except ValueError as exc:
    assert str(exc) == 'signal keys cannot be deleted'
else:
    assert False, 'signal deletion succeeded'
assert date.fromtimestamp(1700000000).year == 2023
assert date(2024, 1, 2).fromtimestamp(1700000000) == date.fromtimestamp(1700000000)
assert ForwardRef('int').evaluate() is int
assert ForwardRef.evaluate(ForwardRef('int')) is int
for text in ['a', ''.join(['ab', 'cd']), ''.join(['longer than ', 'fifteen bytes'])]:
    assert str.upper(text) == text.upper()
assert 'abc'.find('', 4) == -1
assert not 'abc'.startswith('', 4)
assert not 'abc'.endswith('', 4)

# === Descriptor type names and Counter mappings ===
try:
    defaultdict.copy([])
except TypeError as exc:
    assert str(exc) == "descriptor 'copy' for 'collections.defaultdict' objects doesn't apply to a 'list' object"
else:
    assert False, 'defaultdict.copy accepted a list'
factory_copy = defaultdict.copy(defaultdict(list, a=[1]))
assert factory_copy.default_factory is list
assert factory_copy == {'a': [1]}
assert Counter.most_common({'a': 2, 'b': 1}) == [('a', 2), ('b', 1)]
assert Counter.total({'a': 2, 'b': 3}) == 5
assert list(Counter.elements({'a': 2})) == ['a', 'a']
assert Counter.copy([1, 2]) == [1, 2]
assert Counter.copy(42) == 42
for receiver in [[], [1], 42, None]:
    assert Counter.update(receiver) is None
    assert Counter.subtract(receiver) is None
    assert Counter.update(receiver, None) is None
    assert Counter.subtract(receiver, None) is None
    assert Counter.update(receiver, **{}) is None
    assert Counter.subtract(receiver, None, **{}) is None
original_counter = Counter(a=[1])
copied_counter = Counter.copy(original_counter)
assert type(copied_counter) is Counter
assert copied_counter['a'] is original_counter['a']
try:
    Counter.copy(defaultdict(list))
except TypeError as exc:
    assert str(exc) == 'first argument must be callable or None'
else:
    assert False, 'defaultdict constructor requires a callable factory'
counts = {'a': 2}
Counter.update(counts, {'a': 1})
assert counts == {'a': 3}
Counter.subtract(counts, {'a': 1})
assert counts == {'a': 2}
Counter.update(counts, a=1)
assert counts == {'a': 1}
try:
    Counter.subtract(counts, {'a': 1}, b=[1])
except AttributeError as exc:
    assert str(exc) == "'dict' object has no attribute 'subtract'"
else:
    assert False, 'dict has no subtract method'
assert counts == {'a': 0}
bad_counts = {'a': 'bad'}
try:
    Counter.update(bad_counts, {'a': 1}, b=[1])
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'int' and 'str'"
else:
    assert False, 'counts cannot add int and str'
assert bad_counts == {'a': 'bad'}
payload = [1]
keyword_counts = {'a': 2}
Counter.update(keyword_counts, a=payload)
assert keyword_counts['a'] is payload
for invalid, expected_error in [
    (lambda: date.isoformat(1), "descriptor 'isoformat' for 'datetime.date' objects doesn't apply to a 'int' object"),
    (
        lambda: datetime.isoformat(1),
        "descriptor 'isoformat' for 'datetime.datetime' objects doesn't apply to a 'int' object",
    ),
    (
        lambda: Decimal.adjusted(1),
        "descriptor 'adjusted' for 'decimal.Decimal' objects doesn't apply to a 'int' object",
    ),
    (lambda: Counter.get([], 'a'), "descriptor 'get' for 'dict' objects doesn't apply to a 'list' object"),
    (lambda: Counter.most_common([]), "'list' object has no attribute 'items'"),
    (lambda: Counter.total(1), "'int' object has no attribute 'values'"),
    (lambda: Counter.elements(1), "'int' object has no attribute 'items'"),
    (lambda: Counter.total(), "Counter.total() missing 1 required positional argument: 'self'"),
    (
        lambda: Counter.update({}, {'a': 1}),
        'super(type, obj): obj (instance of dict) is not an instance or subtype of type (Counter).',
    ),
]:
    try:
        invalid()
    except (TypeError, AttributeError) as exc:
        assert str(exc) == expected_error
    else:
        assert False, 'expected invalid receiver error'
assert datetime.min == datetime(1, 1, 1)
assert datetime.max == datetime(9999, 12, 31, 23, 59, 59, 999999)

# === Positional date.replace arguments ===
for receiver in [date(2024, 6, 15), datetime(2024, 6, 15, 10, 20, 30, 123, tzinfo=timezone.utc)]:
    result_type = type(receiver)
    assert date.replace(receiver, 2025) == result_type(2025, 6, 15)
    assert date.replace(receiver, 2025, 3) == result_type(2025, 3, 15)
    assert date.replace(receiver, 2025, 3, 4) == result_type(2025, 3, 4)
    assert date.replace(receiver, 2025, month=3, day=4) == result_type(2025, 3, 4)
    try:
        date.replace(receiver, 2025, year=2026)
    except TypeError as exc:
        assert str(exc) == "argument for replace() given by name ('year') and position (1)"
    else:
        assert False, 'replace accepted a duplicate year'
    try:
        date.replace(receiver, 2025, 3, 4, 5)
    except TypeError as exc:
        assert str(exc) == 'replace() takes at most 3 arguments (4 given)'
    else:
        assert False, 'replace accepted four positional arguments'
assert date(2024, 6, 15).replace(2025, 3, day=4) == date(2025, 3, 4)

# === Argument validation ===
try:
    [].reverse([1])
except (TypeError, ValueError) as exc:
    assert str(exc) == 'list.reverse() takes no arguments (1 given)'
else:
    assert False, 'expected argument error'
try:
    [].clear([1])
except (TypeError, ValueError) as exc:
    assert str(exc) == 'list.clear() takes no arguments (1 given)'
else:
    assert False, 'expected argument error'
try:
    [].copy([1])
except (TypeError, ValueError) as exc:
    assert str(exc) == 'list.copy() takes no arguments (1 given)'
else:
    assert False, 'expected argument error'
try:
    date(2024, 1, 2).weekday([1])
except (TypeError, ValueError) as exc:
    assert str(exc) == 'date.weekday() takes no arguments (1 given)'
else:
    assert False, 'expected argument error'
try:
    date(2024, 1, 2).isoweekday([1])
except (TypeError, ValueError) as exc:
    assert str(exc) == 'date.isoweekday() takes no arguments (1 given)'
else:
    assert False, 'expected argument error'
try:
    timedelta(seconds=1).total_seconds([1])
except (TypeError, ValueError) as exc:
    assert str(exc) == 'timedelta.total_seconds() takes no arguments (1 given)'
else:
    assert False, 'expected argument error'
try:
    date(2024, 1, 31).replace(month=2)
except (TypeError, ValueError) as exc:
    assert str(exc) == 'day 31 must be in range 1..29 for month 2 in year 2024'
else:
    assert False, 'expected argument error'
try:
    date(2024, 1, 2).replace(year=0)
except (TypeError, ValueError) as exc:
    assert str(exc) == 'year must be in 1..9999, not 0'
else:
    assert False, 'expected argument error'
try:
    time(1, 2, 3).isoformat(timespec='invalid')
except (TypeError, ValueError) as exc:
    assert str(exc) == 'Unknown timespec value'
else:
    assert False, 'expected argument error'
