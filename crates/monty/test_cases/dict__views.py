# === Type identity and repr ===
d = {'a': 1, 'b': 2}

keys = d.keys()
items = d.items()
values = d.values()

assert type(keys).__name__ == 'dict_keys'
assert type(items).__name__ == 'dict_items'
assert type(values).__name__ == 'dict_values'

assert repr(keys) == "dict_keys(['a', 'b'])"
assert repr(items) == "dict_items([('a', 1), ('b', 2)])"
assert repr(values) == 'dict_values([1, 2])'

# === len() and truthiness ===
assert len(keys) == 2
assert len(items) == 2
assert len(values) == 2
assert bool(keys) is True
assert bool(items) is True
assert bool(values) is True
assert bool({}.keys()) is False
assert bool({}.items()) is False
assert bool({}.values()) is False

# === Iteration order ===
assert list(keys) == ['a', 'b']
assert list(items) == [('a', 1), ('b', 2)]
assert list(values) == [1, 2]

# === Membership ===
assert ('a' in keys) is True
assert ('missing' in keys) is False
assert (('a', 1) in items) is True
assert (('a', 3) in items) is False
assert (('a',) in items) is False
assert (1 in values) is True
assert (3 in values) is False

try:
    ([1], 'x') in {1: 'x'}.items()
    assert False, 'items membership should reject unhashable keys'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a dict key (unhashable type: 'list')"

# === Equality ===
assert keys == keys
assert items == items
assert values == values

assert keys == {'a', 'b'}
assert {'b', 'a'} == keys
assert keys == frozenset({'a', 'b'})
assert frozenset({'a', 'b'}) == keys
assert keys == {'b': 0, 'a': 9}.keys()
assert keys != {'a'}
assert keys != {'a', 'x'}

assert items == {('a', 1), ('b', 2)}
assert {('b', 2), ('a', 1)} == items
assert items == frozenset({('a', 1), ('b', 2)})
assert frozenset({('a', 1), ('b', 2)}) == items
assert items == {'b': 2, 'a': 1}.items()
assert items != {('a', 1)}
assert items != {('a', 2), ('b', 2)}
assert items != {('a', 1), ('x', 9)}
assert ({'a': 1}.values() == {'a': 1}.values()) is False

# === Live behavior after mutation ===
live = {'x': 10}
live_keys = live.keys()
live_items = live.items()
live_values = live.values()
live['y'] = 20

assert list(live_keys) == ['x', 'y']
assert list(live_items) == [('x', 10), ('y', 20)]
assert list(live_values) == [10, 20]
assert repr(live_keys) == "dict_keys(['x', 'y'])"
assert len(live_values) == 2

# === Dict mutation during iteration ===
changing = {'a': 1, 'b': 2}
changing_iter = iter(changing.keys())
assert next(changing_iter) == 'a'
changing['c'] = 3
try:
    next(changing_iter)
    assert False, 'changing dict size during keys iteration should raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration'

changing = {'a': 1, 'b': 2}
changing_iter = iter(changing.items())
assert next(changing_iter) == ('a', 1)
changing['c'] = 3
try:
    next(changing_iter)
    assert False, 'changing dict size during items iteration should raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration'

changing = {'a': 1, 'b': 2}
changing_iter = iter(changing.values())
assert next(changing_iter) == 1
changing['c'] = 3
try:
    next(changing_iter)
    assert False, 'changing dict size during values iteration should raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration'

# === dict_keys & iterable ===
d = {'a': 1, 'b': 2, 'c': 3}
assert d.keys() & {'b', 'c', 'x'} == {'b', 'c'}
assert d.keys() & ('b', 'x', 'a') == {'a', 'b'}
assert d.keys() & iter(['c', 'c', 'a']) == {'a', 'c'}
assert type(d.keys() & {'a'}).__name__ == 'set'

try:
    d.keys() & 1
    assert False, 'keys intersection should reject non-iterables'
except TypeError as e:
    assert str(e) == "'int' object is not iterable"

# === dict_keys set-like operators ===
assert d.keys() | ('c', 'd') == {'a', 'b', 'c', 'd'}
assert d.keys() ^ ('b', 'd', 'e') == {'a', 'c', 'd', 'e'}
assert d.keys() - ('b', 'd') == {'a', 'c'}
assert d.keys() & {'b': 0, 'z': 9}.keys() == {'b'}
assert d.keys() | {'c': 0, 'd': 1}.keys() == {'a', 'b', 'c', 'd'}
assert d.keys().isdisjoint(['x', 'y']) is True
assert d.keys().isdisjoint(iter(['x', 'a'])) is False

# === dict_items set-like operators ===
items_dict = {'a': 1, 'b': 2}
assert items_dict.items() & [('a', 1), ('x', 9)] == {('a', 1)}
assert items_dict.items() | [('c', 3)] == {('a', 1), ('b', 2), ('c', 3)}
assert items_dict.items() ^ [('a', 1), ('c', 3)] == {('b', 2), ('c', 3)}
assert items_dict.items() - [('a', 1)] == {('b', 2)}
assert items_dict.items() & {'b': 2, 'x': 9}.items() == {('b', 2)}
assert items_dict.items().isdisjoint([('x', 1)]) is True
assert items_dict.items().isdisjoint(iter([('a', 1)])) is False

# === dict_values remains non-set-like ===
try:
    {'a': 1}.values() & [1]
    assert False, 'dict_values should not support set-like operators'
except TypeError:
    pass

try:
    {'a': 1}.values().isdisjoint([1])
    assert False, 'dict_values should not gain isdisjoint'
except AttributeError:
    pass

# === Motivating milestone example ===
me_map = {'me': 1, 'you': 2, 'merve': 3}
merve_set = {'merve', 'unknown'}
common_ids = me_map.keys() & merve_set
assert common_ids == {'merve'}
