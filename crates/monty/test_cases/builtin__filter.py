assert list(filter(None, [0, 1, False, True, '', 'hello'])) == [1, True, 'hello'], 'filter None removes falsy values'
assert list(filter(None, [])) == [], 'filter None on empty list'
assert list(filter(None, [0, 0, 0])) == [], 'filter None removes all zeros'
assert list(filter(None, [1, 2, 3])) == [1, 2, 3], 'filter None keeps truthy values'
assert list(filter(None, ['', '', 'x'])) == ['x'], 'filter None keeps non-empty string'

assert list(filter(abs, [-1, 0, 1])) == [-1, 1], 'filter with abs keeps non-zero'
assert list(filter(abs, [0, 0, 0])) == [], 'filter with abs removes zeros'
assert list(filter(abs, [-5, -3, 0, 2, 0, 4])) == [-5, -3, 2, 4], 'filter with abs mixed'

assert list(filter(bool, [0, 1, '', 'x'])) == [1, 'x'], 'filter with bool'
assert list(filter(bool, [False, True, 0, 1])) == [True, 1], 'filter with bool booleans and ints'
assert list(filter(bool, [[], [1], (), (2,)])) == [[1], (2,)], 'filter with bool containers'

# Note: len returns int, so empty containers return 0 (falsy), non-empty return truthy
assert list(filter(len, ['', 'a', '', 'bc'])) == ['a', 'bc'], 'filter with len on strings'
assert list(filter(len, [[], [1], [], [2, 3]])) == [[1], [2, 3]], 'filter with len on lists'
assert list(filter(len, [(), (1,), (), (2, 3)])) == [(1,), (2, 3)], 'filter with len on tuples'

assert list(filter(int, ['0', '1', '2', '0'])) == ['1', '2'], 'filter with int on string numbers'
assert list(filter(int, [0.0, 1.5, 0.0, 2.3])) == [1.5, 2.3], 'filter with int on floats'

assert list(filter(str, [0, 1, '', 'x'])) == [0, 1, 'x'], 'filter with str converts and checks truthiness'

assert list(filter(None, [1, 2, 3])) == [1, 2, 3], 'filter list'

assert list(filter(None, (0, 1, 2))) == [1, 2], 'filter tuple'

assert list(filter(None, 'abc')) == ['a', 'b', 'c'], 'filter string'
assert list(filter(None, 'a b')) == ['a', ' ', 'b'], 'filter string with space'

assert list(filter(None, range(0, 5))) == [1, 2, 3, 4], 'filter range'
assert list(filter(None, range(1, 4))) == [1, 2, 3], 'filter range all truthy'

assert list(filter(None, {0, 1, 2})) == [1, 2] or list(filter(None, {0, 1, 2})) == [2, 1], 'filter set'

assert list(filter(None, [])) == [], 'filter empty list'
assert list(filter(None, ())) == [], 'filter empty tuple'
assert list(filter(None, '')) == [], 'filter empty string'
assert list(filter(None, range(0))) == [], 'filter empty range'

assert list(filter(None, [[], [1], []])) == [[1]], 'filter nested lists'
assert list(filter(None, [(), (1,), ()])) == [(1,)], 'filter nested tuples'


# filter() with user-defined function
# This should error until user-defined functions are supported
def is_positive(x):
    return x > 0


assert list(filter(is_positive, [-1, 1])) == [1], 'filter with user-defined function keeps positives'


assert list(filter(lambda x: x > 0, [-1, 1])) == [1], 'filter with lambda keeps positives'


try:
    list(filter(4, [1, 2]))
    assert False, 'filter with non-callable first argument should raise TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not callable", 'filter with non-callable first argument raises TypeError'
