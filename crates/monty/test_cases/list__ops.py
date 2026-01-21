# === List concatenation (+) ===
assert [1, 2] + [3, 4] == [1, 2, 3, 4], 'basic concat'
assert [] + [1, 2] == [1, 2], 'empty left concat'
assert [1, 2] + [] == [1, 2], 'empty right concat'
assert [] + [] == [], 'empty both concat'
assert [1] + [2] + [3] + [4] == [1, 2, 3, 4], 'multiple concat'
assert [[1]] + [[2]] == [[1], [2]], 'nested concat'

# === Augmented assignment (+=) ===
lst = [1, 2]
lst += [3, 4]
assert lst == [1, 2, 3, 4], 'basic iadd'

lst = [1]
lst += []
assert lst == [1], 'iadd empty'

lst = [1]
lst += [2]
lst += [3]
assert lst == [1, 2, 3], 'multiple iadd'

lst = [1, 2]
lst += lst
assert lst == [1, 2, 1, 2], 'iadd self'

# === List length ===
assert len([]) == 0, 'len empty'
assert len([1, 2, 3]) == 3, 'len basic'

lst = [1]
lst.append(2)
assert len(lst) == 2, 'len after append'

# === List indexing ===
a = []
a.append('value')
assert a[0] == 'value', 'getitem basic'

a = [1, 2, 3]
assert a[0 - 1] == 3, 'getitem negative index'
assert a[-1] == 3, 'getitem -1'
assert a[-2] == 2, 'getitem -2'

# === List repr/str ===
assert repr([]) == '[]', 'empty list repr'
assert str([]) == '[]', 'empty list str'

assert repr([1, 2, 3]) == '[1, 2, 3]', 'list repr'
assert str([1, 2, 3]) == '[1, 2, 3]', 'list str'

# === List repetition (*) ===
assert [1, 2] * 3 == [1, 2, 1, 2, 1, 2], 'list mult int'
assert 3 * [1, 2] == [1, 2, 1, 2, 1, 2], 'int mult list'
assert [1] * 0 == [], 'list mult zero'
assert [1] * -1 == [], 'list mult negative'
assert [] * 5 == [], 'empty list mult'
assert [1, 2] * 1 == [1, 2], 'list mult one'
assert [[1]] * 2 == [[1], [1]], 'nested list mult'

# === List repetition augmented assignment (*=) ===
lst = [1, 2]
lst *= 2
assert lst == [1, 2, 1, 2], 'list imult'

lst = [1]
lst *= 0
assert lst == [], 'list imult zero'

# === list() constructor ===
assert list() == [], 'list() empty'
assert list([1, 2, 3]) == [1, 2, 3], 'list from list'
assert list((1, 2, 3)) == [1, 2, 3], 'list from tuple'
assert list(range(3)) == [0, 1, 2], 'list from range'
assert list('abc') == ['a', 'b', 'c'], 'list from string'
assert list(b'abc') == [97, 98, 99], 'list from bytes'
assert list({'a': 1, 'b': 2}) == ['a', 'b'], 'list from dict yields keys'

# non-ASCII strings (multi-byte UTF-8)
assert list('héllo') == ['h', 'é', 'l', 'l', 'o'], 'list from string with accented char'
assert list('日本') == ['日', '本'], 'list from string with CJK chars'
assert list('a🎉b') == ['a', '🎉', 'b'], 'list from string with emoji'

# === list.append() ===
lst = []
lst.append(1)
assert lst == [1], 'append to empty'
lst.append(2)
assert lst == [1, 2], 'append to non-empty'
lst.append(lst)  # append self creates cycle
assert len(lst) == 3, 'append self increases length'

# === list.insert() ===
# Basic insert at various positions
lst = [1, 2, 3]
lst.insert(0, 'a')
assert lst == ['a', 1, 2, 3], 'insert at beginning'

lst = [1, 2, 3]
lst.insert(1, 'a')
assert lst == [1, 'a', 2, 3], 'insert in middle'

lst = [1, 2, 3]
lst.insert(3, 'a')
assert lst == [1, 2, 3, 'a'], 'insert at end'

# Insert beyond length appends
lst = [1, 2, 3]
lst.insert(100, 'a')
assert lst == [1, 2, 3, 'a'], 'insert beyond length appends'

# Insert with negative index
lst = [1, 2, 3]
lst.insert(-1, 'a')
assert lst == [1, 2, 'a', 3], 'insert at -1 (before last)'

lst = [1, 2, 3]
lst.insert(-2, 'a')
assert lst == [1, 'a', 2, 3], 'insert at -2'

lst = [1, 2, 3]
lst.insert(-100, 'a')
assert lst == ['a', 1, 2, 3], 'insert very negative clamps to 0'

# === list.pop() ===
lst = [1, 2, 3]
assert lst.pop() == 3, 'pop without argument returns last'
assert lst == [1, 2], 'pop removes last element'

lst = [1, 2, 3]
assert lst.pop(0) == 1, 'pop(0) returns first'
assert lst == [2, 3], 'pop(0) removes first element'

lst = [1, 2, 3]
assert lst.pop(1) == 2, 'pop(1) returns middle'
assert lst == [1, 3], 'pop(1) removes middle element'

lst = [1, 2, 3]
assert lst.pop(-1) == 3, 'pop(-1) returns last'
assert lst == [1, 2], 'pop(-1) removes last element'

lst = [1, 2, 3]
assert lst.pop(-2) == 2, 'pop(-2) returns second to last'
assert lst == [1, 3], 'pop(-2) removes second to last element'

# === list.remove() ===
lst = [1, 2, 3, 2]
lst.remove(2)
assert lst == [1, 3, 2], 'remove removes first occurrence'

lst = ['a', 'b', 'c']
lst.remove('b')
assert lst == ['a', 'c'], 'remove string element'

# === list.clear() ===
lst = [1, 2, 3]
lst.clear()
assert lst == [], 'clear empties the list'

lst = []
lst.clear()
assert lst == [], 'clear on empty list is no-op'

# === list.copy() ===
lst = [1, 2, 3]
copy = lst.copy()
assert copy == [1, 2, 3], 'copy creates equal list'
assert copy is not lst, 'copy creates new list object'
lst.append(4)
assert copy == [1, 2, 3], 'copy is independent'

# === list.extend() ===
lst = [1, 2]
lst.extend([3, 4])
assert lst == [1, 2, 3, 4], 'extend with list'

lst = [1]
lst.extend((2, 3))
assert lst == [1, 2, 3], 'extend with tuple'

lst = [1]
lst.extend(range(2, 5))
assert lst == [1, 2, 3, 4], 'extend with range'

lst = [1]
lst.extend('ab')
assert lst == [1, 'a', 'b'], 'extend with string'

lst = []
lst.extend([])
assert lst == [], 'extend empty with empty'

# === list.index() ===
lst = [1, 2, 3, 2]
assert lst.index(2) == 1, 'index finds first occurrence'
assert lst.index(3) == 2, 'index finds element'
assert lst.index(2, 2) == 3, 'index with start'
assert lst.index(2, 1, 4) == 1, 'index with start and end'

# === list.count() ===
lst = [1, 2, 2, 3, 2]
assert lst.count(2) == 3, 'count multiple occurrences'
assert lst.count(1) == 1, 'count single occurrence'
assert lst.count(4) == 0, 'count zero occurrences'
assert [].count(1) == 0, 'count on empty list'

# === list.reverse() ===
lst = [1, 2, 3]
lst.reverse()
assert lst == [3, 2, 1], 'reverse modifies in place'

lst = [1]
lst.reverse()
assert lst == [1], 'reverse single element'

lst = []
lst.reverse()
assert lst == [], 'reverse empty list'

# === list.sort() ===
lst = [3, 1, 2]
lst.sort()
assert lst == [1, 2, 3], 'sort integers'

lst = ['b', 'c', 'a']
lst.sort()
assert lst == ['a', 'b', 'c'], 'sort strings'

lst = [3, 1, 2]
lst.sort(reverse=True)
assert lst == [3, 2, 1], 'sort with reverse=True'

lst = []
lst.sort()
assert lst == [], 'sort empty list'

lst = [1]
lst.sort()
assert lst == [1], 'sort single element'

# === list.sort(key=...) ===
lst = ['banana', 'apple', 'cherry']
lst.sort(key=len)
assert lst == ['apple', 'banana', 'cherry'], 'sort by len'

lst = [[1, 2, 3], [4], [5, 6]]
lst.sort(key=len)
assert lst == [[4], [5, 6], [1, 2, 3]], 'sort nested lists by len'

lst = [[1, 2, 3], [4], [5, 6]]
lst.sort(key=len, reverse=True)
assert lst == [[1, 2, 3], [5, 6], [4]], 'sort by len reverse'

lst = [-3, 1, -2, 4]
lst.sort(key=abs)
assert lst == [1, -2, -3, 4], 'sort by abs'

# key=None is same as no key
lst = [3, 1, 2]
lst.sort(key=None)
assert lst == [1, 2, 3], 'sort with key=None'

lst = [3, 1, 2]
lst.sort(key=None, reverse=True)
assert lst == [3, 2, 1], 'sort with key=None reverse'

# Empty list with key
lst = []
lst.sort(key=len)
assert lst == [], 'sort empty list with key'

# key=int for string-to-int conversion
lst = ['-3', '1', '-2', '4']
lst.sort(key=int)
assert lst == ['-3', '-2', '1', '4'], 'sort strings by int value'

lst = ['10', '2', '1', '100']
lst.sort(key=int)
assert lst == ['1', '2', '10', '100'], 'sort numeric strings by int value'

lst = ['10', '2', '1', '100']
lst.sort(key=int, reverse=True)
assert lst == ['100', '10', '2', '1'], 'sort numeric strings by int reverse'

# === List assignment (setitem) ===
# Basic assignment
lst = [1, 2, 3]
lst[0] = 10
assert lst == [10, 2, 3], 'setitem at index 0'

lst = [1, 2, 3]
lst[1] = 20
assert lst == [1, 20, 3], 'setitem at index 1'

lst = [1, 2, 3]
lst[2] = 30
assert lst == [1, 2, 30], 'setitem at last index'

# Negative index assignment
lst = [1, 2, 3]
lst[-1] = 100
assert lst == [1, 2, 100], 'setitem at -1'

lst = [1, 2, 3]
lst[-2] = 200
assert lst == [1, 200, 3], 'setitem at -2'

lst = [1, 2, 3]
lst[-3] = 300
assert lst == [300, 2, 3], 'setitem at -3'

# Assigning different types
lst = [1, 2, 3]
lst[0] = 'hello'
assert lst == ['hello', 2, 3], 'setitem string value'

lst = [1, 2, 3]
lst[1] = [4, 5]
assert lst == [1, [4, 5], 3], 'setitem list value'

lst = [1, 2, 3]
lst[0] = None
assert lst == [None, 2, 3], 'setitem None value'

# Multiple assignments
lst = [0, 0, 0]
lst[0] = 1
lst[1] = 2
lst[2] = 3
assert lst == [1, 2, 3], 'multiple setitem'

# Assignment preserves other elements
lst = ['a', 'b', 'c', 'd']
lst[1] = 'B'
assert lst[0] == 'a', 'setitem preserves element 0'
assert lst[1] == 'B', 'setitem changes element 1'
assert lst[2] == 'c', 'setitem preserves element 2'
assert lst[3] == 'd', 'setitem preserves element 3'

# === Bool indices ===
# Python allows True/False as indices (True=1, False=0)
lst = ['a', 'b', 'c']
assert lst[False] == 'a', 'getitem with False'
assert lst[True] == 'b', 'getitem with True'

lst = ['x', 'y', 'z']
lst[False] = 'X'
assert lst == ['X', 'y', 'z'], 'setitem with False'

lst = ['x', 'y', 'z']
lst[True] = 'Y'
assert lst == ['x', 'Y', 'z'], 'setitem with True'
