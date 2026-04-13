# === Type identity and repr ===
d = {'a': 1, 'b': 2}

keys = d.keys()
items = d.items()
values = d.values()

assert type(keys).__name__ == 'dict_keys', 'dict.keys() returns a dict_keys view'
assert type(items).__name__ == 'dict_items', 'dict.items() returns a dict_items view'
assert type(values).__name__ == 'dict_values', 'dict.values() returns a dict_values view'

assert repr(keys) == "dict_keys(['a', 'b'])", 'keys repr matches CPython'
assert repr(items) == "dict_items([('a', 1), ('b', 2)])", 'items repr matches CPython'
assert repr(values) == 'dict_values([1, 2])', 'values repr matches CPython'

# === len() and truthiness ===
assert len(keys) == 2, 'keys view reports live dict length'
assert len(items) == 2, 'items view reports live dict length'
assert len(values) == 2, 'values view reports live dict length'
assert bool(keys) is True, 'non-empty keys view is truthy'
assert bool(items) is True, 'non-empty items view is truthy'
assert bool(values) is True, 'non-empty values view is truthy'
assert bool({}.keys()) is False, 'empty keys view is falsy'
assert bool({}.items()) is False, 'empty items view is falsy'
assert bool({}.values()) is False, 'empty values view is falsy'

# === Iteration order ===
assert list(keys) == ['a', 'b'], 'keys iterate in insertion order'
assert list(items) == [('a', 1), ('b', 2)], 'items iterate in insertion order'
assert list(values) == [1, 2], 'values iterate in insertion order'

# === Membership ===
assert ('a' in keys) is True, 'keys membership checks keys'
assert ('missing' in keys) is False, 'keys membership is false for absent keys'
assert (('a', 1) in items) is True, 'items membership matches existing pairs'
assert (('a', 3) in items) is False, 'items membership checks values too'
assert (('a',) in items) is False, 'items membership ignores non-2-tuples'
assert (1 in values) is True, 'values membership checks stored values'
assert (3 in values) is False, 'values membership is false for absent values'

try:
    ([1], 'x') in {1: 'x'}.items()
    assert False, 'items membership should reject unhashable keys'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a dict key (unhashable type: 'list')", (
        'items membership propagates key hash errors'
    )

# === Equality ===
assert keys == keys, 'keys view equals itself'
assert items == items, 'items view equals itself'
assert values == values, 'values view equals itself by identity'

assert keys == {'a', 'b'}, 'keys view compares equal to matching sets'
assert {'b', 'a'} == keys, 'set equality works when dict_keys is on the right'
assert keys == frozenset({'a', 'b'}), 'keys view compares equal to matching frozensets'
assert frozenset({'a', 'b'}) == keys, 'frozenset equality works when dict_keys is on the right'
assert keys == {'b': 0, 'a': 9}.keys(), 'keys view compares equal to another matching keys view'
assert keys != {'a'}, 'keys view equality checks the full key set'
assert keys != {'a', 'x'}, 'keys view inequality checks equal-length mismatches'

assert items == {('a', 1), ('b', 2)}, 'items view compares equal to matching sets'
assert {('b', 2), ('a', 1)} == items, 'set equality works when dict_items is on the right'
assert items == frozenset({('a', 1), ('b', 2)}), 'items view compares equal to matching frozensets'
assert frozenset({('a', 1), ('b', 2)}) == items, 'frozenset equality works when dict_items is on the right'
assert items == {'b': 2, 'a': 1}.items(), 'items view compares equal to another matching items view'
assert items != {('a', 1)}, 'items view equality checks the full item set'
assert items != {('a', 2), ('b', 2)}, 'items view equality checks values as well as keys'
assert items != {('a', 1), ('x', 9)}, 'items view inequality checks equal-length mismatches'
assert ({'a': 1}.values() == {'a': 1}.values()) is False, 'distinct values views are never equal'

# === Live behavior after mutation ===
live = {'x': 10}
live_keys = live.keys()
live_items = live.items()
live_values = live.values()
live['y'] = 20

assert list(live_keys) == ['x', 'y'], 'keys view sees later insertions'
assert list(live_items) == [('x', 10), ('y', 20)], 'items view sees later insertions'
assert list(live_values) == [10, 20], 'values view sees later insertions'
assert repr(live_keys) == "dict_keys(['x', 'y'])", 'keys repr stays live after mutation'
assert len(live_values) == 2, 'values len updates after mutation'

# === Dict mutation during iteration ===
changing = {'a': 1, 'b': 2}
changing_iter = iter(changing.keys())
assert next(changing_iter) == 'a', 'iterator yields the first key before mutation'
changing['c'] = 3
try:
    next(changing_iter)
    assert False, 'changing dict size during keys iteration should raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration', 'keys iteration error matches CPython'

changing = {'a': 1, 'b': 2}
changing_iter = iter(changing.items())
assert next(changing_iter) == ('a', 1), 'iterator yields the first item before mutation'
changing['c'] = 3
try:
    next(changing_iter)
    assert False, 'changing dict size during items iteration should raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration', 'items iteration error matches CPython'

changing = {'a': 1, 'b': 2}
changing_iter = iter(changing.values())
assert next(changing_iter) == 1, 'iterator yields the first value before mutation'
changing['c'] = 3
try:
    next(changing_iter)
    assert False, 'changing dict size during values iteration should raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration', 'values iteration error matches CPython'

# === dict_keys & iterable ===
d = {'a': 1, 'b': 2, 'c': 3}
assert d.keys() & {'b', 'c', 'x'} == {'b', 'c'}, 'keys view intersects sets'
assert d.keys() & ('b', 'x', 'a') == {'a', 'b'}, 'keys view intersects tuples'
assert d.keys() & iter(['c', 'c', 'a']) == {'a', 'c'}, 'keys view intersects iterators'
assert type(d.keys() & {'a'}).__name__ == 'set', 'keys intersection returns a plain set'

try:
    d.keys() & 1
    assert False, 'keys intersection should reject non-iterables'
except TypeError as e:
    assert str(e) == "'int' object is not iterable", 'non-iterable rhs error matches CPython'

# === dict_keys set-like operators ===
assert d.keys() | ('c', 'd') == {'a', 'b', 'c', 'd'}, 'keys view unions arbitrary iterables'
assert d.keys() ^ ('b', 'd', 'e') == {'a', 'c', 'd', 'e'}, 'keys view symmetric difference works'
assert d.keys() - ('b', 'd') == {'a', 'c'}, 'keys view difference works'
assert d.keys() & {'b': 0, 'z': 9}.keys() == {'b'}, 'keys view intersects other keys views'
assert d.keys() | {'c': 0, 'd': 1}.keys() == {'a', 'b', 'c', 'd'}, 'keys view unions other keys views'
assert d.keys().isdisjoint(['x', 'y']) is True, 'keys isdisjoint accepts arbitrary iterables'
assert d.keys().isdisjoint(iter(['x', 'a'])) is False, 'keys isdisjoint consumes iterators'

# === dict_items set-like operators ===
items_dict = {'a': 1, 'b': 2}
assert items_dict.items() & [('a', 1), ('x', 9)] == {('a', 1)}, 'items view intersects iterables of pairs'
assert items_dict.items() | [('c', 3)] == {('a', 1), ('b', 2), ('c', 3)}, 'items view unions iterables of pairs'
assert items_dict.items() ^ [('a', 1), ('c', 3)] == {('b', 2), ('c', 3)}, 'items view symmetric difference works'
assert items_dict.items() - [('a', 1)] == {('b', 2)}, 'items view difference works'
assert items_dict.items() & {'b': 2, 'x': 9}.items() == {('b', 2)}, 'items view intersects other items views'
assert items_dict.items().isdisjoint([('x', 1)]) is True, 'items isdisjoint accepts arbitrary iterables'
assert items_dict.items().isdisjoint(iter([('a', 1)])) is False, 'items isdisjoint consumes iterators'

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
assert common_ids == {'merve'}, 'dict_keys & set supports the motivating use case'
