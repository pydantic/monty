# === dict.clear() ===
d = {'a': 1, 'b': 2}
d.clear()
assert d == {}, 'clear empties the dict'

d = {}
d.clear()
assert d == {}, 'clear on empty dict is no-op'

# === dict.copy() ===
d = {'a': 1, 'b': 2}
copy = d.copy()
assert copy == {'a': 1, 'b': 2}, 'copy creates equal dict'
assert copy is not d, 'copy creates new dict object'
d['c'] = 3
assert 'c' not in copy, 'copy is independent'

d = {}
copy = d.copy()
assert copy == {}, 'copy empty dict'

# === dict.update() ===
d = {'a': 1}
d.update({'b': 2})
assert d == {'a': 1, 'b': 2}, 'update with dict'

d = {'a': 1}
d.update({'a': 10})
assert d == {'a': 10}, 'update overwrites existing key'

d = {'a': 1}
d.update()
assert d == {'a': 1}, 'update with no args is no-op'

d = {}
d.update([('a', 1), ('b', 2)])
assert d == {'a': 1, 'b': 2}, 'update with list of tuples'

# === dict.setdefault() ===
d = {'a': 1}
result = d.setdefault('a', 10)
assert result == 1, 'setdefault returns existing value'
assert d == {'a': 1}, 'setdefault does not overwrite'

d = {'a': 1}
result = d.setdefault('b', 2)
assert result == 2, 'setdefault returns default for new key'
assert d == {'a': 1, 'b': 2}, 'setdefault inserts new key'

d = {'a': 1}
result = d.setdefault('b')
assert result is None, 'setdefault default is None'
assert d == {'a': 1, 'b': None}, 'setdefault inserts None'

# === dict.popitem() ===
d = {'a': 1, 'b': 2}
item = d.popitem()
assert item == ('b', 2), 'popitem returns last inserted item'
assert d == {'a': 1}, 'popitem removes item'

d = {'x': 10}
item = d.popitem()
assert item == ('x', 10), 'popitem on single-item dict'
assert d == {}, 'dict is now empty'
