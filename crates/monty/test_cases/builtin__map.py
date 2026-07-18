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
