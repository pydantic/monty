# call-external
from collections import Counter, defaultdict
from copy import copy, deepcopy
import json


# Exercise both sides of the compaction threshold and repeated shrink/grow cycles.
for n in [1, 7, 63, 64, 65, 129, 256]:
    d = {i: [i] for i in range(n)}
    s = set(range(n))
    for i in range(0, n, 2):
        assert d.pop(i) == [i]
        s.remove(i)
    expected = list(range(1, n, 2))
    assert list(d) == expected
    assert list(d.values()) == [[i] for i in expected]
    assert list(d.items()) == [(i, [i]) for i in expected]
    assert sorted(s) == expected
    assert copy(d) == d and deepcopy(d) == d
    assert copy(s) == s and deepcopy(s) == s
    assert s == frozenset(s) == s.intersection(set(range(n)))
    assert s.union(set(range(0, n, 2))) == set(range(n))
    assert s.difference(s) == set()
    for i in range(n, n + 70):
        d[i] = [i]
        s.add(i)
    expected += list(range(n, n + 70))
    assert list(d) == expected
    assert sorted(s) == expected
    for i in expected[::-1]:
        assert d.popitem() == (i, [i])
    popped = []
    while s:
        popped.append(s.pop())
    assert sorted(popped) == expected
    assert not d and not s


# Deletions before iterator creation must survive suspension with live iterators.
d = {i: i * 2 for i in range(130)}
s = set(range(130))
for i in range(0, 90, 3):
    d.pop(i)
    s.discard(i)
keys, values, items, members = iter(d), iter(d.values()), iter(d.items()), iter(s)
assert next(keys) == 1
assert next(values) == 2
assert next(items) == (1, 2)
first_member = next(members)
assert add_ints(2, 3) == 5
expected = [i for i in range(130) if i >= 90 or i % 3 != 0]
assert list(keys) == expected[1:]
assert list(values) == [i * 2 for i in expected[1:]]
assert list(items) == [(i, i * 2) for i in expected[1:]]
assert sorted([first_member] + list(members)) == expected


# A same-size mutation can trigger compaction between iterator steps.
d = {i: i for i in range(128)}
for i in range(63):
    d.pop(i)
it = iter(d)
assert next(it) == 63
d.pop(127)
d[128] = 128
assert add_ints(3, 4) == 7
assert list(it) == list(range(64, 127)) + [128]


# Cached hashes, collisions, and reinsertions must preserve equality and order.
class Key:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return 1

    def __eq__(self, other):
        return self.value == other.value


keys = [Key(i) for i in range(80)]
d = {key: key.value for key in keys}
s = set(keys)
for i in range(60):
    assert d.pop(Key(i)) == i
    s.remove(Key(i))
for i in range(20):
    d[Key(i)] = i
    s.add(Key(i))
assert [key.value for key in d] == list(range(60, 80)) + list(range(20))
assert sorted(key.value for key in s) == list(range(20)) + list(range(60, 80))
assert all(d[Key(i)] == i for i in range(20))
assert Key(30) not in s and Key(70) in s


# Subclasses, consumers, cycles, and owned values must ignore vacant slots.
counts = Counter({i: i % 3 for i in range(128)})
for i in range(0, 128, 5):
    counts.pop(i)
positive = +counts
assert list(positive) == [i for i in range(128) if i % 5 and i % 3]
assert positive.most_common() == sorted(positive.items(), key=lambda pair: -pair[1])
assert list(positive.elements()) == [i for i in positive for _ in range(positive[i])]
d = defaultdict(list, {str(i): [i] for i in range(80)})
for i in range(30):
    d.pop(str(i))
assert json.loads(json.dumps(d)) == d
assert deepcopy(d).default_factory is list
d['cycle'] = d
cloned = deepcopy(d)
assert cloned['cycle'] is cloned
d.pop('cycle')
cloned.pop('cycle')
