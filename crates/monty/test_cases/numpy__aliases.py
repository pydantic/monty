# skip-cpython
import numpy as np


# === np.absolute -> np.abs ===
assert np.absolute(-3) == 3, 'absolute scalar int'
assert np.absolute(-2.5) == 2.5, 'absolute scalar float'
assert np.absolute([-1.5, 0.0, 2.5]).tolist() == [1.5, 0.0, 2.5], 'absolute float list'
assert np.absolute(np.array([-3, 0, 5])).tolist() == [3, 0, 5], 'absolute 1d array'
assert np.absolute(np.array([[-1, 2], [-3, 4]])).tolist() == [[1, 2], [3, 4]], 'absolute 2d array'
assert np.absolute(np.array([])).tolist() == [], 'absolute empty array'


# === np.amax / np.amin -> np.max / np.min ===
assert np.amax(5) == 5, 'amax scalar'
assert np.amin(-2) == -2, 'amin scalar'
assert np.amax([1.0, 5.0, 3.0]) == 5.0, 'amax float list'
assert np.amin([1.0, -5.0, 3.0]) == -5.0, 'amin float list'
assert np.amax(np.array([1, 5, 3])) == 5, 'amax 1d array'
assert np.amin(np.array([1, -5, 3])) == -5, 'amin 1d array'
assert np.amax(np.array([[1, 4], [2, 3]])) == 4, 'amax 2d array'
assert np.amin(np.array([[1, 4], [2, 3]])) == 1, 'amin 2d array'


# === np.asin / np.acos / np.atan aliases ===
assert np.asin(0.0) == 0.0, 'asin scalar zero'
assert abs(np.asin(1.0) - np.pi / 2) < 1e-12, 'asin scalar one'
assert np.acos(1.0) == 0.0, 'acos scalar one'
assert abs(np.acos(0.0) - np.pi / 2) < 1e-12, 'acos scalar zero'
assert np.atan(0.0) == 0.0, 'atan scalar zero'
assert abs(np.atan(1.0) - np.pi / 4) < 1e-12, 'atan scalar one'
assert np.asin([0.0, 1.0]).tolist()[0] == 0.0, 'asin list first'
assert abs(np.asin([0.0, 1.0]).tolist()[1] - np.pi / 2) < 1e-12, 'asin list second'
acos_result = np.acos(np.array([[1.0, 0.0], [0.0, 1.0]])).tolist()
assert acos_result[0][0] == 0.0, 'acos 2d first'
assert abs(acos_result[0][1] - np.pi / 2) < 1e-12, 'acos 2d second'
atan_result = np.atan(np.array([0.0, 1.0])).tolist()
assert atan_result[0] == 0.0, 'atan 1d first'
assert abs(atan_result[1] - np.pi / 4) < 1e-12, 'atan 1d second'
assert np.atan(np.array([])).tolist() == [], 'atan empty array'


# === additional inverse aliases ===
assert np.asinh(0.0) == 0.0, 'asinh scalar zero'
assert abs(np.asinh(1.0) - 0.881373587019543) < 1e-12, 'asinh scalar one'
assert np.acosh(1.0) == 0.0, 'acosh scalar one'
assert abs(np.acosh(2.0) - 1.3169578969248166) < 1e-12, 'acosh scalar two'
assert np.atanh(0.0) == 0.0, 'atanh scalar zero'
assert abs(np.atanh(0.5) - 0.5493061443340548) < 1e-12, 'atanh scalar half'
assert np.atan2(0.0, 1.0) == 0.0, 'atan2 scalar zero'
assert abs(np.atan2(np.array([1.0]), np.array([1.0])).tolist()[0] - np.pi / 4) < 1e-12, 'atan2 array'


# === np.around -> np.round ===
assert np.around(1.234, 2) == 1.23, 'around scalar'
assert np.around([1.234, 5.678], 1).tolist() == [1.2, 5.7], 'around list'
assert np.around(np.array([1.234, 5.678]), 1).tolist() == [1.2, 5.7], 'around 1d array'
assert np.around(np.array([[1.234, 5.678], [9.012, 3.456]]), 1).tolist() == [
    [1.2, 5.7],
    [9.0, 3.5],
], 'around 2d array'
assert np.around(np.array([]), 1).tolist() == [], 'around empty array'


# === np.asanyarray -> np.asarray ===
a = np.asanyarray([1, 2, 3])
assert a.tolist() == [1, 2, 3], 'asanyarray list'
assert a.shape == (3,), 'asanyarray list shape'
b = np.array([[1, 2], [3, 4]])
c = np.asanyarray(b)
assert c.tolist() == [[1, 2], [3, 4]], 'asanyarray ndarray values'
assert c.shape == (2, 2), 'asanyarray ndarray shape'
assert np.asanyarray([]).tolist() == [], 'asanyarray empty list'


# === common binary ufuncs ===
assert np.add(1, 2) == 3, 'add scalar'
assert np.add(1, 2.5) == 3.5, 'add mixed scalar promotes to float'
assert np.add([1, 2], [3, 4]).tolist() == [4, 6], 'add lists'
assert np.add(np.array([1, 2]), [3, 4]).tolist() == [4, 6], 'add array and list'
assert np.add(np.array([[1, 2], [3, 4]]), 10).tolist() == [[11, 12], [13, 14]], 'add scalar broadcast'
assert np.subtract(10, np.array([1, 2])).tolist() == [9, 8], 'subtract scalar left broadcast'
assert np.multiply(2, [3, 4]).tolist() == [6, 8], 'multiply scalar left list'
assert np.divide(9, np.array([3, 2])).tolist() == [3.0, 4.5], 'divide scalar left array'
assert np.add(np.array([]), np.array([])).tolist() == [], 'add empty arrays'
assert np.subtract(np.array([5, 7]), np.array([2, 3])).tolist() == [3, 4], 'subtract arrays'
assert np.multiply(np.array([2, 3]), np.array([4, 5])).tolist() == [8, 15], 'multiply arrays'
assert np.divide(np.array([5, 9]), np.array([2, 3])).tolist() == [2.5, 3.0], 'divide arrays'
assert np.true_divide(5, 2) == 2.5, 'true_divide scalar'
assert np.floor_divide(np.array([5, 9]), np.array([2, 3])).tolist() == [2, 3], 'floor_divide arrays'
assert np.floor_divide(9, np.array([2, 4])).tolist() == [4, 2], 'floor_divide scalar left array'
assert np.mod(np.array([-3, 4]), np.array([2, 3])).tolist() == [1, 1], 'mod arrays'
assert np.mod(3, np.array([-2, 2])).tolist() == [-1, 1], 'mod scalar left signed divisors'
assert np.remainder(-3, 2) == 1, 'remainder scalar'
assert np.pow([2, 3], [3, 2]).tolist() == [8, 9], 'pow alias lists'


# === comparison ufuncs ===
assert np.equal(1, 1) == True, 'equal scalar true'
assert np.equal(float('nan'), float('nan')) == False, 'equal scalar nan'
assert np.not_equal(1, 2) == True, 'not_equal scalar true'
assert np.not_equal(float('nan'), float('nan')) == True, 'not_equal scalar nan'
assert np.greater(3, 2) == True, 'greater scalar true'
assert np.greater_equal(2, 2) == True, 'greater_equal scalar true'
assert np.less(1, 2) == True, 'less scalar true'
assert np.less_equal(2, 2) == True, 'less_equal scalar true'
assert np.equal([1, 2, 3], [1, 0, 3]).tolist() == [True, False, True], 'equal lists'
assert np.not_equal(np.array([1, 2]), np.array([1, 3])).tolist() == [False, True], 'not_equal arrays'
assert np.greater(np.array([[1, 4], [5, 2]]), 3).tolist() == [[False, True], [True, False]], 'greater 2d'
assert np.greater_equal(3, np.array([2, 3, 4])).tolist() == [True, True, False], 'greater_equal scalar left'
assert np.less(3, np.array([2, 3, 4])).tolist() == [False, False, True], 'less scalar left'
assert np.less_equal(np.array([]), np.array([])).tolist() == [], 'less_equal empty arrays'


# === function aliases and shape helpers ===
assert np.concat([np.array([1, 2]), np.array([3, 4])]).tolist() == [1, 2, 3, 4], 'concat alias'
assert np.cumulative_sum(np.array([1, 2, 3])).tolist() == [1, 3, 6], 'cumulative_sum alias'
assert np.cumulative_prod(np.array([2, 3, 4])).tolist() == [2, 6, 24], 'cumulative_prod alias'
assert np.shape(5) == (), 'shape scalar'
assert np.shape([1, 2, 3]) == (3,), 'shape list'
assert np.shape(np.array([[1, 2], [3, 4]])) == (2, 2), 'shape 2d array'
assert np.size(5) == 1, 'size scalar'
assert np.size(np.array([[1, 2], [3, 4]])) == 4, 'size 2d array'
assert np.size(np.array([])) == 0, 'size empty array'
assert np.ndim(5) == 0, 'ndim scalar'
assert np.ndim([1, 2, 3]) == 1, 'ndim list'
assert np.ndim(np.array([[1, 2], [3, 4]])) == 2, 'ndim 2d array'
