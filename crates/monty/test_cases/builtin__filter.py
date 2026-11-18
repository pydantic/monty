import itertools

assert list(filter(None, [0, 1, False, True, '', 'hello'])) == [1, True, 'hello']
assert list(filter(None, [])) == []
assert list(filter(None, [0, 0, 0])) == []
assert list(filter(None, [1, 2, 3])) == [1, 2, 3]
assert list(filter(None, ['', '', 'x'])) == ['x']

assert list(filter(abs, [-1, 0, 1])) == [-1, 1]
assert list(filter(abs, [0, 0, 0])) == []
assert list(filter(abs, [-5, -3, 0, 2, 0, 4])) == [-5, -3, 2, 4]

assert list(filter(bool, [0, 1, '', 'x'])) == [1, 'x']
assert list(filter(bool, [False, True, 0, 1])) == [True, 1]
assert list(filter(bool, [[], [1], (), (2,)])) == [[1], (2,)]

# Note: len returns int, so empty containers return 0 (falsy), non-empty return truthy
assert list(filter(len, ['', 'a', '', 'bc'])) == ['a', 'bc']
assert list(filter(len, [[], [1], [], [2, 3]])) == [[1], [2, 3]]
assert list(filter(len, [(), (1,), (), (2, 3)])) == [(1,), (2, 3)]

assert list(filter(int, ['0', '1', '2', '0'])) == ['1', '2']
assert list(filter(int, [0.0, 1.5, 0.0, 2.3])) == [1.5, 2.3]

assert list(filter(str, [0, 1, '', 'x'])) == [0, 1, 'x']

assert list(filter(None, [1, 2, 3])) == [1, 2, 3]

assert list(filter(None, (0, 1, 2))) == [1, 2]

assert list(filter(None, 'abc')) == ['a', 'b', 'c']
assert list(filter(None, 'a b')) == ['a', ' ', 'b']

assert list(filter(None, range(0, 5))) == [1, 2, 3, 4]
assert list(filter(None, range(1, 4))) == [1, 2, 3]

assert list(filter(None, {0, 1, 2})) == [1, 2] or list(filter(None, {0, 1, 2})) == [2, 1], 'filter set'

assert list(filter(None, [])) == []
assert list(filter(None, ())) == []
assert list(filter(None, '')) == []
assert list(filter(None, range(0))) == []

assert list(filter(None, [[], [1], []])) == [[1]]
assert list(filter(None, [(), (1,), ()])) == [(1,)]


# === User-defined predicates ===
def is_positive(x):
    return x > 0


assert list(filter(is_positive, [-1, 1])) == [1]


assert list(filter(lambda x: x > 0, [-1, 1])) == [1]


try:
    list(filter(4, [1, 2]))
    assert False, 'filter with non-callable first argument should raise TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not callable"

# === Lazy builtin type and one-time acquisition ===
events = []


class Source:
    def __init__(self):
        self.items = iter([0, 1, 2, 3])

    def __iter__(self):
        events.append('iter')
        return self

    def __next__(self):
        events.append('next')
        return next(self.items)


def odd(item):
    events.append(item)
    return item % 2


filtered = filter(odd, Source())
assert events == ['iter']
assert type(filtered) is filter
assert isinstance(filtered, filter)
assert iter(filtered) is filtered
assert filter.__name__ == 'filter'
assert {filtered: 'value'}[filtered] == 'value'
assert filtered != filter(None, [])
assert next(filtered) == 1
assert events == ['iter', 'next', 0, 'next', 1]
assert [item for item in filtered] == [3]
assert next(filtered, None) is None


# === Predicate failures do not permanently exhaust the filter ===
def sometimes_stops(item):
    if item == 1:
        raise StopIteration('predicate stopped')
    if item == 3:
        raise ValueError('predicate failed')
    return True


filtered = filter(sometimes_stops, [1, 2, 3, 4])
assert next(filtered, 'default') == 'default'
assert next(filtered) == 2
try:
    next(filtered)
    assert False, 'expected predicate failure'
except ValueError as exc:
    assert str(exc) == 'predicate failed'
assert next(filtered) == 4
assert next(filtered, None) is None


# === Re-entrant predicates share source state ===
def reenter(item):
    if item == 1:
        assert next(reentrant) == 2
    return True


reentrant = filter(reenter, [1, 2, 3])
assert next(reentrant) == 1
assert next(reentrant) == 3
assert next(reentrant, None) is None

# === Synchronous consumption preserves re-entry and exceptional advancement ===
reentrant = filter(reenter, [1, 2, 3])
assert list(reentrant) == [1, 3]
filtered = filter(sometimes_stops, [1, 2, 3, 4])
assert list(filtered) == []
assert next(filtered) == 2
try:
    list(filtered)
    assert False, 'expected predicate failure'
except ValueError as exc:
    assert str(exc) == 'predicate failed'
assert next(filtered) == 4


for consume in (next, list):
    for predicate, items in [
        (None, [NotImplemented, ['kept']]),
        (lambda item: NotImplemented if item[0] == 'bad' else True, [['bad'], ['kept']]),
    ]:
        filtered = filter(predicate, items)
        try:
            consume(filtered)
            assert False, 'expected truth test failure'
        except TypeError as exc:
            assert str(exc) == 'NotImplemented should not be used in a boolean context'
        assert next(filtered) == ['kept']

# === Non-callables are rejected only when an item is available ===
assert list(filter(4, [])) == []

# === Nested sources do not recurse on the native stack ===
nested = [1]
for _ in range(1000):
    nested = filter(None, nested)
assert list(nested) == [1]

# === Infinite sources do not run at construction ===
infinite = filter(lambda x: x % 3 == 0, itertools.count(1))
assert [next(infinite), next(infinite), next(infinite)] == [3, 6, 9]
