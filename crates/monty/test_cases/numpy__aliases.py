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


# === shape and index helpers ===
assert np.atleast_1d(5).tolist() == [5], 'atleast_1d scalar'
assert np.atleast_2d([1, 2, 3]).tolist() == [[1, 2, 3]], 'atleast_2d list'
assert np.atleast_3d([1, 2, 3]).tolist() == [[[1], [2], [3]]], 'atleast_3d list'
atleast_a, atleast_b = np.atleast_1d(1, [2, 3])
assert atleast_a.tolist() == [1], 'atleast_1d multi scalar'
assert atleast_b.tolist() == [2, 3], 'atleast_1d multi list'

diag_row, diag_col = np.diag_indices(3)
assert diag_row.tolist() == [0, 1, 2], 'diag_indices first axis'
assert diag_col.tolist() == [0, 1, 2], 'diag_indices second axis'
diag_from_row, diag_from_col = np.diag_indices_from(np.ones((3, 3)))
assert diag_from_row.tolist() == [0, 1, 2], 'diag_indices_from first axis'
assert diag_from_col.tolist() == [0, 1, 2], 'diag_indices_from second axis'

tril_row, tril_col = np.tril_indices(3)
assert tril_row.tolist() == [0, 1, 1, 2, 2, 2], 'tril_indices rows'
assert tril_col.tolist() == [0, 0, 1, 0, 1, 2], 'tril_indices cols'
tril_from_row, tril_from_col = np.tril_indices_from(np.ones((2, 3)), 1)
assert tril_from_row.tolist() == [0, 0, 1, 1, 1], 'tril_indices_from rows'
assert tril_from_col.tolist() == [0, 1, 0, 1, 2], 'tril_indices_from cols'

triu_row, triu_col = np.triu_indices(3)
assert triu_row.tolist() == [0, 0, 0, 1, 1, 2], 'triu_indices rows'
assert triu_col.tolist() == [0, 1, 2, 1, 2, 2], 'triu_indices cols'
triu_from_row, triu_from_col = np.triu_indices_from(np.ones((2, 3)), -1)
assert triu_from_row.tolist() == [0, 0, 0, 1, 1, 1], 'triu_indices_from rows'
assert triu_from_col.tolist() == [0, 1, 2, 0, 1, 2], 'triu_indices_from cols'

grid = np.indices((2, 3))
assert grid.shape == (2, 2, 3), 'indices shape'
assert grid.tolist() == [[[0, 0, 0], [1, 1, 1]], [[0, 1, 2], [0, 1, 2]]], 'indices values'

unravel_row, unravel_col = np.unravel_index([5, 6], (3, 4))
assert unravel_row.tolist() == [1, 1], 'unravel_index rows'
assert unravel_col.tolist() == [1, 2], 'unravel_index cols'
scalar_row, scalar_col = np.unravel_index(5, (3, 4))
assert scalar_row == 1, 'unravel_index scalar row'
assert scalar_col == 1, 'unravel_index scalar col'
assert np.ravel_multi_index(([1, 2], [1, 3]), (3, 4)).tolist() == [5, 11], 'ravel_multi_index arrays'
assert np.ravel_multi_index((1, 1), (3, 4)) == 5, 'ravel_multi_index scalar'


# === module-level manipulation wrappers ===
matrix = np.array([[1, 2, 3], [4, 5, 6]])
assert np.take(matrix, [0, -1, 2]).tolist() == [1, 6, 3], 'take flattened indices'
assert np.take(matrix, 2) == 3, 'take flattened scalar index'
assert np.compress([1, 0, 1, 0, 0, 1], matrix).tolist() == [1, 3, 6], 'compress flattened condition'
assert np.swapaxes(matrix, 0, 1).tolist() == [[1, 4], [2, 5], [3, 6]], 'swapaxes 2d'
assert np.swapaxes(matrix, -1, -2).tolist() == [[1, 4], [2, 5], [3, 6]], 'swapaxes negative axes'
assert np.permute_dims(matrix).tolist() == [[1, 4], [2, 5], [3, 6]], 'permute_dims default'
assert np.permute_dims(matrix, (0, 1)).tolist() == [[1, 2, 3], [4, 5, 6]], 'permute_dims identity'
assert np.matrix_transpose(matrix).tolist() == [[1, 4], [2, 5], [3, 6]], 'matrix_transpose 2d'
try:
    np.matrix_transpose(np.array([1, 2, 3]))
    assert False, 'expected matrix_transpose to reject 1d input'
except ValueError as exc:
    assert str(exc) == 'Input array must be at least 2-dimensional, but it is 1', 'matrix_transpose 1d error'

assert np.rot90(matrix).tolist() == [[3, 6], [2, 5], [1, 4]], 'rot90 one turn'
assert np.rot90(matrix, 2).tolist() == [[6, 5, 4], [3, 2, 1]], 'rot90 two turns'
assert np.rot90(matrix, -1).tolist() == [[4, 1], [5, 2], [6, 3]], 'rot90 negative turn'

cube = np.arange(24).reshape(2, 3, 4)
moved = np.moveaxis(cube, 0, 2)
assert moved.shape == (3, 4, 2), 'moveaxis shape'
assert moved.tolist()[0][0] == [0, 12], 'moveaxis first vector'
assert moved.tolist()[2][3] == [11, 23], 'moveaxis last vector'
rolled = np.rollaxis(cube, 2, 1)
assert rolled.shape == (2, 4, 3), 'rollaxis shape'
assert rolled.tolist()[0][0] == [0, 4, 8], 'rollaxis first vector'
assert rolled.tolist()[1][3] == [15, 19, 23], 'rollaxis last vector'


# === linear algebra and numeric wrappers ===
assert np.vecdot(np.array([1, 2, 3]), np.array([4, 5, 6])) == 32, 'vecdot 1d'
assert np.matvec(np.array([[1, 2], [3, 4]]), np.array([10, 20])).tolist() == [50, 110], 'matvec 2d 1d'
assert np.vecmat(np.array([10, 20]), np.array([[1, 2], [3, 4]])).tolist() == [70, 100], 'vecmat 1d 2d'
assert np.trapezoid(np.array([1, 2, 3])) == 4.0, 'trapezoid unit spacing'
assert np.trapezoid(np.array([1, 2, 3]), np.array([0, 1, 3])) == 6.5, 'trapezoid x coordinates'
assert np.trapezoid(np.array([1, 2, 3]), None, 2.0) == 8.0, 'trapezoid dx'
assert np.vander(np.array([1, 2, 3])).tolist() == [[1, 1, 1], [4, 2, 1], [9, 3, 1]], 'vander default'
assert np.vander(np.array([1, 2, 3]), 2).tolist() == [[1, 1], [2, 1], [3, 1]], 'vander n'
assert np.vander(np.array([1, 2, 3]), 3, True).tolist() == [
    [1, 1, 1],
    [1, 2, 4],
    [1, 3, 9],
], 'vander increasing'


# === integer and boolean bitwise helpers ===
assert np.bitwise_and(6, 3) == 2, 'bitwise_and scalar'
assert np.bitwise_and(True, False) == False, 'bitwise_and bool scalar'
assert np.bitwise_and([1, 2, 3], [3, 1, 2]).tolist() == [1, 0, 2], 'bitwise_and lists'
bool_and = np.bitwise_and(np.array([True, False]), True)
assert bool_and.tolist() == [True, False], 'bitwise_and bool array'
assert str(bool_and.dtype) == 'bool', 'bitwise_and bool dtype'

assert np.bitwise_or([1, 2, 4], 1).tolist() == [1, 3, 5], 'bitwise_or list scalar'
assert np.bitwise_xor(7, [1, 2, 4]).tolist() == [6, 5, 3], 'bitwise_xor scalar list'
assert np.bitwise_not([0, 1, -2]).tolist() == [-1, -2, 1], 'bitwise_not list'
assert np.bitwise_invert([0, 1]).tolist() == [-1, -2], 'bitwise_invert alias'
inverted_bools = np.invert(np.array([True, False]))
assert inverted_bools.tolist() == [False, True], 'invert bool array'
assert str(inverted_bools.dtype) == 'bool', 'invert bool dtype'

assert np.left_shift([1, 2, 3], 2).tolist() == [4, 8, 12], 'left_shift list scalar'
assert np.bitwise_left_shift(1, 3) == 8, 'bitwise_left_shift alias'
assert np.right_shift([8, -8], 1).tolist() == [4, -4], 'right_shift list scalar'
assert np.bitwise_right_shift(-8, 1) == -4, 'bitwise_right_shift alias'
assert np.bitwise_count(7) == 3, 'bitwise_count scalar'
assert np.bitwise_count([-1, -2, -3]).tolist() == [1, 1, 2], 'bitwise_count negative list'

packed = np.packbits([1, 0, 1, 1, 0, 0, 1, 0])
assert packed.tolist() == [178], 'packbits byte'
assert np.unpackbits(packed).tolist() == [1, 0, 1, 1, 0, 0, 1, 0], 'unpackbits roundtrip'


# === integer representation helpers ===
assert np.base_repr(10) == '1010', 'base_repr default base'
assert np.base_repr(-10) == '-1010', 'base_repr negative'
assert np.base_repr(10, 16) == 'A', 'base_repr hex'
assert np.base_repr(10, 2, 5) == '000001010', 'base_repr padding'
assert np.base_repr(0, 2, 5) == '00000', 'base_repr zero padding'

assert np.binary_repr(3) == '11', 'binary_repr positive'
assert np.binary_repr(-3) == '-11', 'binary_repr negative no width'
assert np.binary_repr(3, 5) == '00011', 'binary_repr positive width'
assert np.binary_repr(-3, 5) == '11101', 'binary_repr negative width'


# === finite conversion, predicates, and simple 1d helpers ===
assert np.isposinf([np.inf, -np.inf, 1.0]).tolist() == [True, False, False], 'isposinf values'
assert np.isneginf([np.inf, -np.inf, 1.0]).tolist() == [False, True, False], 'isneginf values'
assert np.asarray_chkfinite([1, 2, 3]).tolist() == [1, 2, 3], 'asarray_chkfinite finite'
try:
    np.asarray_chkfinite([1.0, np.inf])
    assert False, 'expected asarray_chkfinite to reject infinity'
except ValueError as exc:
    assert str(exc) == 'array must not contain infs or NaNs', 'asarray_chkfinite error message'

assert np.ascontiguousarray([1, 2]).tolist() == [1, 2], 'ascontiguousarray list'
assert np.asfortranarray([1, 2]).tolist() == [1, 2], 'asfortranarray list'
assert np.require([1, 2]).tolist() == [1, 2], 'require list'
assert np.real_if_close([1, 2]).tolist() == [1, 2], 'real_if_close list'

assert np.array_equiv([1, 2], [1, 2]) == True, 'array_equiv equal arrays'
assert np.array_equiv([1, 1], 1) == True, 'array_equiv scalar broadcast'
assert np.array_equiv([1, 2], 1) == False, 'array_equiv scalar mismatch'
assert np.ediff1d([[1, 2], [4, 7]]).tolist() == [1, 2, 3], 'ediff1d flattened'
assert np.trim_zeros([0, 0, 1, 0, 2, 0]).tolist() == [1, 0, 2], 'trim_zeros both'
assert np.trim_zeros([0, 0, 1, 0, 2, 0], 'f').tolist() == [1, 0, 2, 0], 'trim_zeros front'


# === real-only aliases and introspection helpers ===
real_values = np.array([-2, 0, 3])
assert np.conj(-5) == -5, 'conj scalar keeps real value'
assert np.conj(real_values).tolist() == [-2, 0, 3], 'conj array keeps real values'
assert np.conjugate([1.5, -2.5]).tolist() == [1.5, -2.5], 'conjugate list converts to array'
assert np.real(-4.5) == -4.5, 'real scalar keeps value'
assert np.real(real_values).tolist() == [-2, 0, 3], 'real array keeps values'
assert np.imag(7) == 0, 'imag int scalar is zero'
assert np.imag(1.25) == 0.0, 'imag float scalar is zero'
assert np.imag(real_values).tolist() == [0, 0, 0], 'imag int array is zeros'
assert np.imag([1.5, -2.5]).tolist() == [0.0, 0.0], 'imag float list is zeros'
assert np.isreal(3) == True, 'isreal scalar true'
assert np.isreal(real_values).tolist() == [True, True, True], 'isreal array all true'
assert np.iscomplex(3) == False, 'iscomplex scalar false'
assert np.iscomplex(real_values).tolist() == [False, False, False], 'iscomplex array all false'
assert np.isrealobj(real_values) == True, 'isrealobj array true'
assert np.isrealobj('text') == True, 'isrealobj string true'
assert np.iscomplexobj(real_values) == False, 'iscomplexobj array false'
assert np.iscomplexobj('text') == False, 'iscomplexobj string false'
assert np.isscalar(1) == True, 'isscalar int true'
assert np.isscalar('text') == True, 'isscalar string true'
assert np.isscalar(np.array([1])) == False, 'isscalar ndarray false'
assert np.isscalar([1]) == False, 'isscalar list false'
assert np.iterable([1, 2]) == True, 'iterable list true'
assert np.iterable((1, 2)) == True, 'iterable tuple true'
assert np.iterable('text') == True, 'iterable string true'
assert np.iterable(np.array([1, 2])) == True, 'iterable ndarray true'
assert np.iterable(1) == False, 'iterable int false'
