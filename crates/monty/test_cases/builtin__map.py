import itertools

assert list(map(abs, [-1, 0, 1, -2])) == [1, 0, 1, 2]
assert list(map(abs, [0, 0, 0])) == [0, 0, 0]

assert list(map(str, [1, 2, 3])) == ['1', '2', '3']
assert list(map(str, [True, False])) == ['True', 'False']

assert list(map(int, ['1', '2', '3'])) == [1, 2, 3]
assert list(map(int, [1.1, 2.9, 3.5])) == [1, 2, 3]
assert list(map(int, [True, False, True])) == [1, 0, 1]

assert list(map(bool, [0, 1, '', 'x'])) == [False, True, False, True]
assert list(map(bool, [[], [1], (), (2,)])) == [False, True, False, True]

assert list(map(len, ['', 'a', 'ab', 'abc'])) == [0, 1, 2, 3]
assert list(map(len, [[], [1], [1, 2], [1, 2, 3]])) == [0, 1, 2, 3]

assert list(map(float, [1, 2, 3])) == [1.0, 2.0, 3.0]
assert list(map(float, ['1.5', '2.5'])) == [1.5, 2.5]

assert list(map(abs, [1, -2, 3])) == [1, 2, 3]

assert list(map(abs, (1, -2, 3))) == [1, 2, 3]

assert list(map(ord, 'abc')) == [97, 98, 99]

assert list(map(abs, range(-3, 3))) == [3, 2, 1, 0, 1, 2]

result = list(map(abs, {-1, 0, 1}))
assert sorted(result) == [0, 1, 1]

assert list(map(abs, [])) == []
assert list(map(abs, ())) == []
assert list(map(abs, '')) == []
assert list(map(abs, range(0))) == []

assert list(map(list, [(1, 2), (3, 4)])) == [[1, 2], [3, 4]]
assert list(map(tuple, [[1, 2], [3, 4]])) == [(1, 2), (3, 4)]

assert list(map(pow, [2, 3, 4], [3, 2, 2])) == [8, 9, 16]

assert list(map(divmod, [10, 20, 30], [3, 6, 7])) == [(3, 1), (3, 2), (4, 2)]

assert list(map(pow, [2, 3, 4, 5], [3, 2])) == [8, 9]
assert list(map(pow, [2, 3], [3, 2, 1, 0])) == [8, 9]

assert list(map(pow, [2], [3, 4, 5])) == [8]


def f(x):
    return x * 2


assert list(map(f, [1, 2, 3])) == [2, 4, 6]


def raise_exception(x):
    raise ValueError('Intentional error')


try:
    list(map(raise_exception, [1, 2, 3]))
    assert False, 'should have failed with exception'
except ValueError as e:
    assert str(e) == 'Intentional error'

try:
    map()
    assert False, 'map() should require arguments'
except TypeError as e:
    assert str(e) == 'map() must have at least two arguments.', f'map() arity: {e}'

try:
    map(None)
    assert False, 'map() with single arg should fail'
except TypeError as e:
    assert str(e) == 'map() must have at least two arguments.', f'map(fn) arity: {e}'

# === Lazy builtin type and construction order ===
events = []


class Source:
    def __init__(self, name, items):
        self.name = name
        self.items = iter(items)

    def __iter__(self):
        events.append(('iter', self.name))
        return self

    def __next__(self):
        events.append(('next', self.name))
        return next(self.items)


def combine(a, b):
    events.append(('call', a, b))
    return a + b


mapped = map(combine, Source('a', [1, 2]), Source('b', [10, 20]), strict=[True])
assert events == [('iter', 'a'), ('iter', 'b')]
assert type(mapped) is map
assert isinstance(mapped, map)
assert iter(mapped) is mapped
assert map.__name__ == 'map'
assert {mapped: 'value'}[mapped] == 'value'
assert mapped != map(combine, [], [])
assert next(mapped) == 11
assert events == [('iter', 'a'), ('iter', 'b'), ('next', 'a'), ('next', 'b'), ('call', 1, 10)]
assert [item for item in mapped] == [22]
assert next(mapped, None) is None

# === Strict lengths and left-to-right consumption ===
assert list(map(pow, [2, 3], [3, 2], strict=True)) == [8, 9]
assert list(map(abs, [], strict=True)) == []
for sources, message in [
    (([1], []), 'map() argument 2 is shorter than argument 1'),
    (([], [1]), 'map() argument 2 is longer than argument 1'),
    (([1], [2], []), 'map() argument 3 is shorter than arguments 1-2'),
    (([], [], [1]), 'map() argument 3 is longer than arguments 1-2'),
]:
    try:
        list(map(lambda *args: args, *sources, strict=True))
        assert False, 'expected unequal lengths to fail'
    except ValueError as exc:
        assert str(exc) == message

left = iter([1, 2, 3])
right = iter([4])
assert list(map(pow, left, right)) == [1]
assert next(left) == 3


# === Callback errors do not permanently exhaust the map ===
def sometimes_stops(item):
    if item == 1:
        raise StopIteration('callback stopped')
    if item == 3:
        raise ValueError('callback failed')
    return item


mapped = map(sometimes_stops, [1, 2, 3, 4], strict=True)
assert next(mapped, 'default') == 'default'
assert next(mapped) == 2
try:
    next(mapped)
    assert False, 'expected callback failure'
except ValueError as exc:
    assert str(exc) == 'callback failed'
assert next(mapped) == 4
assert next(mapped, None) is None


# === Re-entrant callbacks share source state ===
def reenter(item):
    if item == 1:
        return (item, next(reentrant))
    return item


reentrant = map(reenter, [1, 2, 3])
assert next(reentrant) == (1, 2)
assert next(reentrant) == 3
assert next(reentrant, None) is None

# === Non-callables are rejected only when a complete argument row exists ===
assert list(map(None, [])) == []
invalid = map(None, [1])
try:
    next(invalid)
    assert False, 'expected non-callable map callback to fail'
except TypeError as exc:
    assert str(exc) == "'NoneType' object is not callable"

# === Infinite sources do not run at construction ===
infinite = map(abs, itertools.count(-2))
assert [next(infinite), next(infinite), next(infinite)] == [2, 1, 0]

# === Keyword validation precedes positional arity ===
try:
    map(bad=True)
    assert False, 'expected unknown map keyword to fail'
except TypeError as exc:
    assert str(exc) == "map() got an unexpected keyword argument 'bad'"
for args in [(), (abs,)]:
    try:
        map(*args, strict=True)
        assert False, 'expected missing map source to fail'
    except TypeError as exc:
        assert str(exc) == 'map() must have at least two arguments.'
try:
    map(abs, [], **{'strict': True, 'other': 1})
    assert False, 'expected extra keyword to fail'
except TypeError as exc:
    assert str(exc) == 'map() takes at most 1 keyword argument (2 given)'
