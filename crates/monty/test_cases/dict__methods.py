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

# === dict.fromkeys() ===
d = dict.fromkeys(['a', 'b', 'c'])
assert d == {'a': None, 'b': None, 'c': None}, 'fromkeys with list, default None'

d = dict.fromkeys(['a', 'b'], 0)
assert d == {'a': 0, 'b': 0}, 'fromkeys with default value'

d = dict.fromkeys([])
assert d == {}, 'fromkeys with empty iterable'

d = dict.fromkeys('abc')
assert d == {'a': None, 'b': None, 'c': None}, 'fromkeys with string iterable'

d = dict.fromkeys(range(3), 'x')
assert d == {0: 'x', 1: 'x', 2: 'x'}, 'fromkeys with range iterable'

d = dict.fromkeys((1, 2, 3), [])
assert d[1] is d[2] and d[2] is d[3], 'fromkeys shares same value object for all keys'

# Duplicate keys - later occurrence wins
d = dict.fromkeys(['a', 'b', 'a'], 1)
assert d == {'a': 1, 'b': 1}, 'fromkeys with duplicate keys'
assert list(d.keys()) == ['a', 'b'], 'fromkeys preserves first occurrence order'

# === dict.fromkeys() instance access ===
# fromkeys is a classmethod but should also work on instances
d = {}.fromkeys(['a', 'b'])
assert d == {'a': None, 'b': None}, 'fromkeys on empty dict instance'

d = {'x': 1}.fromkeys(['a', 'b'], 0)
assert d == {'a': 0, 'b': 0}, 'fromkeys on non-empty dict instance (ignores original)'

# === dict.update() with keyword arguments ===
d = {'a': 1}
d.update(b=2)
assert d == {'a': 1, 'b': 2}, 'update with single kwarg'

d = {'a': 1}
d.update(b=2, c=3)
assert d == {'a': 1, 'b': 2, 'c': 3}, 'update with multiple kwargs'

d = {'a': 1}
d.update(a=10)
assert d == {'a': 10}, 'update kwarg overwrites existing key'

d = {}
d.update(a=1, b=2)
assert d == {'a': 1, 'b': 2}, 'update empty dict with kwargs'

# update with both positional dict and kwargs
d = {'a': 1}
d.update({'b': 2}, c=3)
assert d == {'a': 1, 'b': 2, 'c': 3}, 'update with dict and kwargs'

# kwargs overwrite positional dict values
d = {'a': 1}
d.update({'b': 2}, b=20)
assert d == {'a': 1, 'b': 20}, 'update kwargs overwrite positional dict'

# update with iterable and kwargs
d = {}
d.update([('a', 1)], b=2)
assert d == {'a': 1, 'b': 2}, 'update with iterable and kwargs'

# === Error message for unknown classmethod ===
# Error message should say 'dict' not 'type'
try:
    dict.nonexistent()
    assert False, 'should raise AttributeError'
except AttributeError as e:
    msg = str(e)
    assert 'dict' in msg, f'error should mention dict, got: {e}'
    assert 'nonexistent' in msg, f'error should mention method name, got: {e}'

# === dict.update() sequence element error ===
# Invalid sequence elements should raise ValueError
try:
    d = {}
    d.update([('a', 1), 'x', ('c', 3)])  # 'x' at index 1 is not a 2-tuple
    assert False, 'should raise ValueError'
except (ValueError, TypeError) as e:
    msg = str(e)
    # Error message should mention 'length' requirement
    assert 'length' in msg.lower(), f'error should mention length, got: {e}'
    # TODO: CPython includes element index (#N) in error message
    # assert '#1' in msg, 'error should mention element index'
