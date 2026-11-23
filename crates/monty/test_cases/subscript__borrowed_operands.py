from collections import Counter, defaultdict


def lookup(container, key):
    result = container[key]
    return result


def literal(container):
    return container['field']


for container in [[11, 22], (11, 22)]:
    assert lookup(container, -1) == 22
    assert lookup(container, True) == 22
    assert lookup(container, False) == 11
    assert lookup(container, slice(None)) == container

for stored, key in [(True, 1), (False, 0), (1.0, 1), (1, 1.0), (-1.0, -1), (2**70, 2**70)]:
    assert lookup({stored: 'found'}, key) == 'found'

parts = ['f', 'ield']
for key in ['field', ''.join(parts), 'a heap string longer than fifteen bytes']:
    value = [key]
    assert lookup({key: value}, key) is value

assert literal(defaultdict(list)) == []
assert literal(Counter()) == 0


class ResizingIndex:
    def __index__(self):
        items.clear()
        items.append(99)
        return 0


items = [11]
assert lookup(items, ResizingIndex()) == 99


class Key:
    def __hash__(self):
        return hash('field')

    def __eq__(self, other):
        events.append(other)
        return other == 'field'


events = []
assert literal({Key(): 'custom'}) == 'custom'
assert events == ['field']


class RaisingKey:
    def __hash__(self):
        return 0

    def __eq__(self, other):
        raise ValueError('comparison')


try:
    lookup({RaisingKey(): 1}, 0)
    assert False, 'expected comparison failure'
except ValueError as exc:
    assert str(exc) == 'comparison'


def replace_container(container, key):
    container = container[key]
    return container


def replace_key(container, key):
    key = container[key]
    return key


value = [42]
assert replace_container([value], 0) is value
assert replace_key({'field': value}, 'field') is value
cycle = []
cycle.append(cycle)
assert replace_container(cycle, 0) is cycle


def failed_assignment(container, key):
    result = ['previous']
    try:
        result = container[key]
        assert False, 'expected subscription failure'
    except (IndexError, KeyError) as exc:
        assert str(exc) in ['list index out of range', "'missing'"]
    assert result == ['previous']


failed_assignment([], 0)
failed_assignment({}, 'missing')


def missing_operand(flag):
    if flag:
        container = [1]
    key = 0
    result = container[key]
    return result


try:
    missing_operand(False)
    assert False, 'expected unbound local'
except UnboundLocalError as exc:
    assert str(exc) == "cannot access local variable 'container' where it is not associated with a value"


def global_lookup():
    global result
    result = items[key]


items = [42]
key = 0
result = cycle
global_lookup()
assert result == 42
items = {'field': value}
key = 'field'
global_lookup()
assert result is value
