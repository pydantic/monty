assert list(map(abs, [-1, 0, 1, -2])) == [1, 0, 1, 2], 'map with abs'
assert list(map(abs, [0, 0, 0])) == [0, 0, 0], 'map with abs all zeros'

assert list(map(str, [1, 2, 3])) == ['1', '2', '3'], 'map with str on ints'
assert list(map(str, [True, False])) == ['True', 'False'], 'map with str on bools'

assert list(map(int, ['1', '2', '3'])) == [1, 2, 3], 'map with int on strings'
assert list(map(int, [1.1, 2.9, 3.5])) == [1, 2, 3], 'map with int on floats'
assert list(map(int, [True, False, True])) == [1, 0, 1], 'map with int on bools'

assert list(map(bool, [0, 1, '', 'x'])) == [False, True, False, True], 'map with bool'
assert list(map(bool, [[], [1], (), (2,)])) == [False, True, False, True], 'map with bool on containers'

assert list(map(len, ['', 'a', 'ab', 'abc'])) == [0, 1, 2, 3], 'map with len on strings'
assert list(map(len, [[], [1], [1, 2], [1, 2, 3]])) == [0, 1, 2, 3], 'map with len on lists'

assert list(map(float, [1, 2, 3])) == [1.0, 2.0, 3.0], 'map with float on ints'
assert list(map(float, ['1.5', '2.5'])) == [1.5, 2.5], 'map with float on strings'

assert list(map(abs, [1, -2, 3])) == [1, 2, 3], 'map on list'

assert list(map(abs, (1, -2, 3))) == [1, 2, 3], 'map on tuple'

assert list(map(ord, 'abc')) == [97, 98, 99], 'map ord on string'

assert list(map(abs, range(-3, 3))) == [3, 2, 1, 0, 1, 2], 'map on range'

result = list(map(abs, {-1, 0, 1}))
assert sorted(result) == [0, 1, 1], 'map on set'

assert list(map(abs, [])) == [], 'map on empty list'
assert list(map(abs, ())) == [], 'map on empty tuple'
assert list(map(abs, '')) == [], 'map on empty string'
assert list(map(abs, range(0))) == [], 'map on empty range'

assert list(map(list, [(1, 2), (3, 4)])) == [[1, 2], [3, 4]], 'map with list constructor'
assert list(map(tuple, [[1, 2], [3, 4]])) == [(1, 2), (3, 4)], 'map with tuple constructor'

assert list(map(pow, [2, 3, 4], [3, 2, 2])) == [8, 9, 16], 'map with pow and 2 iterables'

assert list(map(divmod, [10, 20, 30], [3, 6, 7])) == [(3, 1), (3, 2), (4, 2)], 'map with divmod and 2 iterables'

assert list(map(pow, [2, 3, 4, 5], [3, 2])) == [8, 9], 'map stops at shortest iterable'
assert list(map(pow, [2, 3], [3, 2, 1, 0])) == [8, 9], 'map stops at shortest iterable (first shorter)'

assert list(map(pow, [2], [3, 4, 5])) == [8], 'map with single item in shortest'

map()
# Raise=TypeError('map() must have at least two arguments.')

map(None)
# Raise=TypeError('map() must have at least two arguments.')
