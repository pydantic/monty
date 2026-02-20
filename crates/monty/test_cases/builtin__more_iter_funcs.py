# === min() ===
# Basic min operations
assert min([1, 2, 3]) == 1, 'min of list'
assert min([3, 1, 2]) == 1, 'min of unsorted list'
assert min([5]) == 5, 'min of single element'
assert min(1, 2, 3) == 1, 'min of multiple args'
assert min(3, 1, 2) == 1, 'min of unsorted args'
assert min(-5, -10, -1) == -10, 'min of negatives'

# min with strings
assert min(['b', 'a', 'c']) == 'a', 'min of string list'
assert min('b', 'a', 'c') == 'a', 'min of string args'

# min with floats
assert min([1.5, 0.5, 2.5]) == 0.5, 'min of floats'
assert min(1.5, 0.5) == 0.5, 'min float args'

# === max() ===
# Basic max operations
assert max([1, 2, 3]) == 3, 'max of list'
assert max([3, 1, 2]) == 3, 'max of unsorted list'
assert max([5]) == 5, 'max of single element'
assert max(1, 2, 3) == 3, 'max of multiple args'
assert max(3, 1, 2) == 3, 'max of unsorted args'
assert max(-5, -10, -1) == -1, 'max of negatives'

# max with strings
assert max(['b', 'a', 'c']) == 'c', 'max of string list'
assert max('b', 'a', 'c') == 'c', 'max of string args'

# max with floats
assert max([1.5, 0.5, 2.5]) == 2.5, 'max of floats'
assert max(1.5, 2.5) == 2.5, 'max float args'

# === sorted() ===
# Basic sorted operations
assert sorted([3, 1, 2]) == [1, 2, 3], 'sorted int list'
assert sorted([1, 2, 3]) == [1, 2, 3], 'sorted already sorted'
assert sorted([3, 2, 1]) == [1, 2, 3], 'sorted reverse order'
assert sorted([]) == [], 'sorted empty list'
assert sorted([5]) == [5], 'sorted single element'

# sorted with strings
assert sorted(['c', 'a', 'b']) == ['a', 'b', 'c'], 'sorted strings'

# sorted with heap-allocated strings (from split)
assert sorted('banana,apple,cherry'.split(',')) == ['apple', 'banana', 'cherry'], 'sorted split strings'

# sorted with multi-char string literals (heap-allocated)
assert sorted(['banana', 'apple', 'cherry']) == ['apple', 'banana', 'cherry'], 'sorted multi-char strings'

# min/max with heap-allocated strings
assert min('banana,apple,cherry'.split(',')) == 'apple', 'min of split strings'
assert max('banana,apple,cherry'.split(',')) == 'cherry', 'max of split strings'

# sorted with negative numbers
assert sorted([-3, 1, -2, 2]) == [-3, -2, 1, 2], 'sorted with negatives'

# sorted with tuple
assert sorted((3, 1, 2)) == [1, 2, 3], 'sorted tuple returns list'

# sorted preserves duplicates
assert sorted([3, 1, 2, 1, 3]) == [1, 1, 2, 3, 3], 'sorted with duplicates'

# sorted with range
assert sorted(range(5, 0, -1)) == [1, 2, 3, 4, 5], 'sorted range'

# === sorted() with reverse ===
assert sorted([3, 1, 2], reverse=True) == [3, 2, 1], 'sorted reverse=True'
assert sorted([3, 1, 2], reverse=False) == [1, 2, 3], 'sorted reverse=False'
assert sorted(['c', 'a', 'b'], reverse=True) == ['c', 'b', 'a'], 'sorted strings reverse'
assert sorted([], reverse=True) == [], 'sorted empty reverse'
assert sorted([5], reverse=True) == [5], 'sorted single reverse'
assert sorted([3, 1, 2], reverse=0) == [1, 2, 3], 'sorted reverse=0 (falsy)'
assert sorted([3, 1, 2], reverse=1) == [3, 2, 1], 'sorted reverse=1 (truthy)'

# === sorted() with key ===
assert sorted([3, -1, 2, -4], key=abs) == [-1, 2, 3, -4], 'sorted key=abs'
assert sorted(['banana', 'apple', 'cherry'], key=len) == ['apple', 'banana', 'cherry'], 'sorted key=len'
assert sorted([3, 1, 2], key=None) == [1, 2, 3], 'sorted key=None same as no key'


def negate(x):
    return -x


assert sorted([1, -2, 3], key=negate) == [3, 1, -2], 'sorted key=user-defined function'

# === sorted() with key and reverse ===
assert sorted([3, -1, 2, -4], key=abs, reverse=True) == [-4, 3, 2, -1], 'sorted key=abs reverse=True'
assert sorted(['banana', 'apple', 'cherry'], key=len, reverse=True) == ['banana', 'cherry', 'apple'], (
    'sorted key=len reverse=True'
)
assert sorted([3, 1, 2], key=None, reverse=True) == [3, 2, 1], 'sorted key=None reverse=True'

# === reversed() ===
# Basic reversed operations
assert list(reversed([1, 2, 3])) == [3, 2, 1], 'reversed list'
assert list(reversed([1])) == [1], 'reversed single element'
assert list(reversed([])) == [], 'reversed empty list'

# reversed tuple
assert list(reversed((1, 2, 3))) == [3, 2, 1], 'reversed tuple'

# reversed string
assert list(reversed('abc')) == ['c', 'b', 'a'], 'reversed string'

# reversed range
assert list(reversed(range(1, 4))) == [3, 2, 1], 'reversed range'

# === enumerate() ===
# Basic enumerate operations
assert list(enumerate(['a', 'b', 'c'])) == [(0, 'a'), (1, 'b'), (2, 'c')], 'enumerate list'
assert list(enumerate([])) == [], 'enumerate empty list'
assert list(enumerate(['x'])) == [(0, 'x')], 'enumerate single element'

# enumerate with start
assert list(enumerate(['a', 'b'], 1)) == [(1, 'a'), (2, 'b')], 'enumerate with start'
assert list(enumerate(['a', 'b'], 10)) == [(10, 'a'), (11, 'b')], 'enumerate with start 10'

# enumerate string
assert list(enumerate('ab')) == [(0, 'a'), (1, 'b')], 'enumerate string'

# enumerate range
assert list(enumerate(range(3))) == [(0, 0), (1, 1), (2, 2)], 'enumerate range'

# === zip() ===
# Basic zip operations
assert list(zip([1, 2], ['a', 'b'])) == [(1, 'a'), (2, 'b')], 'zip two lists'
assert list(zip([1], ['a'])) == [(1, 'a')], 'zip single elements'
assert list(zip([], [])) == [], 'zip empty lists'

# zip truncates to shortest
assert list(zip([1, 2, 3], ['a', 'b'])) == [(1, 'a'), (2, 'b')], 'zip truncates to shortest'
assert list(zip([1], ['a', 'b', 'c'])) == [(1, 'a')], 'zip truncates first shorter'

# zip three iterables
assert list(zip([1, 2], ['a', 'b'], [True, False])) == [(1, 'a', True), (2, 'b', False)], 'zip three lists'

# zip with different types
assert list(zip(range(3), 'abc')) == [(0, 'a'), (1, 'b'), (2, 'c')], 'zip range and string'

# zip single iterable
assert list(zip([1, 2, 3])) == [(1,), (2,), (3,)], 'zip single iterable'

# zip with empty
assert list(zip([1, 2], [])) == [], 'zip with empty second'
assert list(zip([], [1, 2])) == [], 'zip with empty first'
