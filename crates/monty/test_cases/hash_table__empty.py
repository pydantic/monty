mapping = {i: [i] for i in range(128)}
values = set(range(128))
for i in range(128):
    mapping.pop(i)
    values.remove(i)
assert list(mapping) == []
assert list(mapping.items()) == []
assert list(mapping.values()) == []
assert repr(mapping) == '{}'
assert mapping.copy() == {}
assert values.copy() == set()
assert dict(mapping) == {}
assert frozenset(values) == frozenset()

# Reusing the emptied tables must still retain new entries and their order.
mapping['new'] = [1]
values.add('new')
assert mapping.copy() == {'new': [1]}
assert values.copy() == {'new'}
assert next(iter(mapping)) == 'new'
assert next(iter(values)) == 'new'
