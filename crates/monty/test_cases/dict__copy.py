from collections import Counter, defaultdict


class Key:
    def __init__(self, number):
        self.number = number

    def __hash__(self):
        events.append(('hash', self.number))
        if forbidden:
            raise RuntimeError('copy must not hash keys')
        return 5

    def __eq__(self, other):
        events.append(('eq', self.number))
        if forbidden:
            raise RuntimeError('copy must not compare keys')
        return self is other


# Collisions and deleted slots must not trigger callbacks during a copy.
events = []
forbidden = False
keys = [Key(i) for i in range(8)]
values = [[i] for i in range(8)]
source = {key: value for key, value in zip(keys, values)}
source.pop(keys[1])
source.pop(keys[3])
events.clear()
forbidden = True
copied = source.copy()
assert copied is not source
assert events == []
assert len(copied) == 6
for left, right in zip(copied, source):
    assert left is right
for left, right in zip(copied.values(), source.values()):
    assert left is right
forbidden = False
copied[keys[0]].append(99)
assert source[keys[0]][-1] == 99
source.clear()
assert len(copied) == 6
assert copied[keys[0]][-1] == 99

# A sparse table is compacted without changing insertion order.
source = {str(i): [i] for i in range(100)}
for i in range(90):
    source.pop(str(i))
copied = source.copy()
assert list(copied) == list(source)
assert list(copied.values()) == list(source.values())
source.clear()
assert len(copied) == 10


def factory():
    return []


# Specialized dicts retain their factory or missing-count behavior.
source = defaultdict(factory)
source['a'].append(1)
copied = source.copy()
assert copied.default_factory is factory
assert copied['a'] is source['a']
assert copied['b'] == []
assert 'b' not in source
assert defaultdict(factory).copy().default_factory is factory
assert defaultdict(None).copy().default_factory is None
source = Counter({'a': 2, 'b': -3})
copied = source.copy()
assert copied['absent'] == 0
assert list(copied.items()) == list(source.items())
assert Counter().copy()['absent'] == 0

# A shallow copy keeps references to the source, including self-cycles.
source = {}
source['self'] = source
copied = source.copy()
assert copied['self'] is source
source.clear()
assert copied['self'] == {}
assert {}.copy() == {}


class ChangingKey:
    def __hash__(self):
        return 11

    def __eq__(self, other):
        return equal_now


# Keys that became equal after insertion must remain distinct entries.
equal_now = False
left, right = ChangingKey(), ChangingKey()
source = {left: 1, right: 2}
equal_now = True
copied = source.copy()
assert len(copied) == 2
assert list(copied)[0] is left
assert list(copied)[1] is right
assert list(copied.values()) == [1, 2]
