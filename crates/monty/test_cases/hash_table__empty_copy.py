import copy
from collections import Counter, defaultdict


for kind, original in [
    ('dict', {i: [i] for i in range(128)}),
    ('counter', Counter({i: i + 1 for i in range(128)})),
    ('defaultdict', defaultdict(list, {i: [i] for i in range(128)})),
]:
    for key in list(original):
        original.pop(key)
    copied = original.copy()
    assert type(copied) is type(original)
    assert copied is not original
    assert copied == {}

    shallow = copy.copy(original)
    assert shallow is not original
    assert type(shallow) is type(original)
    assert shallow == {}
    merged = original | {}
    assert merged == {}
    assert type(merged) is (defaultdict if kind == 'defaultdict' else dict)

    if kind == 'defaultdict':
        assert copied['missing'] == []
        assert shallow['missing'] == []
        assert merged['missing'] == []
        assert 'missing' not in original
    elif kind == 'counter':
        assert copied['missing'] == 0
        assert shallow['missing'] == 0
        assert 'missing' not in copied
    copied['new'] = [1]
    assert 'new' not in original
    original['source'] = [2]
    assert 'source' not in copied


def make_factory():
    captured = []

    def factory():
        return captured

    return factory


# The copy must own the heap-backed factory after the original is released.
source = defaultdict(make_factory(), {'old': [1]})
source.pop('old')
copied = source.copy()
source = None
copied['missing'].append('kept')
assert copied['another'] is copied['missing']
assert copied['another'] == ['kept']

counter = Counter({'zero': 0, 'negative': -2})
counter_copy = counter.copy()
assert list(counter_copy.items()) == [('zero', 0), ('negative', -2)]
assert counter_copy['missing'] == 0

values = set(range(128))
for value in range(128):
    values.remove(value)

updated = {'keep'}
updated.update(values)
assert updated == {'keep'}
assert updated.isdisjoint(values)
assert values.issubset(updated)

copied = values.copy()
assert copied is not values
assert copied == set()
copied.add('new')
assert not values
values.add('source')
assert 'source' not in copied

# A copy must keep heap references shared, including after earlier deletions.
shared = [1]
mapping = {i: [i] for i in range(128)}
for i in range(127):
    mapping.pop(i)
mapping['shared'] = shared
copied_mapping = mapping.copy()
assert list(copied_mapping) == [127, 'shared']
assert copied_mapping['shared'] is shared
shared.append(2)
assert copied_mapping['shared'] == [1, 2]

# Copying cannot change the source iterator's position or mutation detection.
mapping = {'old': 1}
iterator = iter(mapping)
assert next(iterator) == 'old'
mapping.pop('old')
assert mapping.copy() == {}
mapping['new'] = 2
try:
    next(iterator)
except RuntimeError as error:
    assert str(error) == 'dictionary keys changed during iteration'
else:
    assert False
