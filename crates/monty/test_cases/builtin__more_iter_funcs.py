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

# max with keyword arguments
assert max([3, -1, 2, -4], key=abs) == -4, 'max key=abs'
assert max(['a', 'bbb', 'cc'], key=len) == 'bbb', 'max key=len'
assert max(['a', 'bbb', 'cc'], key=lambda s: len(s)) == 'bbb', 'max key=lambda simple callable'
assert max('a', 'bbb', 'cc', key=len) == 'bbb', 'max multiple args key=len'
assert max([1, 2, 3], key=None) == 3, 'max key=None same as no key'
assert max([], default='fallback') == 'fallback', 'max default for empty iterable'
assert max([], key=len, default='fallback') == 'fallback', 'max key+default for empty iterable'

# min with keyword arguments
assert min([3, -1, 2, -4], key=abs) == -1, 'min key=abs'
assert min(['a', 'bbb', 'cc'], key=len) == 'a', 'min key=len'
assert min(['a', 'bbb', 'cc'], key=lambda s: len(s)) == 'a', 'min key=lambda simple callable'
assert min('a', 'bbb', 'cc', key=len) == 'a', 'min multiple args key=len'
assert min([1, 2, 3], key=None) == 1, 'min key=None same as no key'
assert min([], default='fallback') == 'fallback', 'min default for empty iterable'
assert min([], key=len, default='fallback') == 'fallback', 'min key+default for empty iterable'

# max/min with tuple-producing key functions
ranked_items = [
    {'downloads': 10, 'likes': 1},
    {'downloads': 10, 'likes': 5},
    {'downloads': 20, 'likes': 0},
]
assert max(ranked_items, key=lambda item: (item.get('downloads', 0), item.get('likes', 0))) == {
    'downloads': 20,
    'likes': 0,
}, 'max key=lambda tuple ranking'

tie_items = [
    {'downloads': 10, 'likes': 5, 'name': 'first'},
    {'downloads': 10, 'likes': 5, 'name': 'second'},
]
assert max(tie_items, key=lambda item: (item['downloads'], item['likes']))['name'] == 'first', (
    'max returns first maximal item on ties'
)
assert min(tie_items, key=lambda item: (item['downloads'], item['likes']))['name'] == 'first', (
    'min returns first minimal item on ties'
)

try:
    max([1], nope=1)
    assert False, 'invalid max keyword should raise TypeError'
except TypeError as e:
    assert e.args == ("max() got an unexpected keyword argument 'nope'",), 'max invalid keyword error matches CPython'

try:
    min([1], nope=1)
    assert False, 'invalid min keyword should raise TypeError'
except TypeError as e:
    assert e.args == ("min() got an unexpected keyword argument 'nope'",), 'min invalid keyword error matches CPython'

try:
    max(key=int)
    assert False, 'max with only kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ('max expected at least 1 argument, got 0',), 'max kwargs-only arity error matches CPython'

try:
    min(default=None, key=int)
    assert False, 'min with only kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ('min expected at least 1 argument, got 0',), 'min kwargs-only arity error matches CPython'

try:
    max(nope=1)
    assert False, 'max with only unexpected kwargs should still raise the zero-arg TypeError'
except TypeError as e:
    assert e.args == ('max expected at least 1 argument, got 0',), (
        'max zero-arg error takes precedence over kwargs validation'
    )

try:
    min(nope=1)
    assert False, 'min with only unexpected kwargs should still raise the zero-arg TypeError'
except TypeError as e:
    assert e.args == ('min expected at least 1 argument, got 0',), (
        'min zero-arg error takes precedence over kwargs validation'
    )

try:
    max(key=int, nope=1)
    assert False, 'max with mixed kwargs and no positional args should still raise the zero-arg TypeError'
except TypeError as e:
    assert e.args == ('max expected at least 1 argument, got 0',), 'max zero-arg error beats mixed kwargs validation'

try:
    max(1, 2, default=3)
    assert False, 'max with multiple args and default should raise TypeError'
except TypeError as e:
    assert e.args == ('Cannot specify a default for max() with multiple positional arguments',), (
        'max multiple args default error matches CPython'
    )

try:
    min(1, 2, default=3)
    assert False, 'min with multiple args and default should raise TypeError'
except TypeError as e:
    assert e.args == ('Cannot specify a default for min() with multiple positional arguments',), (
        'min multiple args default error matches CPython'
    )

try:
    max(1, key=int)
    assert False, 'max single non-iterable arg with key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not iterable",), 'max single arg with key still uses iterable form'

try:
    min(1, key=int)
    assert False, 'min single non-iterable arg with key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not iterable",), 'min single arg with key still uses iterable form'

try:
    max([1], key=1)
    assert False, 'max non-callable key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not callable",), 'max non-callable key error matches CPython'

try:
    min([1], key=1)
    assert False, 'min non-callable key should raise TypeError'
except TypeError as e:
    assert e.args == ("'int' object is not callable",), 'min non-callable key error matches CPython'

try:
    max([])
    assert False, 'max empty iterable without default should raise ValueError'
except ValueError as e:
    assert e.args == ('max() iterable argument is empty',), 'max empty iterable error unchanged'

try:
    min([])
    assert False, 'min empty iterable without default should raise ValueError'
except ValueError as e:
    assert e.args == ('min() iterable argument is empty',), 'min empty iterable error unchanged'

assert max([1], default=2) == 1, 'max ignores default for non-empty iterable'
assert min([1], default=2) == 1, 'min ignores default for non-empty iterable'
assert max([], key=1, default='fallback') == 'fallback', 'max does not validate key for empty iterable with default'
assert min([], key=1, default='fallback') == 'fallback', 'min does not validate key for empty iterable with default'

try:
    max([1], key=abs, **{'key': len})
    assert False, 'duplicate max key should raise TypeError'
except TypeError as e:
    assert e.args == ("max() got multiple values for keyword argument 'key'",), (
        'max duplicate key error matches CPython'
    )

try:
    min([], default='x', **{'default': 'y'})
    assert False, 'duplicate min default should raise TypeError'
except TypeError as e:
    assert e.args == ("min() got multiple values for keyword argument 'default'",), (
        'min duplicate default error matches CPython'
    )

try:
    max([], **{1: 2})
    assert False, 'max non-string keyword key should raise TypeError'
except TypeError as e:
    assert e.args == ('keywords must be strings',), 'max non-string keyword key error matches CPython'

try:
    max([1, 'a'])
    assert False, 'max with incomparable iterable items should raise TypeError'
except TypeError as e:
    assert e.args == ("'>' not supported between instances of 'str' and 'int'",), (
        'max iterable comparison error matches CPython'
    )

try:
    min(1, 'a')
    assert False, 'min with incomparable positional args should raise TypeError'
except TypeError as e:
    assert e.args == ("'<' not supported between instances of 'str' and 'int'",), (
        'min positional comparison error matches CPython'
    )

max_key_map = {10: 1, 20: 3, 30: 3, 40: 2}
assert max([10, 20, 30, 40], key=lambda item: max_key_map[item]) == 20, (
    'max returns first item among repeated maximal keys'
)

min_key_map = {10: 2, 20: 1, 30: 1, 40: 3}
assert min([10, 20, 30, 40], key=lambda item: min_key_map[item]) == 20, (
    'min returns first item among repeated minimal keys'
)

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

try:
    sorted(1, 2)
    assert False, 'sorted() with too many positional arguments should raise TypeError'
except TypeError as e:
    assert e.args == ('sorted expected 1 argument, got 2',), 'sorted() positional arity error matches CPython'

try:
    sorted([1], nope=1)
    assert False, 'sorted() with invalid keyword should raise TypeError'
except TypeError as e:
    assert e.args == ("sort() got an unexpected keyword argument 'nope'",), (
        'sorted() invalid keyword error matches CPython'
    )

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

try:
    sorted([1], key=abs, **{'key': len})
    assert False, 'duplicate sorted key should raise TypeError'
except TypeError as e:
    assert e.args == ("sorted() got multiple values for keyword argument 'key'",), (
        'sorted duplicate key error matches CPython'
    )


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

# === zip(strict=True) ===
# Equal length iterables succeed
assert list(zip([1, 2], [3, 4], strict=True)) == [(1, 3), (2, 4)], 'zip strict equal lengths'
assert list(zip([1], [2], [3], strict=True)) == [(1, 2, 3)], 'zip strict three single-element lists'
assert list(zip([], [], strict=True)) == [], 'zip strict empty lists'
assert list(zip(strict=True)) == [], 'zip strict no arguments'
assert list(zip([1, 2, 3], strict=True)) == [(1,), (2,), (3,)], 'zip strict single iterable'

# strict=False behaves like default
assert list(zip([1, 2, 3], [4, 5], strict=False)) == [(1, 4), (2, 5)], 'zip strict=False truncates'

# Falsy values are accepted
assert list(zip([1, 2, 3], [4, 5], strict=0)) == [(1, 4), (2, 5)], 'zip strict=0 is falsy'

# Second argument shorter
try:
    list(zip([1, 2, 3], [4, 5], strict=True))
    assert False, 'zip strict should raise for shorter arg 2'
except ValueError as e:
    assert str(e) == 'zip() argument 2 is shorter than argument 1', 'zip strict shorter error'

# Second argument longer
try:
    list(zip([1, 2], [4, 5, 6], strict=True))
    assert False, 'zip strict should raise for longer arg 2'
except ValueError as e:
    assert str(e) == 'zip() argument 2 is longer than argument 1', 'zip strict longer error'

# Third argument shorter with plural
try:
    list(zip([1, 2], [3, 4], [5], strict=True))
    assert False, 'zip strict should raise for shorter arg 3'
except ValueError as e:
    assert str(e) == 'zip() argument 3 is shorter than arguments 1-2', 'zip strict shorter plural'

# Fourth argument shorter
try:
    list(zip([1, 2], [3, 4], [5, 6], [7], strict=True))
    assert False, 'zip strict should raise for shorter arg 4'
except ValueError as e:
    assert str(e) == 'zip() argument 4 is shorter than arguments 1-3', 'zip strict shorter 4 args'

# Third argument longer than arguments 1-2 (both exhausted)
try:
    list(zip([1], [2], [3, 4], strict=True))
    assert False, 'zip strict should raise for longer arg 3'
except ValueError as e:
    assert str(e) == 'zip() argument 3 is longer than arguments 1-2', 'zip strict longer plural'

# Unexpected keyword argument
try:
    list(zip([1], foo=True))
    assert False, 'zip unexpected kwarg should raise TypeError'
except TypeError as e:
    assert str(e) == "zip() got an unexpected keyword argument 'foo'", 'zip unexpected kwarg error'
