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
# CPython clones compact constructor inputs without comparing colliding keys.
constructor_source = {key: value for key, value in source.items()}
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
for constructed in [dict(constructor_source), Counter(constructor_source), defaultdict(None, constructor_source)]:
    assert constructed is not source
    assert len(constructed) == 6
    for left, right in zip(constructed, source):
        assert left is right
    for left, right in zip(constructed.values(), source.values()):
        assert left is right
empty_counter = Counter()
empty_counter.update(constructor_source)
assert list(empty_counter.values()) == list(source.values())
assert events == []
forbidden = False
# Sparse inputs can compare keys in CPython, but never hash them again.
for constructor in [dict, Counter]:
    events.clear()
    constructed = constructor(source)
    assert list(constructed.values()) == list(source.values())
    assert all(event[0] != 'hash' for event in events)
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
assert type(dict(source)) is dict
assert type(source.copy()) is defaultdict
assert defaultdict(factory).copy().default_factory is factory
assert defaultdict(None).copy().default_factory is None
source = Counter({'a': 2, 'b': -3})
copied = source.copy()
assert copied['absent'] == 0
assert list(copied.items()) == list(source.items())
assert type(dict(source)) is dict
assert type(source.copy()) is Counter
assert Counter().copy()['absent'] == 0

# Dict keyword arguments overwrite values; Counter keyword arguments add counts.
assert dict({'a': 2}, a=3) == {'a': 3}
assert Counter({'a': 2}, a=3) == {'a': 5}
assert Counter({'a': True})['a'] is True
assert dict([('a', 2)]) == {'a': 2}
assert Counter(['a', 'a']) == {'a': 2}
nonempty_counter = Counter(a=2)
nonempty_counter.update({'a': 3, 'b': 4})
assert nonempty_counter == {'a': 5, 'b': 4}
empty_counter = Counter()
empty_counter.update(empty_counter)
assert empty_counter == {}
nonempty_counter.update(nonempty_counter)
assert nonempty_counter == {'a': 10, 'b': 8}
empty_counter.subtract({'a': 2})
assert empty_counter == {'a': -2}

# A shallow copy keeps references to the source, including self-cycles.
source = {}
source['self'] = source
copied = source.copy()
assert copied['self'] is source
source.clear()
assert copied['self'] == {}
assert {}.copy() == {}
assert dict() == {}


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
for copied in [source.copy(), dict(source), Counter(source)]:
    assert len(copied) == 2
    assert list(copied)[0] is left
    assert list(copied)[1] is right
    assert list(copied.values()) == [1, 2]

# Drained mapping sources must still apply constructor keywords and choose the result kind.
shared = []
for source in [
    {i: [i] for i in range(128)},
    Counter({i: i for i in range(128)}),
    defaultdict(factory, {i: [i] for i in range(128)}),
]:
    for key in list(source):
        source.pop(key)

    plain = dict(source, extra=shared)
    assert type(plain) is dict
    assert plain == {'extra': []}
    assert plain['extra'] is shared

    counted = Counter(source, zero=0, negative=-2)
    assert type(counted) is Counter
    assert list(counted.items()) == [('zero', 0), ('negative', -2)]
    assert counted['missing'] == 0

    defaulted = defaultdict(factory, source, extra=shared)
    assert type(defaulted) is defaultdict
    assert defaulted.default_factory is factory
    assert defaulted['extra'] is shared
    assert defaulted['missing'] == []
    assert 'missing' not in source

    counted.update(source)
    assert list(counted.items()) == [('zero', 0), ('negative', -2)]
    empty_counted = Counter()
    empty_counted.update(source, extra=2)
    assert empty_counted == {'extra': 2}
    assert source == {}
