from collections import deque

for count in [-3, 0, 1, 2, 5]:
    item = [1]
    a = [item, 2]
    alias = a
    expected = a * count
    a *= count
    assert a is alias and a == expected
    if count > 0:
        assert a[0] is item
    for maxlen in [None, 0, 1, 3, 7]:
        d = deque([item, 2], maxlen)
        alias = d
        expected = d * count
        d *= count
        assert d is alias and d == expected and d.maxlen == maxlen

for iterable in [(2, 3), range(2, 4), 'ab', {'a': 1}, {2, 3}]:
    a = [1]
    alias = a
    expected = [1] + list(iterable)
    a += iterable
    assert a is alias and a == expected

a = [1, 2]
alias = a
a += a
assert a is alias and a == [1, 2, 1, 2]


class Failing:
    def __init__(self):
        self.step = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.step += 1
        if self.step == 1:
            return [2]
        raise ValueError('stop')


a = [1]
alias = a
try:
    a += Failing()
except ValueError as exc:
    assert str(exc) == 'stop'
else:
    assert False
assert a is alias and a == [1, [2]]


class Count:
    def __index__(self):
        target.append(3)
        return 2


for target in [[1, 2], deque([1, 2])]:
    alias = target
    target *= Count()
    assert target is alias and list(target) == [1, 2, 3, 1, 2, 3]


class Reflected:
    def __radd__(self, other):
        return 42

    def __rmul__(self, other):
        return 43

    def __index__(self):
        raise AssertionError('reflected arithmetic must run first')


for make in [list, deque]:
    a = make([1])
    alias = a
    a += Reflected()
    assert a == 42 and list(alias) == [1]
    a = make([1])
    alias = a
    a *= Reflected()
    assert a == 43 and list(alias) == [1]


class Declining:
    def __rmul__(self, other):
        calls.append('rmul')
        return NotImplemented


for make in [list, deque]:
    calls = []
    a = make([1])
    try:
        a *= Declining()
    except TypeError:
        pass
    else:
        assert False
    assert calls == ['rmul']

for count in [0, 1, 2]:
    d = deque([1, 2])
    it = iter(d)
    d *= count
    if count == 1:
        assert next(it) == 1
    else:
        try:
            next(it)
        except RuntimeError:
            pass
        else:
            assert False

for source in [{2, 3}, frozenset({2, 3})]:
    for operation in ['or', 'and', 'sub', 'xor']:
        s = {1, 2}
        alias = s
        if operation == 'or':
            s |= source
            expected = {1, 2, 3}
        elif operation == 'and':
            s &= source
            expected = {2}
        elif operation == 'sub':
            s -= source
            expected = {1}
        else:
            s ^= source
            expected = {1, 3}
        assert s is alias and s == expected

for operation in ['or', 'and', 'sub', 'xor']:
    s = {1, 2}
    alias = s
    if operation == 'or':
        s |= s
    elif operation == 'and':
        s &= s
    elif operation == 'sub':
        s -= s
    else:
        s ^= s
    assert s is alias
    assert s == ({1, 2} if operation in ['or', 'and'] else set())

for operation in ['or', 'and', 'sub', 'xor']:
    s = {1, 2}
    alias = s
    view = {2: None, 3: None}.keys()
    if operation == 'or':
        s |= view
        expected = {1, 2, 3}
    elif operation == 'and':
        s &= view
        expected = {2}
    elif operation == 'sub':
        s -= view
        expected = {1}
    else:
        s ^= view
        expected = {1, 3}
    assert s is not alias and alias == {1, 2} and s == expected


for make in [list, deque]:
    a = make()
    a.append(a)
    alias = a
    a *= 3
    assert a is alias and len(a) == 3 and all(item is a for item in a)
    a *= 0
    assert a is alias and len(a) == 0

bounded = deque([1, 2], 3)
alias = bounded
bounded *= 10**12
assert bounded is alias and list(bounded) == [2, 1, 2]


class Key:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        hashes.append(self.n)
        return 0

    def __eq__(self, other):
        return self.n == other.n


for operation in ['or', 'and', 'sub', 'xor']:
    hashes = []
    a, b, c = Key(1), Key(2), Key(3)
    s = {a, b}
    t = {b, c}
    hashes.clear()
    if operation == 'or':
        s |= t
        expected = [1, 2, 3]
    elif operation == 'and':
        s &= t
        expected = [2]
    elif operation == 'sub':
        s -= t
        expected = [1]
    else:
        s ^= t
        expected = [1, 3]
    assert hashes == []
    assert sorted(item.n for item in s) == expected
