# === List concatenation (+) ===
assert [1, 2] + [3, 4] == [1, 2, 3, 4]
assert [] + [1, 2] == [1, 2]
assert [1, 2] + [] == [1, 2]
assert [] + [] == []
assert [1] + [2] + [3] + [4] == [1, 2, 3, 4]
assert [[1]] + [[2]] == [[1], [2]]

# === Augmented assignment (+=) ===
lst = [1, 2]
lst += [3, 4]
assert lst == [1, 2, 3, 4]

lst = [1]
alias = lst
lst += [2]
assert lst is alias
assert alias == [1, 2]

lst = [1, 2, 3]
index = 1
lst[index] += 5
assert lst == [1, 7, 3]

try:
    lst = [1]
    lst[5] += 1
    assert False, 'subscript += past the end of a list should raise IndexError'
except IndexError as e:
    assert e.args == ('list index out of range',)

lst = [1]
lst += []
assert lst == [1]

lst = [1]
lst += [2]
lst += [3]
assert lst == [1, 2, 3]

lst = [1, 2]
lst += lst
assert lst == [1, 2, 1, 2]

# === List length ===
assert len([]) == 0
assert len([1, 2, 3]) == 3

lst = [1]
lst.append(2)
assert len(lst) == 2

# === List indexing ===
a = []
a.append('value')
assert a[0] == 'value'

a = [1, 2, 3]
assert a[0 - 1] == 3
assert a[-1] == 3
assert a[-2] == 2

# === List repr/str ===
assert repr([]) == '[]'
assert str([]) == '[]'

assert repr([1, 2, 3]) == '[1, 2, 3]'
assert str([1, 2, 3]) == '[1, 2, 3]'

# === List repetition (*) ===
assert [1, 2] * 3 == [1, 2, 1, 2, 1, 2]
assert 3 * [1, 2] == [1, 2, 1, 2, 1, 2]
assert [1] * 0 == []
assert [1] * -1 == []
assert [] * 5 == []
assert [1, 2] * 1 == [1, 2]
assert [[1]] * 2 == [[1], [1]]

# === List repetition augmented assignment (*=) ===
lst = [1, 2]
lst *= 2
assert lst == [1, 2, 1, 2]

lst = [1]
lst *= 0
assert lst == []

# === list() constructor ===
assert list() == []
assert list([1, 2, 3]) == [1, 2, 3]
assert list((1, 2, 3)) == [1, 2, 3]
assert list(range(3)) == [0, 1, 2]
assert list('abc') == ['a', 'b', 'c']
assert list(b'abc') == [97, 98, 99]
assert list({'a': 1, 'b': 2}) == ['a', 'b']

# non-ASCII strings (multi-byte UTF-8)
assert list('héllo') == ['h', 'é', 'l', 'l', 'o']
assert list('日本') == ['日', '本']
assert list('a🎉b') == ['a', '🎉', 'b']

# === list.append() ===
lst = []
lst.append(1)
assert lst == [1]
lst.append(2)
assert lst == [1, 2]
lst.append(lst)  # append self creates cycle
assert len(lst) == 3

# === list.insert() ===
# Basic insert at various positions
lst = [1, 2, 3]
lst.insert(0, 'a')
assert lst == ['a', 1, 2, 3]

lst = [1, 2, 3]
lst.insert(1, 'a')
assert lst == [1, 'a', 2, 3]

lst = [1, 2, 3]
lst.insert(3, 'a')
assert lst == [1, 2, 3, 'a']

# Insert beyond length appends
lst = [1, 2, 3]
lst.insert(100, 'a')
assert lst == [1, 2, 3, 'a']

# Insert with negative index
lst = [1, 2, 3]
lst.insert(-1, 'a')
assert lst == [1, 2, 'a', 3]

lst = [1, 2, 3]
lst.insert(-2, 'a')
assert lst == [1, 'a', 2, 3]

lst = [1, 2, 3]
lst.insert(-100, 'a')
assert lst == ['a', 1, 2, 3]

# === list.pop() ===
lst = [1, 2, 3]
assert lst.pop() == 3
assert lst == [1, 2]

lst = [1, 2, 3]
assert lst.pop(0) == 1
assert lst == [2, 3]

lst = [1, 2, 3]
assert lst.pop(1) == 2
assert lst == [1, 3]

lst = [1, 2, 3]
assert lst.pop(-1) == 3
assert lst == [1, 2]

lst = [1, 2, 3]
assert lst.pop(-2) == 2
assert lst == [1, 3]

# === list.remove() ===
lst = [1, 2, 3, 2]
lst.remove(2)
assert lst == [1, 3, 2]

lst = ['a', 'b', 'c']
lst.remove('b')
assert lst == ['a', 'c']

# === list.clear() ===
lst = [1, 2, 3]
lst.clear()
assert lst == []

lst = []
lst.clear()
assert lst == []

# === list.copy() ===
lst = [1, 2, 3]
copy = lst.copy()
assert copy == [1, 2, 3]
assert copy is not lst
lst.append(4)
assert copy == [1, 2, 3]

# === list.extend() ===
lst = [1, 2]
lst.extend([3, 4])
assert lst == [1, 2, 3, 4]

lst = [1]
lst.extend((2, 3))
assert lst == [1, 2, 3]

lst = [1]
lst.extend(range(2, 5))
assert lst == [1, 2, 3, 4]

lst = [1]
lst.extend('ab')
assert lst == [1, 'a', 'b']

lst = []
lst.extend([])
assert lst == []

# === list.index() ===
lst = [1, 2, 3, 2]
assert lst.index(2) == 1
assert lst.index(3) == 2
assert lst.index(2, 2) == 3
assert lst.index(2, 1, 4) == 1

# Regression: `-index` on i64::MIN used to panic when normalising start/end
_I64_MIN = -(2**63)
assert lst.index(1, _I64_MIN) == 0
assert lst.index(2, _I64_MIN, 4) == 1

# === list.count() ===
lst = [1, 2, 2, 3, 2]
assert lst.count(2) == 3
assert lst.count(1) == 1
assert lst.count(4) == 0
assert [].count(1) == 0

# === list.reverse() ===
lst = [1, 2, 3]
lst.reverse()
assert lst == [3, 2, 1]

lst = [1]
lst.reverse()
assert lst == [1]

lst = []
lst.reverse()
assert lst == []

# === list.sort() ===
lst = [3, 1, 2]
lst.sort()
assert lst == [1, 2, 3]

lst = ['b', 'c', 'a']
lst.sort()
assert lst == ['a', 'b', 'c']

lst = [3, 1, 2]
lst.sort(reverse=True)
assert lst == [3, 2, 1]

lst = []
lst.sort()
assert lst == []

lst = [1]
lst.sort()
assert lst == [1]

# === list.sort(key=...) ===
lst = ['banana', 'apple', 'cherry']
lst.sort(key=len)
assert lst == ['apple', 'banana', 'cherry']

lst = [[1, 2, 3], [4], [5, 6]]
lst.sort(key=len)
assert lst == [[4], [5, 6], [1, 2, 3]]

lst = [[1, 2, 3], [4], [5, 6]]
lst.sort(key=len, reverse=True)
assert lst == [[1, 2, 3], [5, 6], [4]]

lst = [-3, 1, -2, 4]
lst.sort(key=abs)
assert lst == [1, -2, -3, 4]

# key=None is same as no key
lst = [3, 1, 2]
lst.sort(key=None)
assert lst == [1, 2, 3]

lst = [3, 1, 2]
lst.sort(key=None, reverse=True)
assert lst == [3, 2, 1]

# Empty list with key
lst = []
lst.sort(key=len)
assert lst == []

# key=int for string-to-int conversion
lst = ['-3', '1', '-2', '4']
lst.sort(key=int)
assert lst == ['-3', '-2', '1', '4']

lst = ['10', '2', '1', '100']
lst.sort(key=int)
assert lst == ['1', '2', '10', '100']

lst = ['10', '2', '1', '100']
lst.sort(key=int, reverse=True)
assert lst == ['100', '10', '2', '1']

# user-defined key function


def last_char(s):
    return s[-1]


lst = ['cherry', 'banana', 'apple']
lst.sort(key=last_char)
assert lst == ['banana', 'apple', 'cherry']


# key function might raise exception
lst = ['']
try:
    lst.sort(key=last_char)
except IndexError:
    pass  # expected since last_char('') raises IndexError


# === list.sort() reentrant mutation by key callback (issue #411) ===
# CPython detaches the list during sort so reentrant access sees an empty
# list. If the user re-populates the live list, sort raises ValueError after
# restoring the detached (sorted) buffer.

# Key callback observes empty list during sort
xs1 = [3, 2, 1]


def empty_key(value):
    assert len(xs1) == 0
    return value


xs1.sort(key=empty_key)
assert xs1 == [1, 2, 3]

# Repopulating the list during sort must raise ValueError
xs2 = [3, 2, 1]


def repopulate_key(value):
    xs2.append(99)
    return value


try:
    xs2.sort(key=repopulate_key)
    assert False, 'expected ValueError when key callback repopulates the list'
except ValueError as exc:
    assert str(exc) == 'list modified during sort'
assert xs2 == [1, 2, 3]


# === List assignment (setitem) ===
# Basic assignment
lst = [1, 2, 3]
lst[0] = 10
assert lst == [10, 2, 3]

lst = [1, 2, 3]
lst[1] = 20
assert lst == [1, 20, 3]

lst = [1, 2, 3]
lst[2] = 30
assert lst == [1, 2, 30]

# Negative index assignment
lst = [1, 2, 3]
lst[-1] = 100
assert lst == [1, 2, 100]

lst = [1, 2, 3]
lst[-2] = 200
assert lst == [1, 200, 3]

lst = [1, 2, 3]
lst[-3] = 300
assert lst == [300, 2, 3]

# Assigning different types
lst = [1, 2, 3]
lst[0] = 'hello'
assert lst == ['hello', 2, 3]

lst = [1, 2, 3]
lst[1] = [4, 5]
assert lst == [1, [4, 5], 3]

lst = [1, 2, 3]
lst[0] = None
assert lst == [None, 2, 3]

# Multiple assignments
lst = [0, 0, 0]
lst[0] = 1
lst[1] = 2
lst[2] = 3
assert lst == [1, 2, 3]

# Assignment preserves other elements
lst = ['a', 'b', 'c', 'd']
lst[1] = 'B'
assert lst[0] == 'a'
assert lst[1] == 'B'
assert lst[2] == 'c'
assert lst[3] == 'd'

# === Bool indices ===
# Python allows True/False as indices (True=1, False=0)
lst = ['a', 'b', 'c']
assert lst[False] == 'a'
assert lst[True] == 'b'

lst = ['x', 'y', 'z']
lst[False] = 'X'
assert lst == ['X', 'y', 'z']

lst = ['x', 'y', 'z']
lst[True] = 'Y'
assert lst == ['x', 'Y', 'z']

# === Nested list equality ===
# same-length lists with matching nested elements
assert [[1, 2], [3, 4]] == [[1, 2], [3, 4]]
# same-length but different nested elements (exercises py_eq early return)
assert [[1, 2], [3, 4]] != [[1, 2], [3, 5]]
assert [[]] != [[1]]
# deeper nesting
assert [[[1]]] == [[[1]]]
assert [[[1]]] != [[[2]]]
# mixed nesting depths
assert [[1], 2] == [[1], 2]
assert [[1], 2] != [[1], 3]

# === Nested list repr ===
assert repr([[1, 2], [3, 4]]) == '[[1, 2], [3, 4]]'
assert repr([[]]) == '[[]]'
assert repr([[1], [2, 3]]) == '[[1], [2, 3]]'

# === list.remove() with nested elements ===
x = [1, 2]
lst = [x, [3, 4], x]
lst.remove([1, 2])
assert lst == [[3, 4], [1, 2]]

lst = [1, [2, 3], 4]
lst.remove([2, 3])
assert lst == [1, 4]

# === list.index() with nested elements ===
lst = [[3], [1, 2], [4]]
assert lst.index([1, 2]) == 1

lst = [[1], [2], [1]]
assert lst.index([1]) == 0

# === list.count() with nested elements ===
lst = [[1, 2], [3], [1, 2], 4, [1, 2]]
assert lst.count([1, 2]) == 3
assert lst.count([3]) == 1
assert lst.count([99]) == 0
assert [].count([1]) == 0

# === Nested list containment ===
assert [1, 2] in [[1, 2], [3, 4]]
assert [5, 6] not in [[1, 2], [3, 4]]
assert [] in [[], [1]]

# === List unpacking (PEP 448) ===
a = [1, 2]
b = [3, 4]
assert [*a] == [1, 2]
assert [*a, *b] == [1, 2, 3, 4]
assert [0, *a, 5] == [0, 1, 2, 5]
assert [*[]] == []
assert [*(1, 2)] == [1, 2]
assert [*'abc'] == ['a', 'b', 'c']
assert [*{'x': 1, 'y': 2}] == ['x', 'y']
# Heap-allocated set: covers the HeapData::Set arm in list_extend
assert sorted([*{1, 2, 3}]) == [1, 2, 3]
# Heap-allocated Str (result of concat, not interned): covers HeapData::Str in list_extend
hs = 'hel' + 'lo'
assert [*hs] == ['h', 'e', 'l', 'l', 'o']


# Non-iterable heap-allocated Ref (closure) hits the inner `_` arm in list_extend.
# A plain top-level function is Value::DefFunction (not a Ref), so a closure is
# required to reach the Value::Ref(_) branch (HeapData that is not List/Tuple/Set/Dict/Str).
def _make_list_unpack_closure():
    _sentinel = 1

    def _inner():
        return _sentinel

    return _inner


_list_unpack_closure = _make_list_unpack_closure()
try:
    _x = [*_list_unpack_closure]
    assert False, 'expected TypeError for non-iterable heap closure in list unpack'
except TypeError:
    pass

# === Nested subscript assignment ===
a = [[1, 2, 3], [4, 5, 6]]
a[0][2] = 99
assert a[0][2] == 99
assert a == [[1, 2, 99], [4, 5, 6]]

# === Nested subscript augmented assignment ===
a = [[1, 2, 3]]
a[0][2] += 1
assert a == [[1, 2, 4]]

a = [[10, 20], [30, 40]]
a[1][0] -= 5
assert a == [[10, 20], [25, 40]]

# === Triple nesting ===
a = [[[0]]]
a[0][0][0] = 7
assert a[0][0][0] == 7

a = [[[10]]]
a[0][0][0] += 1
assert a[0][0][0] == 11

# === Mixed dict-list nesting ===
d = {'k': [1, 2, 3]}
d['k'][0] = 100
assert d['k'] == [100, 2, 3]

d = {'k': [1, 2, 3]}
d['k'][0] += 100
assert d['k'] == [101, 2, 3]

# === Nested dict assignment ===
d = {'a': {'x': 1, 'y': 2}}
d['a']['y'] = 42
assert d['a']['y'] == 42

d = {'a': {'x': 1}}
d['a']['x'] += 10
assert d['a']['x'] == 11

# === Eval-once semantics for augmented subscript assignment ===
# CPython evaluates the container and index expressions exactly once,
# in left-to-right order. Verify Monty matches this behavior.
_eval_log = []


def _tracking_obj():
    _eval_log.append('obj')
    return [10, 20, 30]


def _tracking_index():
    _eval_log.append('idx')
    return 1


_tracking_obj()[_tracking_index()] += 100
assert _eval_log == ['obj', 'idx'], f'eval-once order: {_eval_log}'

# Also verify the assignment itself is correct (even though the list is temporary)
_result_list = [10, 20, 30]
_eval_log.clear()


def _tracking_obj2():
    _eval_log.append('obj')
    return _result_list


def _tracking_index2():
    _eval_log.append('idx')
    return 2


_tracking_obj2()[_tracking_index2()] += 7
assert _eval_log == ['obj', 'idx'], f'eval-once order with persistent list: {_eval_log}'
assert _result_list == [10, 20, 37], f'augmented assign via function: {_result_list}'
