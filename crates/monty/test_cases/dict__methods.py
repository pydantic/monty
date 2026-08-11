# === dict.clear() ===
d = {'a': 1, 'b': 2}
d.clear()
assert d == {}

d = {}
d.clear()
assert d == {}

# === dict.copy() ===
d = {'a': 1, 'b': 2}
copy = d.copy()
assert copy == {'a': 1, 'b': 2}
assert copy is not d
d['c'] = 3
assert 'c' not in copy

d = {}
copy = d.copy()
assert copy == {}

# === dict.update() ===
d = {'a': 1}
d.update({'b': 2})
assert d == {'a': 1, 'b': 2}

d = {'a': 1}
d.update({'a': 10})
assert d == {'a': 10}

d = {'a': 1}
d.update()
assert d == {'a': 1}

d = {}
d.update([('a', 1), ('b', 2)])
assert d == {'a': 1, 'b': 2}

# === dict.setdefault() ===
d = {'a': 1}
result = d.setdefault('a', 10)
assert result == 1
assert d == {'a': 1}

d = {'a': 1}
result = d.setdefault('b', 2)
assert result == 2
assert d == {'a': 1, 'b': 2}

d = {'a': 1}
result = d.setdefault('b')
assert result is None
assert d == {'a': 1, 'b': None}

# === dict.popitem() ===
d = {'a': 1, 'b': 2}
item = d.popitem()
assert item == ('b', 2)
assert d == {'a': 1}

d = {'x': 10}
item = d.popitem()
assert item == ('x', 10)
assert d == {}

# === dict.fromkeys() ===
d = dict.fromkeys(['a', 'b', 'c'])
assert d == {'a': None, 'b': None, 'c': None}

d = dict.fromkeys(['a', 'b'], 0)
assert d == {'a': 0, 'b': 0}

d = dict.fromkeys([])
assert d == {}

d = dict.fromkeys('abc')
assert d == {'a': None, 'b': None, 'c': None}

d = dict.fromkeys(range(3), 'x')
assert d == {0: 'x', 1: 'x', 2: 'x'}

d = dict.fromkeys((1, 2, 3), [])
assert d[1] is d[2] and d[2] is d[3], 'fromkeys shares same value object for all keys'

# Duplicate keys - later occurrence wins
d = dict.fromkeys(['a', 'b', 'a'], 1)
assert d == {'a': 1, 'b': 1}
assert list(d.keys()) == ['a', 'b']

# === dict.fromkeys() instance access ===
# fromkeys is a classmethod but should also work on instances
d = {}.fromkeys(['a', 'b'])
assert d == {'a': None, 'b': None}

d = {'x': 1}.fromkeys(['a', 'b'], 0)
assert d == {'a': 0, 'b': 0}

# === dict.update() with keyword arguments ===
d = {'a': 1}
d.update(b=2)
assert d == {'a': 1, 'b': 2}

d = {'a': 1}
d.update(b=2, c=3)
assert d == {'a': 1, 'b': 2, 'c': 3}

d = {'a': 1}
d.update(a=10)
assert d == {'a': 10}

d = {}
d.update(a=1, b=2)
assert d == {'a': 1, 'b': 2}

# update with both positional dict and kwargs
d = {'a': 1}
d.update({'b': 2}, c=3)
assert d == {'a': 1, 'b': 2, 'c': 3}

# kwargs overwrite positional dict values
d = {'a': 1}
d.update({'b': 2}, b=20)
assert d == {'a': 1, 'b': 20}

# update with iterable and kwargs
d = {}
d.update([('a', 1)], b=2)
assert d == {'a': 1, 'b': 2}

# `**` unpacking with a runtime-built (non-interned) string key must still
# reach **kwargs — only genuinely non-string keys are rejected.
runtime_key = 'zzz' + 'qqq'
d = dict(**{runtime_key: 1})
assert d == {'zzzqqq': 1}
d.update(**{runtime_key: 2})
assert d == {'zzzqqq': 2}

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
