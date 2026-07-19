# === min() ===
# Basic min operations
assert min([1, 2, 3]) == 1
assert min([3, 1, 2]) == 1
assert min([5]) == 5
assert min(1, 2, 3) == 1
assert min(3, 1, 2) == 1
assert min(-5, -10, -1) == -10

# min with strings
assert min(['b', 'a', 'c']) == 'a'
assert min('b', 'a', 'c') == 'a'

# min with floats
assert min([1.5, 0.5, 2.5]) == 0.5
assert min(1.5, 0.5) == 0.5

# === max() ===
# Basic max operations
assert max([1, 2, 3]) == 3
assert max([3, 1, 2]) == 3
assert max([5]) == 5
assert max(1, 2, 3) == 3
assert max(3, 1, 2) == 3
assert max(-5, -10, -1) == -1

# max with strings
assert max(['b', 'a', 'c']) == 'c'
assert max('b', 'a', 'c') == 'c'

# max with floats
assert max([1.5, 0.5, 2.5]) == 2.5
assert max(1.5, 2.5) == 2.5

# max with keyword arguments
assert max([3, -1, 2, -4], key=abs) == -4
assert max(['a', 'bbb', 'cc'], key=len) == 'bbb'
assert max(['a', 'bbb', 'cc'], key=lambda s: len(s)) == 'bbb'
assert max('a', 'bbb', 'cc', key=len) == 'bbb'
assert max([1, 2, 3], key=None) == 3
assert max([], default='fallback') == 'fallback'
assert max([], key=len, default='fallback') == 'fallback'

# min with keyword arguments
assert min([3, -1, 2, -4], key=abs) == -1
assert min(['a', 'bbb', 'cc'], key=len) == 'a'
assert min(['a', 'bbb', 'cc'], key=lambda s: len(s)) == 'a'
assert min('a', 'bbb', 'cc', key=len) == 'a'
assert min([1, 2, 3], key=None) == 1
assert min([], default='fallback') == 'fallback'
assert min([], key=len, default='fallback') == 'fallback'

# max/min with tuple-producing key functions
ranked_items = [
    {'downloads': 10, 'likes': 1},
    {'downloads': 10, 'likes': 5},
    {'downloads': 20, 'likes': 0},
]
assert max(ranked_items, key=lambda item: (item.get('downloads', 0), item.get('likes', 0))) == {
    'downloads': 20,
    'likes': 0,
}

tie_items = [
    {'downloads': 10, 'likes': 5, 'name': 'first'},
    {'downloads': 10, 'likes': 5, 'name': 'second'},
]
assert max(tie_items, key=lambda item: (item['downloads'], item['likes']))['name'] == 'first'
assert min(tie_items, key=lambda item: (item['downloads'], item['likes']))['name'] == 'first'

try:
    max([1], nope=1)
    assert False, 'invalid max keyword should raise TypeError'
except TypeError as e:
    assert e.args == ("max() got an unexpected keyword argument 'nope'",)

try:
    min([1], nope=1)
    assert False, 'invalid min keyword should raise TypeError'
except TypeError as e:
    assert e.args == ("min() got an unexpected keyword argument 'nope'",)

try:
    max(key=int)
    assert False, 'max with only kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ('max expected at least 1 argument, got 0',)

try:
    min(default=None, key=int)
    assert False, 'min with only kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ('min expected at least 1 argument, got 0',)

try:
    max(nope=1)
    assert False, 'max with only unexpected kwargs should still raise a TypeError'
except TypeError:
    # CPython: 'max expected at least 1 argument, got 0' (validates positional
    # count first). Monty: "max() got an unexpected keyword argument 'nope'"
    # (validates kwargs first, via the FromArgs macro pipeline).
    pass

try:
    min(nope=1)
    assert False, 'min with only unexpected kwargs should still raise a TypeError'
except TypeError:
    # See note above; Monty differs from CPython on validation order.
    pass

try:
    max(key=int, nope=1)
    assert False, 'max with mixed kwargs and no positional args should still raise a TypeError'
except TypeError:
    # See note above; Monty differs from CPython on validation order.
    pass

try:
    max(1, 2, default=3)
    assert False, 'max with multiple args and default should raise TypeError'
except TypeError as e:
    assert e.args == ('Cannot specify a default for max() with multiple positional arguments',)

try:
    min(1, 2, default=3)
    assert False, 'min with multiple args and default should raise TypeError'
except TypeError as e:
    assert e.args == ('Cannot specify a default for min() with multiple positional arguments',)

try:
    max(1, key=int)
    assert False, 'max single non-iterable arg with key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not iterable",)

try:
    min(1, key=int)
    assert False, 'min single non-iterable arg with key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not iterable",)

try:
    max([1], key=1)
    assert False, 'max non-callable key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not callable",)

try:
    min([1], key=1)
    assert False, 'min non-callable key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not callable",)

try:
    max([])
    assert False, 'max empty iterable without default should raise ValueError'
except ValueError as e:
    assert e.args == ('max() iterable argument is empty',)

try:
    min([])
    assert False, 'min empty iterable without default should raise ValueError'
except ValueError as e:
    assert e.args == ('min() iterable argument is empty',)

assert max([1], default=2) == 1
assert min([1], default=2) == 1
assert max([], key=1, default='fallback') == 'fallback'
assert min([], key=1, default='fallback') == 'fallback'

try:
    max([1], key=abs, **{'key': len})
    assert False, 'duplicate max key should raise TypeError'
except TypeError as e:
    assert e.args == ("max() got multiple values for keyword argument 'key'",)

try:
    min([], default='x', **{'default': 'y'})
    assert False, 'duplicate min default should raise TypeError'
except TypeError as e:
    assert e.args == ("min() got multiple values for keyword argument 'default'",)

try:
    max([], **{1: 2})
    assert False, 'max non-string keyword key should raise TypeError'
except TypeError as e:
    assert e.args == ('keywords must be strings',)

try:
    max([1, 'a'])
    assert False, 'max with incomparable iterable items should raise TypeError'
except TypeError as e:
    assert e.args == ("'>' not supported between instances of 'str' and 'int'",)

try:
    min(1, 'a')
    assert False, 'min with incomparable positional args should raise TypeError'
except TypeError as e:
    assert e.args == ("'<' not supported between instances of 'str' and 'int'",)

max_key_map = {10: 1, 20: 3, 30: 3, 40: 2}
assert max([10, 20, 30, 40], key=lambda item: max_key_map[item]) == 20

min_key_map = {10: 2, 20: 1, 30: 1, 40: 3}
assert min([10, 20, 30, 40], key=lambda item: min_key_map[item]) == 20

# === sorted() ===
# Basic sorted operations
assert sorted([3, 1, 2]) == [1, 2, 3]
assert sorted([1, 2, 3]) == [1, 2, 3]
assert sorted([3, 2, 1]) == [1, 2, 3]
assert sorted([]) == []
assert sorted([5]) == [5]

# sorted with strings
assert sorted(['c', 'a', 'b']) == ['a', 'b', 'c']

# sorted with heap-allocated strings (from split)
assert sorted('banana,apple,cherry'.split(',')) == ['apple', 'banana', 'cherry']

# sorted with multi-char string literals (heap-allocated)
assert sorted(['banana', 'apple', 'cherry']) == ['apple', 'banana', 'cherry']

# min/max with heap-allocated strings
assert min('banana,apple,cherry'.split(',')) == 'apple'
assert max('banana,apple,cherry'.split(',')) == 'cherry'

# sorted with negative numbers
assert sorted([-3, 1, -2, 2]) == [-3, -2, 1, 2]

# sorted with tuple
assert sorted((3, 1, 2)) == [1, 2, 3]

# sorted preserves duplicates
assert sorted([3, 1, 2, 1, 3]) == [1, 1, 2, 3, 3]

# sorted with range
assert sorted(range(5, 0, -1)) == [1, 2, 3, 4, 5]

try:
    sorted(1, 2)
    assert False, 'sorted() with too many positional arguments should raise TypeError'
except TypeError as e:
    assert e.args == ('sorted expected 1 argument, got 2',)

try:
    sorted([1], nope=1)
    assert False, 'sorted() with invalid keyword should raise TypeError'
except TypeError as e:
    assert str(e) == "sort() got an unexpected keyword argument 'nope'", f'sorted unknown kw: {e}'

# === sorted() with reverse ===
assert sorted([3, 1, 2], reverse=True) == [3, 2, 1]
assert sorted([3, 1, 2], reverse=False) == [1, 2, 3]
assert sorted(['c', 'a', 'b'], reverse=True) == ['c', 'b', 'a']
assert sorted([], reverse=True) == []
assert sorted([5], reverse=True) == [5]
assert sorted([3, 1, 2], reverse=0) == [1, 2, 3]
assert sorted([3, 1, 2], reverse=1) == [3, 2, 1]

# === sorted() with key ===
assert sorted([3, -1, 2, -4], key=abs) == [-1, 2, 3, -4]
assert sorted(['banana', 'apple', 'cherry'], key=len) == ['apple', 'banana', 'cherry']
assert sorted([3, 1, 2], key=None) == [1, 2, 3]

try:
    sorted([1], key=abs, **{'key': len})
    assert False, 'duplicate sorted key should raise TypeError'
except TypeError as e:
    assert e.args == ("sorted() got multiple values for keyword argument 'key'",)


def negate(x):
    return -x


assert sorted([1, -2, 3], key=negate) == [3, 1, -2]

# === sorted() with key and reverse ===
assert sorted([3, -1, 2, -4], key=abs, reverse=True) == [-4, 3, 2, -1]
assert sorted(['banana', 'apple', 'cherry'], key=len, reverse=True) == ['banana', 'cherry', 'apple']
assert sorted([3, 1, 2], key=None, reverse=True) == [3, 2, 1]

# === reversed() ===
# Basic reversed operations
assert list(reversed([1, 2, 3])) == [3, 2, 1]
assert list(reversed([1])) == [1]
assert list(reversed([])) == []

# reversed tuple
assert list(reversed((1, 2, 3))) == [3, 2, 1]

# reversed string
assert list(reversed('abc')) == ['c', 'b', 'a']

# reversed range
assert list(reversed(range(1, 4))) == [3, 2, 1]

# === enumerate() ===
# Basic enumerate operations
assert list(enumerate(['a', 'b', 'c'])) == [(0, 'a'), (1, 'b'), (2, 'c')]
assert list(enumerate([])) == []
assert list(enumerate(['x'])) == [(0, 'x')]

# enumerate with start
assert list(enumerate(['a', 'b'], 1)) == [(1, 'a'), (2, 'b')]
assert list(enumerate(['a', 'b'], 10)) == [(10, 'a'), (11, 'b')]

# a non-iterable errors out of iterator construction without leaking a
# heap-backed start=
try:
    enumerate(1, start=10**30)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not iterable"

# enumerate keyword forms (CPython's vectorcall accepts all of these)
assert list(enumerate(['a', 'b'], start=1)) == [(1, 'a'), (2, 'b')]
assert list(enumerate(iterable=['a'])) == [(0, 'a')]
assert list(enumerate(iterable=['a'], start=2)) == [(2, 'a')]
assert list(enumerate(start=2, iterable=['a'])) == [(2, 'a')]
assert list(enumerate(['a'], start=True)) == [(1, 'a')]

# enumerate string
assert list(enumerate('ab')) == [(0, 'a'), (1, 'b')]

# enumerate range
assert list(enumerate(range(3))) == [(0, 0), (1, 1), (2, 2)]

# === zip() ===
# Basic zip operations
assert list(zip([1, 2], ['a', 'b'])) == [(1, 'a'), (2, 'b')]
assert list(zip([1], ['a'])) == [(1, 'a')]
assert list(zip([], [])) == []

# zip truncates to shortest
assert list(zip([1, 2, 3], ['a', 'b'])) == [(1, 'a'), (2, 'b')]
assert list(zip([1], ['a', 'b', 'c'])) == [(1, 'a')]

# zip three iterables
assert list(zip([1, 2], ['a', 'b'], [True, False])) == [(1, 'a', True), (2, 'b', False)]

# zip with different types
assert list(zip(range(3), 'abc')) == [(0, 'a'), (1, 'b'), (2, 'c')]

# zip single iterable
assert list(zip([1, 2, 3])) == [(1,), (2,), (3,)]

# zip with empty
assert list(zip([1, 2], [])) == []
assert list(zip([], [1, 2])) == []

# === zip(strict=True) ===
# Equal length iterables succeed
assert list(zip([1, 2], [3, 4], strict=True)) == [(1, 3), (2, 4)]
assert list(zip([1], [2], [3], strict=True)) == [(1, 2, 3)]
assert list(zip([], [], strict=True)) == []
assert list(zip(strict=True)) == []
assert list(zip([1, 2, 3], strict=True)) == [(1,), (2,), (3,)]

# strict=False behaves like default
assert list(zip([1, 2, 3], [4, 5], strict=False)) == [(1, 4), (2, 5)]

# Falsy values are accepted
assert list(zip([1, 2, 3], [4, 5], strict=0)) == [(1, 4), (2, 5)]

# Second argument shorter
try:
    list(zip([1, 2, 3], [4, 5], strict=True))
    assert False, 'zip strict should raise for shorter arg 2'
except ValueError as e:
    assert str(e) == 'zip() argument 2 is shorter than argument 1'

# Second argument longer
try:
    list(zip([1, 2], [4, 5, 6], strict=True))
    assert False, 'zip strict should raise for longer arg 2'
except ValueError as e:
    assert str(e) == 'zip() argument 2 is longer than argument 1'

# Third argument shorter with plural
try:
    list(zip([1, 2], [3, 4], [5], strict=True))
    assert False, 'zip strict should raise for shorter arg 3'
except ValueError as e:
    assert str(e) == 'zip() argument 3 is shorter than arguments 1-2'

# Fourth argument shorter
try:
    list(zip([1, 2], [3, 4], [5, 6], [7], strict=True))
    assert False, 'zip strict should raise for shorter arg 4'
except ValueError as e:
    assert str(e) == 'zip() argument 4 is shorter than arguments 1-3'

# Third argument longer than arguments 1-2 (both exhausted)
try:
    list(zip([1], [2], [3, 4], strict=True))
    assert False, 'zip strict should raise for longer arg 3'
except ValueError as e:
    assert str(e) == 'zip() argument 3 is longer than arguments 1-2'

# Unexpected keyword argument
try:
    list(zip([1], foo=True))
    assert False, 'zip unexpected kwarg should raise TypeError'
except TypeError as e:
    assert str(e) == "zip() got an unexpected keyword argument 'foo'"
