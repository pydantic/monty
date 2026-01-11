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
