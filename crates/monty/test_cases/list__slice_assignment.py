a = [0, 1, 2, 3]
alias = a
a[1:3] = range(5)
assert a == [0, 0, 1, 2, 3, 4, 3]
assert alias is a
a[:] = a
assert a == [0, 0, 1, 2, 3, 4, 3]
a[::-1] = a
assert a == [3, 4, 3, 2, 1, 0, 0]
a[2:1] = 'xy'
assert a == [3, 4, 'x', 'y', 3, 2, 1, 0, 0]
a[:] = []
assert a == []

for container in [list, tuple, iter]:
    for step in [1, 2, -1, -2, 10**30, -(10**30)]:
        a = [[0], [1], [2], [3], [4]]
        expected = a[:]
        indices = list(range(5))[::step]
        replacement = [[i + 10] for i in indices]
        a[::step] = container(replacement)
        for i, item in zip(indices, replacement):
            expected[i] = item
        assert a == expected

for step in [2, -1]:
    a = [[1], [2], [3]]
    try:
        a[::step] = [[9]]
        assert False, 'expected ValueError'
    except ValueError:
        pass
    assert a == [[1], [2], [3]]

try:
    a[::0] = []
    assert False, 'expected ValueError'
except ValueError:
    pass
try:
    a[:] = 42
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'must assign iterable to extended slice'


class Replacement:
    def __iter__(self):
        a[:] = [0, 1]
        return iter([9])


a = [0, 1, 2, 3]
a[1:] = Replacement()
assert a == [0, 9]


class Broken:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('broken')


a = [[1], [2]]
try:
    a[:] = Broken()
    assert False, 'expected ValueError'
except ValueError:
    pass
assert a == [[1], [2]]

for bounds in [(None, None), (-100, 100), (100, -100), (-3, -1), (3, 1)]:
    for step in [1, 2, -1, -2, 10**30, -(10**30)]:
        a = list(range(5))
        key = slice(bounds[0], bounds[1], step)
        indices = a[key]
        replacement = [i + 10 for i in indices]
        a[key] = replacement
        expected = list(range(5))
        for index, item in zip(indices, replacement):
            expected[index] = item
        assert a == expected

a = [[1], [2], [3]]
a[1:2] = []
assert a == [[1], [3]]
a[100:200] = ([4], [5])
assert a == [[1], [3], [4], [5]]
a[-100:-99] = [[0]]
assert a == [[0], [1], [3], [4], [5]]


class TypeErrorIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise TypeError('from next')


try:
    a[:] = TypeErrorIterator()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'from next'


class BrokenIterable:
    def __iter__(self):
        raise error


error = ValueError('from iter')
a = [[1], [2]]
try:
    a[:] = BrokenIterable()
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'from iter'
assert a == [[1], [2]]

error = TypeError('from iter')
try:
    a[:] = BrokenIterable()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'must assign iterable to extended slice'
assert a == [[1], [2]]
