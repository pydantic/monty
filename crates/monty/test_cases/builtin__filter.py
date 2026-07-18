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


# filter() with user-defined function
# This should error until user-defined functions are supported
def is_positive(x):
    return x > 0


assert list(filter(is_positive, [-1, 1])) == [1]


assert list(filter(lambda x: x > 0, [-1, 1])) == [1]


try:
    list(filter(4, [1, 2]))
    assert False, 'filter with non-callable first argument should raise TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not callable"
