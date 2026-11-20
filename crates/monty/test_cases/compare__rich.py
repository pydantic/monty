from collections import Counter, deque, namedtuple
from datetime import date, datetime, time, timezone

# === Independent operators preserve arbitrary results ===
results = [[0], [], {'le': 1}, ('gt',), None, 'ne']


class Rich:
    def __eq__(self, other):
        return results[0]

    def __lt__(self, other):
        return results[1]

    def __le__(self, other):
        return results[2]

    def __gt__(self, other):
        return results[3]

    def __ge__(self, other):
        return results[4]

    def __ne__(self, other):
        return results[5]


rich = Rich()
assert (rich == rich) is results[0]
assert (rich < rich) is results[1]
assert (rich <= rich) is results[2]
assert (rich > rich) is results[3]
assert (rich >= rich) is results[4]
assert (rich != rich) is results[5]
assert rich == rich
assert not (rich < rich), 'false rich results must be truth-tested'
assert rich <= rich
assert rich > rich
assert not (rich >= rich), 'None is a valid rich result, not NotImplemented'
assert rich != rich
assert (rich < rich < rich) is results[1]
assert (rich <= rich >= rich) is None

# The final unequal elements' results are preserved by sequence comparisons.
Point = namedtuple('Point', ['x'])
for wrap in (list, tuple, deque, Point):
    if wrap is Point:
        a, b = Point(Rich()), Point(Rich())
    else:
        a, b = wrap([Rich()]), wrap([Rich()])
    # Rich.__eq__ is truthy, so the elements are skipped and lengths decide.
    assert (a <= b) is True


class UnequalRich:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        return results[2]

    def __le__(self, other):
        return results[3]

    def __gt__(self, other):
        return results[0]

    def __ge__(self, other):
        return results[1]


for wrap in (list, tuple):
    a, b = wrap([UnequalRich()]), wrap([UnequalRich()])
    assert (a < b) is results[2]
    assert (a <= b) is results[3]
    assert (a > b) is results[0]
    assert (a >= b) is results[1]
a, b = Point(UnequalRich()), Point(UnequalRich())
assert (a < b) is results[2]
assert (a <= b) is results[3]

# Deque normalizes the element's result to bool, unlike list and tuple.
a, b = deque([UnequalRich()]), deque([UnequalRich()])
assert (a < b) is True
assert (a <= b) is True
assert (a > b) is True
assert (a >= b) is False

# === NotImplemented triggers the reflected operator, even for the same type ===
calls = []


class Reflect:
    def __init__(self, name):
        self.name = name

    def __lt__(self, other):
        calls.append((self.name, 'lt'))
        return NotImplemented

    def __gt__(self, other):
        calls.append((self.name, 'gt'))
        return results[0]

    def __le__(self, other):
        calls.append((self.name, 'le'))
        return NotImplemented

    def __ge__(self, other):
        calls.append((self.name, 'ge'))
        return results[2]

    def __eq__(self, other):
        calls.append((self.name, 'eq'))
        return NotImplemented

    def __ne__(self, other):
        calls.append((self.name, 'ne'))
        return NotImplemented


a, b = Reflect('a'), Reflect('b')
assert (a < b) is results[0]
assert calls == [('a', 'lt'), ('b', 'gt')]
calls.clear()
assert (a <= b) is results[2]
assert calls == [('a', 'le'), ('b', 'ge')]
calls.clear()
assert (a == b) is False
assert calls == [('a', 'eq'), ('b', 'eq')]
calls.clear()
assert (a != b) is True
assert calls == [('a', 'ne'), ('b', 'ne')]
calls.clear()
assert (a == a) is True
assert calls == [('a', 'eq'), ('a', 'eq')]
calls.clear()
assert (a != a) is False
assert calls == [('a', 'ne'), ('a', 'ne')]
assert (1 < b) is results[0]
assert (1 <= b) is results[2]

# Tuple subclasses receive reflected priority over plain tuples.
calls.clear()
assert ((a,) < Point(b)) is results[0]
assert calls == [('b', 'eq'), ('a', 'eq'), ('b', 'gt')]


# === Default __ne__ delegates to __eq__, but explicit __ne__ is independent ===
class EqOnly:
    def __eq__(self, other):
        return []


assert (EqOnly() != EqOnly()) is True


class DeclineNe:
    def __eq__(self, other):
        raise ValueError('__eq__ must not run')

    def __ne__(self, other):
        return NotImplemented


a, b = DeclineNe(), DeclineNe()
assert (a != b) is True
assert (a != a) is False


# === Sequence comparisons skip identical elements, not identical containers ===
class IdentityOnly:
    def __eq__(self, other):
        raise ValueError('element equality')

    def __le__(self, other):
        raise ValueError('element ordering')


shared = IdentityOnly()
for wrap in (list, tuple, deque):
    assert wrap([shared, 0]) < wrap([shared, 1])
    assert wrap([shared]) <= wrap([shared])
assert Point(shared) <= Point(shared)
try:
    shared <= shared
    assert False, 'expected __le__ even for identical operands'
except ValueError as exc:
    assert str(exc) == 'element ordering'


# === Comparisons observe mutation without retaining stale element borrows ===
class Grow:
    def __eq__(self, other):
        left.append(0)
        right.append(1)
        return True


left, right = [Grow()], [Grow()]
assert left < right


# === Counter partial orders work through every comparison consumer ===
small, large = Counter(a=1), Counter(a=2)
assert min(large, small) is small
assert max(small, large) is large
assert sorted([large, small]) == [small, large]
for wrap in (list, tuple, deque):
    assert wrap([small]) < wrap([large])
    assert (wrap([Counter(a=1)]) <= wrap([Counter(b=1)])) is False


class Count:
    def __le__(self, other):
        calls.append('le')
        return True

    def __eq__(self, other):
        calls.append('eq')
        return False


calls.clear()
assert Counter(a=Count()) < Counter(a=Count())
assert calls == ['le', 'le', 'eq']
count = Count()
assert (Counter(a=count) == Counter(a=count)) is False


# === Sorting uses < only, with stable ties in both directions ===
class Sortable:
    def __init__(self, key, label):
        self.key = key
        self.label = label

    def __lt__(self, other):
        return [True] if self.key < other.key else []

    def __eq__(self, other):
        raise ValueError('sorting must not use ==')

    def __gt__(self, other):
        raise ValueError('sorting must not use >')


items = [Sortable(2, 'a'), Sortable(1, 'b'), Sortable(2, 'c'), Sortable(1, 'd')]
assert [x.label for x in sorted(items)] == ['b', 'd', 'a', 'c']
assert [x.label for x in sorted(items, reverse=True)] == ['a', 'c', 'b', 'd']
items.sort(reverse=True)
assert [x.label for x in items] == ['a', 'c', 'b', 'd']
assert min(items).label == 'b'
try:
    max(items)
    assert False, 'max must use >'
except ValueError as exc:
    assert str(exc) == 'sorting must not use >'
key_calls = []


def sort_key(x):
    key_calls.append(x)
    return Sortable(x % 3, '')


assert sorted([5, 3, 1, 4, 2], key=sort_key) == [3, 1, 4, 5, 2]
assert key_calls == [5, 3, 1, 4, 2]
for n in (0, 1, 2, 7, 8, 9, 31, 64, 127):
    values = [(i * 19) % 37 for i in range(n)]
    ascending = sorted(values)
    assert ascending == [v for v in range(37) for x in values if v == x]
    assert sorted(values, reverse=True) == list(reversed(ascending))

for n in (8, 9):
    tagged = [Sortable(i % 3, i) for i in range(n)]
    for reverse in (False, True):
        keys = (2, 1, 0) if reverse else (0, 1, 2)
        expected = [i for key in keys for i in range(n) if i % 3 == key]
        assert [item.label for item in sorted(tagged, reverse=reverse)] == expected


# Comparison callbacks see a detached list and mutation is rejected.
class MutateSort:
    def __lt__(self, other):
        assert sorting == []
        sorting.append('changed')
        return True


sorting = [MutateSort(), MutateSort()]
try:
    sorting.sort()
    assert False, 'expected sorting mutation error'
except ValueError as exc:
    assert str(exc) == 'list modified during sort'
assert len(sorting) == 2


# Even inconsistent comparators must not violate the host sort's contract.
class AlwaysLess:
    def __lt__(self, other):
        return True


inconsistent = [AlwaysLess() for _ in range(100)]
sorted_inconsistent = sorted(inconsistent)
assert len(sorted_inconsistent) == len(inconsistent)
assert all(item in sorted_inconsistent for item in inconsistent), 'sorting must retain every item'


# === Exceptions and invalid dunders ===
class Broken:
    def __eq__(self, other):
        raise ValueError('comparison failed')

    def __lt__(self, other):
        raise ValueError('comparison failed')


for compare in (
    lambda: Broken() == 0,
    lambda: Broken() < 0,
    lambda: min([Broken(), Broken()]),
    lambda: sorted([Broken(), Broken()]),
):
    try:
        compare()
        assert False, 'expected comparison error'
    except ValueError as exc:
        assert str(exc) == 'comparison failed'
try:
    assert Broken() == 0
    assert False, 'expected comparison error in fused assert'
except ValueError as exc:
    assert str(exc) == 'comparison failed'
try:
    assert Broken() < 0
    assert False, 'expected comparison error in fused assert'
except ValueError as exc:
    assert str(exc) == 'comparison failed'


class NotCallable:
    __lt__ = None


try:
    NotCallable() < 0
    assert False, 'expected a noncallable dunder to raise'
except TypeError as exc:
    assert str(exc) == "'NoneType' object is not callable"

# === Builtin-specific ordering errors survive sequence nesting ===
for a, b, message in (
    (time(), time(tzinfo=timezone.utc), "can't compare offset-naive and offset-aware times"),
    (
        datetime(2024, 1, 1),
        datetime(2024, 1, 1, tzinfo=timezone.utc),
        "can't compare offset-naive and offset-aware datetimes",
    ),
    (
        date(2024, 1, 1),
        datetime(2024, 1, 1),
        "'<' not supported between instances of 'datetime.date' and 'datetime.datetime'",
    ),
    (1, 'a', "'<' not supported between instances of 'int' and 'str'"),
):
    try:
        [a] < [b]
        assert False, 'expected element comparison error'
    except TypeError as exc:
        assert str(exc) == message
