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

# Exhausted iterators remain exhausted after the dictionary changes size.
exhausted_dict = {'a': 1}
exhausted_iter = iter(exhausted_dict)
assert list(exhausted_iter) == ['a']
exhausted_dict['b'] = 2
assert next(exhausted_iter, None) is None

# === Unsupported operators do not materialize views ===
# `dict_items` materialization hashes its pairs, so unsupported operators must
# reject the operand types before encountering an unhashable value.
unhashable_values = {'a': [1]}

try:
    unhashable_values.items() + 1
    assert False, 'dict_items should not support +'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for +: 'dict_items' and 'int'", 'items + int names the operand types'

try:
    unhashable_values.items() * 2
    assert False, 'dict_items should not support *'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for *: 'dict_items' and 'int'", 'items * int names the operand types'

try:
    unhashable_values.items() << 1
    assert False, 'dict_items should not support <<'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for <<: 'dict_items' and 'int'", (
        'items << int names the operand types'
    )

try:
    unhashable_values.items() >> 1
    assert False, 'dict_items should not support >>'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for >>: 'dict_items' and 'int'", (
        'items >> int names the operand types'
    )

try:
    1 + unhashable_values.items()
    assert False, 'int + dict_items should not be supported'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for +: 'int' and 'dict_items'", 'reflected + names the operand types'

try:
    2 * unhashable_values.items()
    assert False, 'int * dict_items should not be supported'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for *: 'int' and 'dict_items'", 'reflected * names the operand types'

try:
    unhashable_values.items() - [1]
    assert False, 'items difference over unhashable values should raise'
except TypeError as e:
    assert str(e) == "cannot use 'tuple' as a set element (unhashable type: 'list')", (
        'supported items difference materializes the view'
    )

# Intersection checks the other iterable before hashing any live item values.
assert unhashable_values.items() & [] == set()
assert [] & unhashable_values.items() == set()
for expression in (lambda: unhashable_values.items() & 1, lambda: 1 & unhashable_values.items()):
    try:
        expression()
        assert False, 'items intersection should reject non-iterables first'
    except TypeError as e:
        assert str(e) == "'int' object is not iterable"

# The other set-like operators materialize the items view first.
for expression in (
    lambda: unhashable_values.items() | 1,
    lambda: unhashable_values.items() ^ 1,
):
    try:
        expression()
        assert False, 'items operation should reject unhashable values'
    except TypeError as e:
        assert str(e) == "cannot use 'tuple' as a set element (unhashable type: 'list')"

# Reflected operations validate the left iterable before materializing the view.
for expression in (
    lambda: 1 | unhashable_values.items(),
    lambda: 1 ^ unhashable_values.items(),
    lambda: 1 - unhashable_values.items(),
):
    try:
        expression()
        assert False, 'reflected items operation should reject a non-iterable left operand'
    except TypeError as e:
        assert str(e) == "'int' object is not iterable"

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
assert d.keys() | ('c', 'd') == {'a', 'b', 'c', 'd'}, 'keys view unions arbitrary iterables'
assert ['c', 'd'] | d.keys() == {'a', 'b', 'c', 'd'}, 'keys reflected union accepts arbitrary iterables'
assert d.keys() ^ ('b', 'd', 'e') == {'a', 'c', 'd', 'e'}, 'keys view symmetric difference works'
assert ['b', 'd', 'e'] ^ d.keys() == {'a', 'c', 'd', 'e'}, (
    'keys reflected symmetric difference accepts arbitrary iterables'
)
assert d.keys() - ('b', 'd') == {'a', 'c'}, 'keys view difference works'
assert ['b', 'd'] - d.keys() == {'d'}, 'keys reflected difference preserves operand order'
assert d.keys() & {'b': 0, 'z': 9}.keys() == {'b'}, 'keys view intersects other keys views'
assert d.keys() | {'c': 0, 'd': 1}.keys() == {'a', 'b', 'c', 'd'}, 'keys view unions other keys views'
assert d.keys().isdisjoint(['x', 'y']) is True, 'keys isdisjoint accepts arbitrary iterables'
assert d.keys().isdisjoint(iter(['x', 'a'])) is False, 'keys isdisjoint consumes iterators'

# === dict_items set-like operators ===
items_dict = {'a': 1, 'b': 2}
assert items_dict.items() & [('a', 1), ('x', 9)] == {('a', 1)}, 'items view intersects iterables of pairs'
assert [('a', 1), ('x', 9)] & items_dict.items() == {('a', 1)}, (
    'items reflected intersection accepts iterables of pairs'
)
assert items_dict.items() | [('c', 3)] == {('a', 1), ('b', 2), ('c', 3)}, 'items view unions iterables of pairs'
assert [('c', 3)] | items_dict.items() == {('a', 1), ('b', 2), ('c', 3)}, (
    'items reflected union accepts iterables of pairs'
)
assert items_dict.items() ^ [('a', 1), ('c', 3)] == {('b', 2), ('c', 3)}, 'items view symmetric difference works'
assert [('a', 1), ('c', 3)] ^ items_dict.items() == {('b', 2), ('c', 3)}, 'items reflected symmetric difference works'
assert items_dict.items() - [('a', 1)] == {('b', 2)}, 'items view difference works'
assert [('a', 1), ('c', 3)] - items_dict.items() == {('c', 3)}, 'items reflected difference preserves operand order'
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

# === Motivating milestone examples ===
assert ['foo', 'bar'] | {}.keys() == {'foo', 'bar'}, 'list union empty keys view returns set of list items'

me_map = {'me': 1, 'you': 2, 'merve': 3}
merve_set = {'merve', 'unknown'}
common_ids = me_map.keys() & merve_set
assert common_ids == {'merve'}

# === `in` / `not in` on the views ===
view_source = {'a': 1, 'b': 2}
assert 'a' in view_source.keys()
assert 'z' not in view_source.keys()
assert 2 in view_source.values()
assert 9 not in view_source.values()
# An items probe must be a (key, value) pair; anything else is simply absent.
assert ('a', 1) in view_source.items()
assert ('a', 2) not in view_source.items()
assert 'a' not in view_source.items()

# The values view is a linear scan comparing by value.
nested_view = {'a': [1, 2], 'b': (3, 4), 'c': 1}
assert [1, 2] in nested_view.values()
assert (3, 4) in nested_view.values()
assert [1, 3] not in nested_view.values()
assert 1.0 in nested_view.values()
assert True in nested_view.values()
assert None not in nested_view.values()
# Duplicate values are found once each.
dupe_view = {'a': 1, 'b': 1}
assert 1 in dupe_view.values()
assert 2 not in dupe_view.values()
# The items view looks the key up, then compares the stored value.
assert ('a', [1, 2]) in nested_view.items()
assert ('a', [1, 3]) not in nested_view.items()
assert ('z', [1, 2]) not in nested_view.items()
assert ('c', True) in nested_view.items()
assert ('c', 1.0) in nested_view.items()
# Probes that are not a 2-item pair can never be members.
assert ('a', [1, 2], 'extra') not in nested_view.items()
assert ('a',) not in nested_view.items()
assert ['a', [1, 2]] not in nested_view.items()
# Keys view membership rejects an unhashable probe, as dict lookup does.
try:
    [1] in nested_view.keys()
    assert False, 'expected TypeError for an unhashable keys probe'
except TypeError as exc:
    assert str(exc) == "cannot use 'list' as a dict key (unhashable type: 'list')"
