import bisect
from collections import Counter, deque
from bisect import bisect_left, bisect_right, insort_left, insort_right

assert bisect.bisect is bisect_right
assert bisect.insort is insort_right

# Exhaustive small sorted inputs, including duplicates and bounded subranges.
for size in range(12):
    a = [i // 3 for i in range(size)]
    for x in range(-1, 6):
        for lo in range(size + 2):
            for hi in range(-3, size + 1):
                upper = size if hi == -1 else hi
                expected_left = lo
                expected_right = lo
                for i in range(lo, upper):
                    if a[i] < x:
                        expected_left += 1
                    if not x < a[i]:
                        expected_right += 1
                assert bisect_left(a, x, lo, hi) == expected_left
                assert bisect_right(a, x, lo, hi) == expected_right
                assert bisect_left(a=a, x=x, lo=lo, hi=hi) == expected_left
                assert bisect_right(a, x, lo, None) == bisect_right(a, x, lo)

for a, x, left, right in [
    ((1, 2, 2, 4), 2, 1, 3),
    (range(0, 10, 2), 4, 2, 3),
    ('abbc', 'b', 1, 3),
    (b'abbc', 98, 1, 3),
    (deque([1, 2, 2, 4]), 2, 1, 3),
]:
    assert bisect_left(a, x) == left
    assert bisect_right(a, x) == right

# key is applied only to the sequence for searches, and to x once for insertion.
rows = [(1, 'a'), (2, 'b'), (2, 'c'), (4, 'd')]
assert bisect_left(rows, 2, key=lambda row: row[0]) == 1
assert bisect_right(rows, 2, key=lambda row: row[0]) == 3


def key(row):
    calls.append(row)
    return row[0]


for insert, expected in [(insort_left, 1), (insort_right, 3)]:
    a = rows.copy()
    x = (2, 'new')
    calls = []

    assert insert(a, x, key=key) is None
    assert a[expected] is x
    assert calls[0] is x
    assert calls.count(x) == 1
    assert len(a) == 5

for insert in [insort_left, insort_right, bisect.insort]:
    a = [1, 3, 5]
    assert insert(a, 4) is None
    assert a == [1, 3, 4, 5]
    insert(a, 0, 1, 1)
    assert a == [1, 0, 3, 4, 5]
    insert(a, 9, 99, 0)
    assert a[-1] == 9
    d = deque([1, 3])
    insert(d, 2)
    assert list(d) == [1, 2, 3]

# Python comparisons: NaN must not be treated as equality or ordered.
nan = float('nan')
assert bisect_left([1.0, 2.0], nan) == 0
assert bisect_right([1.0, 2.0], nan) == 2
assert bisect_left([nan, nan], 1) == 0
assert bisect_right([nan, nan], 1) == 2
assert bisect_left([False, True], True) == 1
assert bisect_right([False, True], True) == 2
assert bisect_left([10**50, 10**60], 10**55) == 1
assert bisect_left([1, 2], 2, False, True) == 1
assert bisect_left([], 1, 2**63 - 1, 0) == 2**63 - 1

# A callback may mutate the container; each subsequent subscript sees it.
a = [1, 2, 3, 4]
seen = []


def changing_key(item):
    seen.append(item)
    if len(seen) == 1:
        a[0] = 0
    return item


assert bisect_left(a, 2, key=changing_key) == 1
assert seen == [3, 2, 0]


# Lexicographic comparison checks equality before ordering each element.
from collections import namedtuple

Pair = namedtuple('Pair', ['value', 'tag'])
assert bisect_left(Pair(1, 3), 2) == 1
assert bisect_right(Pair(1, 3), 3) == 2
for seq, target in [
    ([(nan, 1), (nan, 2)], (nan, 1)),
    ([[nan, 1], [nan, 2]], [nan, 1]),
    ([Pair(nan, 1), Pair(nan, 2)], (nan, 1)),
    ([(nan, 1), (nan, 2)], Pair(nan, 1)),
]:
    assert bisect_left(seq, target) == 0
    assert bisect_right(seq, target) == 1

for seq, target in [([[[1]]], [['a']]), ([(1,)], ('a',)), ([Pair(1, 2)], ('a', 2))]:
    try:
        bisect_left(seq, target)
        assert False, 'expected failure'
    except TypeError as exc:
        assert str(exc) == "'<' not supported between instances of 'int' and 'str'"


# Counter has Python sequence slots despite being a mapping; dict/defaultdict do not.
assert bisect_left(Counter(), 3) == 0
counter = Counter({0: 1, 1: 3, 2: 5})
assert bisect_left(counter, 3) == 1
assert bisect_right(counter, 3) == 2

# Function representations retain the canonical name, including through aliases.
for function, name in [
    (bisect_left, 'bisect_left'),
    (bisect_right, 'bisect_right'),
    (insort_left, 'insort_left'),
    (insort_right, 'insort_right'),
    (bisect.bisect, 'bisect_right'),
    (bisect.insort, 'insort_right'),
]:
    assert name in repr(function)


# Lengths above 32-bit usize still fit Monty's fixed 64-bit index width.
large_range = range(2**40)
assert bisect_left(large_range, 2**39) == 2**39
assert bisect_right(large_range, 2**39) == 2**39 + 1
assert bisect_left(range(-(2**62), 2**62 - 1), 0) == 2**62
