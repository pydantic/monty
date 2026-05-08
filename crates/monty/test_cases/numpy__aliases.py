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
assert np.angle(1.0) == 0.0, 'angle positive real scalar'
assert np.angle(-1.0) == np.pi, 'angle negative real scalar'
assert np.angle(-0.0) == np.pi, 'angle negative zero scalar'
assert np.angle([1.0, -1.0, 0.0, -0.0]).tolist() == [0.0, np.pi, 0.0, np.pi], 'angle real list'
assert np.angle([-1.0], True).tolist() == [180.0], 'angle degrees'


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
assert list(np.ndindex()) == [()], 'ndindex empty shape'
assert list(np.ndindex(2, 3)) == [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)], 'ndindex two dims'
assert list(np.ndindex((2, 1))) == [(0, 0), (1, 0)], 'ndindex tuple shape'
nd_iter_arr = np.array([[10, 20], [30, 40]])
assert list(np.ndenumerate(nd_iter_arr)) == [
    ((0, 0), 10),
    ((0, 1), 20),
    ((1, 0), 30),
    ((1, 1), 40),
], 'ndenumerate matrix'
assert list(np.ndenumerate(5)) == [((), 5)], 'ndenumerate scalar'
assert list(np.nditer(nd_iter_arr)) == [10, 20, 30, 40], 'nditer matrix order'
assert list(np.nditer(5)) == [5], 'nditer scalar'

diagflat = np.diagflat([[1, 2], [3, 4]])
assert diagflat.shape == (4, 4), 'diagflat flattened shape'
assert diagflat.tolist() == [[1, 0, 0, 0], [0, 2, 0, 0], [0, 0, 3, 0], [0, 0, 0, 4]], 'diagflat values'
assert np.diagflat([1, 2], 1).tolist() == [[0, 1, 0], [0, 0, 2], [0, 0, 0]], 'diagflat positive k'
assert np.diagflat([1, 2], -1).tolist() == [[0, 0, 0], [1, 0, 0], [0, 2, 0]], 'diagflat negative k'

ix_row, ix_col = np.ix_([0, 2], [1, 3, 4])
assert ix_row.shape == (2, 1), 'ix_ first shape'
assert ix_col.shape == (1, 3), 'ix_ second shape'
assert ix_row.tolist() == [[0], [2]], 'ix_ first values'
assert ix_col.tolist() == [[1, 3, 4]], 'ix_ second values'

mask_row, mask_col = np.mask_indices(3, np.triu, 1)
assert mask_row.tolist() == [0, 0, 1], 'mask_indices upper rows'
assert mask_col.tolist() == [1, 2, 2], 'mask_indices upper cols'
lower_row, lower_col = np.mask_indices(3, np.tril, -1)
assert lower_row.tolist() == [1, 2, 2], 'mask_indices lower rows'
assert lower_col.tolist() == [0, 0, 1], 'mask_indices lower cols'

memory_arr = np.array([[1, 2], [3, 4]])
memory_alias = memory_arr
memory_copy = memory_arr.copy()
assert np.isfortran(memory_arr) == False, 'isfortran row-major array'
assert np.shares_memory(memory_arr, memory_alias) == True, 'shares_memory same ndarray ref'
assert np.shares_memory(memory_arr, memory_copy) == False, 'shares_memory copied ndarray'
assert np.may_share_memory(memory_arr, memory_alias) == True, 'may_share_memory same ndarray ref'
assert np.may_share_memory(memory_arr, memory_copy) == False, 'may_share_memory copied ndarray'


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

dstack_1d = np.dstack(([1, 2], [3, 4]))
assert dstack_1d.shape == (1, 2, 2), 'dstack 1d shape'
assert dstack_1d.tolist() == [[[1, 3], [2, 4]]], 'dstack 1d values'
dstack_2d = np.dstack(([[1, 2], [3, 4]], [[5, 6], [7, 8]]))
assert dstack_2d.shape == (2, 2, 2), 'dstack 2d shape'
assert dstack_2d.tolist() == [[[1, 5], [2, 6]], [[3, 7], [4, 8]]], 'dstack 2d values'
block_scalar = np.block(1)
assert block_scalar.shape == (), 'block scalar shape'
assert block_scalar.tolist() == 1, 'block scalar value'
assert np.block(np.array([1, 2])).tolist() == [1, 2], 'block array leaf'
assert np.block([1, 2, 3]).tolist() == [1, 2, 3], 'block flat scalars'
assert np.block([[1, 2], [3, 4]]).tolist() == [[1, 2], [3, 4]], 'block nested scalars'
assert np.block([np.array([1, 2]), np.array([3, 4])]).tolist() == [1, 2, 3, 4], 'block flat arrays'
assert np.block([np.array([[1, 2]]), np.array([[3, 4]])]).tolist() == [[1, 2, 3, 4]], 'block flat matrices'
assert np.block(
    [
        [np.array([[1, 2]]), np.array([[3]])],
        [np.array([[4, 5]]), np.array([[6]])],
    ]
).tolist() == [[1, 2, 3], [4, 5, 6]], 'block matrix assembly'
try:
    np.block((1, 2))
    assert False, 'expected tuple block layout to fail'
except TypeError as exc:
    assert str(exc) == (
        'arrays is a tuple. Only lists can be used to arrange blocks, and np.block does not allow implicit conversion '
        'from tuple to ndarray.'
    ), 'block tuple layout error'
try:
    np.block([1, [2, 3]])
    assert False, 'expected mismatched block depth to fail'
except ValueError as exc:
    assert str(exc) == (
        'List depths are mismatched. First element was at depth 1, but there is an element at depth 2 (arrays[1][0])'
    ), 'block depth mismatch error'

depth_parts = np.dsplit(cube, 2)
assert len(depth_parts) == 2, 'dsplit equal section count'
assert depth_parts[0].shape == (2, 3, 2), 'dsplit first equal shape'
assert depth_parts[0].tolist()[0][0] == [0, 1], 'dsplit first equal values'
assert depth_parts[1].tolist()[1][2] == [22, 23], 'dsplit second equal values'
depth_index_parts = np.dsplit(cube, [1, 3])
assert len(depth_index_parts) == 3, 'dsplit index section count'
assert depth_index_parts[0].shape == (2, 3, 1), 'dsplit first index shape'
assert depth_index_parts[1].tolist()[0][1] == [5, 6], 'dsplit middle index values'
assert depth_index_parts[2].tolist()[1][2] == [23], 'dsplit final index values'

unstack_row0, unstack_row1 = np.unstack(matrix)
assert unstack_row0.tolist() == [1, 2, 3], 'unstack axis0 first row'
assert unstack_row1.tolist() == [4, 5, 6], 'unstack axis0 second row'
unstack_col0, unstack_col1, unstack_col2 = np.unstack(matrix, 1)
assert unstack_col0.tolist() == [1, 4], 'unstack axis1 first column'
assert unstack_col1.tolist() == [2, 5], 'unstack axis1 second column'
assert unstack_col2.tolist() == [3, 6], 'unstack axis1 third column'
unstack_scalar0, unstack_scalar1, unstack_scalar2 = np.unstack(np.array([1, 2, 3]))
assert unstack_scalar0 == 1, 'unstack 1d first scalar'
assert unstack_scalar1 == 2, 'unstack 1d second scalar'
assert unstack_scalar2 == 3, 'unstack 1d third scalar'

diag_mut = np.arange(9).reshape(3, 3)
assert np.fill_diagonal(diag_mut, 7) is None, 'fill_diagonal return'
assert diag_mut.tolist() == [[7, 1, 2], [3, 7, 5], [6, 7, 7]], 'fill_diagonal 2d values'

put_mut = np.array([0, 1, 2, 3, 4])
assert np.put(put_mut, [0, -1, 2], [10, 20]) is None, 'put return'
assert put_mut.tolist() == [10, 1, 10, 3, 20], 'put cycles values by index list'

copy_mut = np.array([0, 1, 2])
assert np.copyto(copy_mut, 5) is None, 'copyto scalar return'
assert copy_mut.tolist() == [5, 5, 5], 'copyto scalar broadcast'
copy_where = np.array([0, 1, 2])
assert np.copyto(copy_where, [7, 8, 9], where=[True, False, True]) is None, 'copyto where return'
assert copy_where.tolist() == [7, 1, 9], 'copyto where mask'

putmask_mut = np.array([0, 1, 2, 3, 4])
assert np.putmask(putmask_mut, [True, False, True, False, True], [9, 8]) is None, 'putmask return'
assert putmask_mut.tolist() == [9, 1, 9, 3, 9], 'putmask uses flat index cycling'

place_mut = np.array([0, 1, 2, 3, 4])
assert np.place(place_mut, [True, False, True, False, True], [5, 6]) is None, 'place return'
assert place_mut.tolist() == [5, 1, 6, 3, 5], 'place uses selected-position cycling'


# === linear algebra and numeric wrappers ===
assert np.vecdot(np.array([1, 2, 3]), np.array([4, 5, 6])) == 32, 'vecdot 1d'
assert np.matvec(np.array([[1, 2], [3, 4]]), np.array([10, 20])).tolist() == [50, 110], 'matvec 2d 1d'
assert np.vecmat(np.array([10, 20]), np.array([[1, 2], [3, 4]])).tolist() == [70, 100], 'vecmat 1d 2d'
assert np.tensordot(np.array([1, 2]), np.array([3, 4]), axes=0).tolist() == [[3, 4], [6, 8]], 'tensordot outer'
assert np.tensordot(np.array([1, 2]), np.array([3, 4]), axes=1) == 11, 'tensordot vector scalar'
assert np.tensordot(np.array([[1, 2], [3, 4]]), np.array([10, 20]), axes=1).tolist() == [
    50,
    110,
], 'tensordot matrix vector'
assert np.tensordot(
    np.array([[[1, 2], [3, 4]], [[5, 6], [7, 8]]]),
    np.array([[10, 20], [30, 40]]),
    ([1, 2], [0, 1]),
).tolist() == [300, 700], 'tensordot explicit axes'
assert np.einsum('i,i->', np.array([1, 2]), np.array([3, 4])) == 11, 'einsum dot'
assert np.einsum('ij,j->i', np.array([[1, 2], [3, 4]]), np.array([10, 20])).tolist() == [
    50,
    110,
], 'einsum matrix vector'
assert np.einsum('ij,jk->ik', np.array([[1, 2], [3, 4]]), np.array([[10, 20], [30, 40]])).tolist() == [
    [70, 100],
    [150, 220],
], 'einsum matrix multiply'
assert np.einsum('ij->ji', np.array([[1, 2], [3, 4]])).tolist() == [[1, 3], [2, 4]], 'einsum transpose'
assert np.einsum('ii->i', np.array([[1, 2], [3, 4]])).tolist() == [1, 4], 'einsum diagonal'
assert np.einsum('i,j->ij', np.array([1, 2]), np.array([3, 4])).tolist() == [[3, 4], [6, 8]], 'einsum outer'
einsum_path, einsum_path_details = np.einsum_path('ij,jk->ik', np.array([[1, 2]]), np.array([[3], [4]]))
assert einsum_path == ['einsum_path', (0, 1)], 'einsum_path simple path'
assert len(einsum_path_details) > 0, 'einsum_path details are nonempty'
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
assert np.polyadd([1, 2, 3], [10, 20]).tolist() == [1, 12, 23], 'polyadd aligns coefficients'
assert np.polysub([1, 2, 3], [10, 20]).tolist() == [1, -8, -17], 'polysub aligns coefficients'
assert np.polymul([1, 2], [3, 4]).tolist() == [3, 10, 8], 'polymul convolution'
assert np.poly([1, 2, 3]).tolist() == [1.0, -6.0, 11.0, -6.0], 'poly roots to coefficients'
polydiv_exact_q, polydiv_exact_r = np.polydiv([1, 0, -1], [1, -1])
assert polydiv_exact_q.tolist() == [1.0, 1.0], 'polydiv exact quotient'
assert polydiv_exact_r.tolist() == [0.0], 'polydiv exact remainder'
polydiv_rem_q, polydiv_rem_r = np.polydiv([1, 2, 3], [1, 1])
assert polydiv_rem_q.tolist() == [1.0, 1.0], 'polydiv quotient with remainder'
assert polydiv_rem_r.tolist() == [2.0], 'polydiv nonzero remainder'
assert np.polyval([1, 0, -1], 2) == 3, 'polyval scalar'
assert np.polyval([1, 0, -1], [0, 1, 2]).tolist() == [-1, 0, 3], 'polyval array'
assert np.polyder([1, 2, 3]).tolist() == [2, 2], 'polyder first derivative'
assert np.polyder([1, 2, 3], 2).tolist() == [2], 'polyder second derivative'
assert np.polyint([2, 2]).tolist() == [1.0, 2.0, 0.0], 'polyint first integral'
assert np.polyint([2, 2], 2).tolist() == [1 / 3, 1.0, 0.0, 0.0], 'polyint second integral'
assert np.kron([1, 2], [10, 20, 30]).tolist() == [10, 20, 30, 20, 40, 60], 'kron 1d'
assert np.kron([[1, 2], [3, 4]], [[0, 5], [6, 7]]).tolist() == [
    [0, 5, 0, 10],
    [6, 7, 12, 14],
    [0, 15, 0, 20],
    [18, 21, 24, 28],
], 'kron 2d'
assert np.cov([1, 2, 3]) == 1.0, 'cov 1d'
assert np.cov([[1, 2, 3], [2, 4, 6]]).tolist() == [[1.0, 2.0], [2.0, 4.0]], 'cov 2d rows'
assert np.corrcoef([1, 2, 3]) == 1.0, 'corrcoef 1d'
assert np.corrcoef([[1, 2, 3], [2, 4, 6]]).tolist() == [[1.0, 1.0], [1.0, 1.0]], 'corrcoef 2d rows'

unique_input = np.array([3, 1, 3, 2, 1, 3])
assert np.sort(np.unique_values(unique_input)).tolist() == [1, 2, 3], 'unique_values sorted contents'
unique_counts = np.unique_counts(unique_input)
assert unique_counts.values.tolist() == [1, 2, 3], 'unique_counts values'
assert unique_counts.counts.tolist() == [2, 1, 3], 'unique_counts counts'
unique_inverse = np.unique_inverse(unique_input)
assert unique_inverse.values.tolist() == [1, 2, 3], 'unique_inverse values'
assert unique_inverse.inverse_indices.tolist() == [2, 0, 2, 1, 0, 2], 'unique_inverse indices'
unique_all = np.unique_all(unique_input)
assert unique_all.values.tolist() == [1, 2, 3], 'unique_all values'
assert unique_all.indices.tolist() == [1, 3, 0], 'unique_all first indices'
assert unique_all.inverse_indices.tolist() == [2, 0, 2, 1, 0, 2], 'unique_all inverse indices'
assert unique_all.counts.tolist() == [2, 1, 3], 'unique_all counts'

partition_input = np.array([3, 1, 2])
assert np.partition(partition_input, 1).tolist() == [1, 2, 3], 'partition 1d deterministic sorted subset'
assert np.argpartition(partition_input, 1).tolist() == [1, 2, 0], 'argpartition 1d deterministic argsort subset'
partition_neg = np.partition(partition_input, -1).tolist()
assert partition_neg[-1] == 3, 'partition negative kth places max'
assert sorted(partition_neg) == [1, 2, 3], 'partition negative kth preserves values'
lex_row = np.lexsort(([2, 1, 2, 1], [0, 1, 0, 0]))
assert lex_row.tolist() == [3, 0, 2, 1], 'lexsort two keys'
assert np.lexsort(([3, 1, 2],)).tolist() == [1, 2, 0], 'lexsort one key'


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
assert np.unwrap([0.0, 1.0, 2.0]).tolist() == [0.0, 1.0, 2.0], 'unwrap no jump'
unwrapped_pos = np.unwrap([0.0, 3.5, 6.0]).tolist()
assert abs(unwrapped_pos[1] + 2.7831853071795862) < 1e-12, 'unwrap positive jump first'
assert abs(unwrapped_pos[2] + 0.28318530717958623) < 1e-12, 'unwrap positive jump second'
unwrapped_neg = np.unwrap([0.0, -3.5, -6.0]).tolist()
assert abs(unwrapped_neg[1] - 2.7831853071795862) < 1e-12, 'unwrap negative jump first'
assert abs(unwrapped_neg[2] - 0.28318530717958623) < 1e-12, 'unwrap negative jump second'


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


# === dtype aliases and scalar constants ===
assert np.array([1.2, -2.8]).astype(np.int_).tolist() == [1, -2], 'int_ dtype alias'
assert np.array([1.2, -2.8]).astype(np.intc).tolist() == [1, -2], 'intc dtype alias'
assert np.array([1.2, 2.8]).astype(np.uint8).tolist() == [1, 2], 'uint8 dtype alias'
assert np.array([1, 0, -2]).astype(np.bool).tolist() == [True, False, True], 'bool dtype alias'
assert np.array([1, 2]).astype(np.double).tolist() == [1.0, 2.0], 'double dtype alias'
assert np.array([1, 2]).astype(np.float16).tolist() == [1.0, 2.0], 'float16 dtype alias'
assert np.array([1, 2]).astype(np.longdouble).tolist() == [1.0, 2.0], 'longdouble dtype alias'
assert np.array([1, 2]).astype('uint8').tolist() == [1, 2], 'astype string uint alias'
assert np.dtype('float64') == np.float64, 'dtype normalizes float64 string'
assert np.dtype(np.float32) == np.float32, 'dtype preserves compact float32 marker'
assert np.dtype(int) == np.int64, 'dtype normalizes Python int type'
assert np.dtype(float) == np.float64, 'dtype normalizes Python float type'
assert np.dtype(bool) == np.bool_, 'dtype normalizes Python bool type'
assert str(np.dtype('int64')) == 'int64', 'dtype string display'
assert np.dtype(np.complex64) == np.complex64, 'dtype preserves complex64 metadata marker'
assert np.dtype(np.csingle) == np.complex64, 'dtype normalizes csingle metadata alias'
assert np.dtype(np.cdouble) == np.complex128, 'dtype normalizes cdouble metadata alias'
assert np.dtype(np.clongdouble) == np.complex128, 'dtype normalizes clongdouble metadata alias'
assert np.dtype(np.str_) == np.str_, 'dtype preserves str_ metadata marker'
assert np.dtype(np.bytes_) == np.bytes_, 'dtype preserves bytes_ metadata marker'
assert np.dtype(np.void) == np.void, 'dtype preserves void metadata marker'
assert np.dtype(np.object_) == np.object_, 'dtype preserves object_ metadata marker'
assert np.dtype(np.datetime64) == np.datetime64, 'dtype preserves datetime64 metadata marker'
assert np.dtype(np.timedelta64) == np.timedelta64, 'dtype preserves timedelta64 metadata marker'
try:
    np.astype(np.array([1]), np.complex64)
    assert False, 'expected module astype complex64 to fail'
except TypeError as exc:
    assert str(exc) == 'numpy.astype() unsupported dtype: complex64', 'module astype rejects complex storage'
try:
    np.array([1]).astype(np.str_)
    assert False, 'expected ndarray astype str_ to fail'
except TypeError as exc:
    assert str(exc) == 'unsupported dtype: str_', 'ndarray astype rejects string storage'
assert np.astype(np.array([1.2, -2.8]), np.int64).tolist() == [1, -2], 'module astype int dtype'
assert np.astype(np.array([1, 0, -2]), bool).tolist() == [True, False, True], 'module astype bool dtype'
assert np.astype(np.array([1, 2]), np.float64).tolist() == [1.0, 2.0], 'module astype float dtype'
assert isinstance(np.array([1, 2]), np.ndarray) == True, 'ndarray type object matches arrays'
assert np.ndarray.__name__ == 'ndarray', 'ndarray type object name'
assert repr(np.ndarray) == "<class 'numpy.ndarray'>", 'ndarray type object repr'
flat_marker = np.array([[1, 2], [3, 4]]).flat
assert isinstance(flat_marker, np.flatiter) == True, 'flatiter marker matches ndarray flat result'
assert isinstance(np.array([1, 2, 3]), np.flatiter) == False, 'plain ndarray is not flatiter'
assert np.flatiter.__name__ == 'flatiter', 'flatiter type object name'
assert type(flat_marker).__name__ == 'flatiter', 'flatiter result type name'
assert flat_marker.tolist() == [1, 2, 3, 4], 'flatiter marker backed by flat data'
assert list(flat_marker) == [1, 2, 3, 4], 'flatiter iteration follows flat data'
assert isinstance(np.add, np.ufunc) == True, 'add is public ufunc-like callable'
assert isinstance(np.sin, np.ufunc) == True, 'sin is public ufunc-like callable'
assert isinstance(np.array, np.ufunc) == False, 'array constructor is not a ufunc'
assert type(np.add).__name__ == 'ufunc', 'ufunc callable type name'
assert np.ufunc.__name__ == 'ufunc', 'ufunc type object name'
assert np.can_cast(np.int8, np.int16) == True, 'can_cast integer aliases'
assert np.can_cast(np.float64, np.int64) == False, 'can_cast float to int is unsafe'
assert np.promote_types(np.int8, np.float32) == np.float32, 'promote_types int float32'
assert np.result_type(np.array([1, 2]), 1.5) == np.float64, 'result_type array scalar'
assert np.common_type(np.array([1, 2]), np.array([3, 4])) == np.float64, 'common_type int arrays'
assert np.min_scalar_type(-3) == np.int8, 'min_scalar_type negative int alias'
assert np.min_scalar_type(3) == np.uint8, 'min_scalar_type positive int alias'
assert np.issubdtype(np.int64, np.integer) == True, 'issubdtype integer category'
assert np.issubdtype(np.float64, np.floating) == True, 'issubdtype floating category'
assert np.issubdtype(np.float64, np.inexact) == True, 'issubdtype inexact category'
assert np.issubdtype(np.int64, np.inexact) == False, 'issubdtype int not inexact'
assert np.issubdtype(np.bool_, np.integer) == False, 'issubdtype bool not integer'
assert np.issubdtype(np.complex64, np.complexfloating) == True, 'complex64 is complexfloating'
assert np.issubdtype(np.complex128, np.inexact) == True, 'complex128 is inexact'
assert np.issubdtype(np.complex128, np.number) == True, 'complex128 is number'
assert np.issubdtype(np.str_, np.character) == True, 'str_ is character'
assert np.issubdtype(np.bytes_, np.character) == True, 'bytes_ is character'
assert np.issubdtype(np.void, np.flexible) == True, 'void is flexible'
assert np.issubdtype(np.object_, np.generic) == True, 'object_ is generic'
assert np.issubdtype(np.datetime64, np.generic) == True, 'datetime64 is generic'
assert np.issubdtype(np.timedelta64, np.number) == True, 'timedelta64 is number'
assert np.isdtype(np.float64, 'real floating') == True, 'isdtype real floating'
assert np.isdtype(np.int64, 'integral') == True, 'isdtype integral'
assert np.isdtype(np.int64, 'numeric') == True, 'isdtype numeric int'
assert np.isdtype(np.bool_, 'numeric') == False, 'isdtype numeric excludes bool'
assert np.isdtype(np.complex64, 'complex floating') == True, 'isdtype complex floating'
assert np.isdtype(np.complex64, 'numeric') == True, 'isdtype numeric complex'
assert np.isdtype(np.str_, 'numeric') == False, 'isdtype string not numeric'
assert np.can_cast(np.complex64, np.complex128) == True, 'can_cast complex64 to complex128'
assert np.can_cast(np.complex64, np.float64) == False, 'can_cast complex to float is unsafe'
assert np.can_cast(np.str_, np.object_) == True, 'can_cast str to object'
assert np.can_cast(np.void, np.str_) == False, 'can_cast void to str false'
assert np.promote_types(np.complex64, np.float32) == np.complex64, 'promote complex64 float32'
assert np.promote_types(np.complex64, np.int64) == np.complex128, 'promote complex64 int64'
assert np.result_type(np.complex64, np.float32) == np.complex64, 'result_type complex64 float32'
assert np.result_type(np.object_, np.int64) == np.object_, 'result_type object int64'
assert isinstance(1, np.ScalarType) == True, 'ScalarType includes int'
assert isinstance(1.5, np.ScalarType) == True, 'ScalarType includes float'
assert isinstance('x', np.ScalarType) == True, 'ScalarType includes str'
assert isinstance([], np.ScalarType) == False, 'ScalarType excludes list'
float64_info = np.finfo(np.float64)
assert float64_info.bits == 64, 'finfo float64 bits'
assert float64_info.eps == 2.220446049250313e-16, 'finfo float64 eps'
assert float64_info.tiny == 2.2250738585072014e-308, 'finfo float64 tiny'
assert str(float64_info.dtype) == 'float64', 'finfo float64 dtype'
float32_info = np.finfo('float32')
assert float32_info.bits == 32, 'finfo float32 bits'
assert float32_info.precision == 6, 'finfo float32 precision'
assert float32_info.resolution == 1e-06, 'finfo float32 resolution'
assert np.finfo('float16').tiny == 6.103515625e-05, 'finfo float16 tiny'
int16_info = np.iinfo('int16')
assert int16_info.bits == 16, 'iinfo int16 bits'
assert int16_info.min == -32768, 'iinfo int16 min'
assert int16_info.max == 32767, 'iinfo int16 max'
assert str(int16_info.dtype) == 'int16', 'iinfo int16 dtype'
assert np.iinfo('uint8').max == 255, 'iinfo uint8 max'
assert np.iinfo('uint64').max == 18446744073709551615, 'iinfo uint64 max'
assert np.iinfo(1).bits == 64, 'iinfo scalar int bits'


# === float formatting helpers ===
assert np.format_float_positional(1.0) == '1.', 'format_float_positional whole float'
assert np.format_float_positional(1.2, precision=2, unique=False) == '1.20', 'format_float_positional fixed precision'
assert np.format_float_positional(1.2, sign=True) == '+1.2', 'format_float_positional sign'
assert np.format_float_positional(1.2, min_digits=4) == '1.2000', 'format_float_positional min digits'
assert np.format_float_scientific(1.0) == '1.e+00', 'format_float_scientific whole float'
assert np.format_float_scientific(1000.0) == '1.e+03', 'format_float_scientific positive exponent'
assert np.format_float_scientific(0.0001234) == '1.234e-04', 'format_float_scientific negative exponent'
assert np.format_float_scientific(1.2, precision=2, unique=False) == '1.20e+00', (
    'format_float_scientific fixed precision'
)


def fromfunction_sum(row, col):
    return row + col


def fromfunction_linear(row, col):
    return row * 10 + col


def fromfunction_scale(index, scale=1):
    return index * scale


assert np.fromfunction(fromfunction_sum, (2, 3)).tolist() == [
    [0.0, 1.0, 2.0],
    [1.0, 2.0, 3.0],
], 'fromfunction float coordinate sum'
assert np.fromfunction(fromfunction_linear, (2, 3), dtype=int).tolist() == [
    [0, 1, 2],
    [10, 11, 12],
], 'fromfunction integer coordinate expression'
assert np.fromfunction(fromfunction_scale, (4,), dtype=int, scale=3).tolist() == [
    0,
    3,
    6,
    9,
], 'fromfunction forwards callable kwargs'
assert np.fromfunction(fromfunction_scale, (0,), dtype=int).tolist() == [], 'fromfunction empty shape'
assert np.fromiter([1, 2, 3], int).tolist() == [1, 2, 3], 'fromiter int list'
assert np.fromiter([1, 2.5, 3], float).tolist() == [1.0, 2.5, 3.0], 'fromiter float list'
assert np.fromiter([1, 2, 3], np.int64, count=2).tolist() == [1, 2], 'fromiter count keyword'
assert np.fromiter([True, False], bool).tolist() == [True, False], 'fromiter bool dtype'
assert np.fromiter([1, 2], None).tolist() == [1.0, 2.0], 'fromiter dtype none default'
assert np.fromstring('1 2 3', dtype=int, sep=' ').tolist() == [1, 2, 3], 'fromstring int whitespace'
assert np.fromstring('1, 2, 3', dtype=float, sep=',').tolist() == [
    1.0,
    2.0,
    3.0,
], 'fromstring float comma'
assert np.fromstring('1,2,3', dtype=np.int64, count=2, sep=',').tolist() == [1, 2], 'fromstring count'
assert np.fromstring('1 0 2', dtype=bool, sep=' ').tolist() == [True, False, True], 'fromstring bool'
assert np.mintypecode(['i', 'f']) == 'f', 'mintypecode int float'
assert np.typename('i') == 'integer', 'typename integer code'
assert np.typename('d') == 'double precision', 'typename double code'
assert np.typecodes['Float'] == 'efdg', 'typecodes float family'
assert np.sctypeDict['float64'] == np.float64, 'sctypeDict float64 alias'
assert np.sctypeDict['int32'] == np.int32, 'sctypeDict int32 alias'
assert np.sctypeDict['bool'] == np.bool_, 'sctypeDict bool alias'
assert np.sctypeDict['complex64'] == np.complex64, 'sctypeDict complex64 alias'
assert np.sctypeDict['str_'] == np.str_, 'sctypeDict str_ alias'
assert np.sctypeDict['object_'] == np.object_, 'sctypeDict object_ alias'
assert np.info is not None, 'info export present'
err_policy = np.geterr()
assert err_policy['divide'] == 'warn', 'geterr divide policy'
assert err_policy['under'] == 'ignore', 'geterr under policy'
seterr_previous = np.seterr(divide='ignore')
assert seterr_previous['divide'] == 'warn', 'seterr returns previous policy'
np.seterr(divide='warn')
print_options = np.get_printoptions()
assert print_options['threshold'] == 1000, 'get_printoptions threshold'
assert print_options['precision'] == 8, 'get_printoptions precision'
assert np.set_printoptions(threshold=10) is None, 'set_printoptions return'
np.set_printoptions(threshold=1000)
assert np.getbufsize() == 8192, 'getbufsize default'
assert np.setbufsize(8192) == 8192, 'setbufsize previous size'
assert np.errstate(divide='ignore') is not None, 'errstate placeholder'
assert np.printoptions(threshold=10) is not None, 'printoptions placeholder'
assert np.geterrcall() is None, 'geterrcall default'
assert np.seterrcall(None) is None, 'seterrcall previous callback'
assert np.geterrcall() is None, 'geterrcall after reset'
assert np.show_runtime is not None, 'show_runtime export present'
assert np.test is not None, 'test export present'
display_int = np.array([1, 2, 3])
assert np.array2string(display_int) == '[1 2 3]', 'array2string int vector'
assert np.array_str(display_int) == '[1 2 3]', 'array_str int vector'
assert np.array_repr(display_int) == 'array([1, 2, 3])', 'array_repr int vector'
display_float = np.array([1.0, 2.0, 3.0])
assert np.array2string(display_float) == '[1. 2. 3.]', 'array2string float vector'
assert np.array_str(display_float) == '[1. 2. 3.]', 'array_str float vector'
display_bool = np.array([True, False])
assert np.array2string(display_bool) == '[ True False]', 'array2string bool vector'
display_matrix = np.array([[1, 2], [3, 4]])
assert np.array2string(display_matrix) == '[[1 2]\n [3 4]]', 'array2string matrix'
assert np.array_str(display_matrix) == '[[1 2]\n [3 4]]', 'array_str matrix'
display_empty = np.array([])
assert np.array2string(display_empty) == '[]', 'array2string empty'
assert np.array_str(display_empty) == '[]', 'array_str empty'
assert np.array_repr(display_empty) == 'array([], dtype=float64)', 'array_repr empty'
choose_idx = np.array([0, 1, 0, 1])
assert np.choose(choose_idx, [[10, 20, 30, 40], [50, 60, 70, 80]]).tolist() == [
    10,
    60,
    30,
    80,
], 'choose vector'
assert np.resize([1, 2, 3], (2, 4)).tolist() == [[1, 2, 3, 1], [2, 3, 1, 2]], 'resize repeats'
take_axis_arr = np.array([[10, 20, 30], [40, 50, 60]])
take_axis_idx = np.array([[2, 1], [0, 2]])
assert np.take_along_axis(take_axis_arr, take_axis_idx, axis=1).tolist() == [
    [30, 20],
    [40, 60],
], 'take_along_axis axis 1'
assert np.take_along_axis(take_axis_arr, np.array([[1, 0, 1]]), axis=0).tolist() == [[40, 20, 60]], (
    'take_along_axis axis 0'
)
put_axis_arr = np.array([[10, 20, 30], [40, 50, 60]])
assert np.put_along_axis(put_axis_arr, take_axis_idx, [[99, 88], [77, 66]], axis=1) is None, 'put_along_axis return'
assert put_axis_arr.tolist() == [[10, 88, 99], [77, 50, 66]], 'put_along_axis axis 1'


# === axis and subarray helpers ===
axis_arr = np.array([[1, 2, 3], [4, 5, 6]])


def apply_row_summary(row):
    return np.array([row[0], row[-1], row.sum()])


def apply_column_product(col):
    return col[0] * col[-1]


def apply_offset(row, offset=0):
    return row + offset


assert np.apply_along_axis(apply_row_summary, 1, axis_arr).tolist() == [
    [1, 3, 6],
    [4, 6, 15],
], 'apply_along_axis inserts vector result at iterated axis'
assert np.apply_along_axis(apply_column_product, 0, axis_arr).tolist() == [4, 10, 18], 'apply_along_axis scalar result'
assert np.apply_along_axis(apply_offset, -1, axis_arr, offset=10).tolist() == [
    [11, 12, 13],
    [14, 15, 16],
], 'apply_along_axis forwards callable kwargs'
assert np.apply_over_axes(np.sum, axis_arr, [0]).tolist() == [[5, 7, 9]], 'apply_over_axes sum axis 0'
assert np.apply_over_axes(np.sum, axis_arr, [0, 1]).tolist() == [[21]], 'apply_over_axes sum axes 0 and 1'
assert np.apply_over_axes(np.prod, axis_arr, (1,)).tolist() == [[6], [120]], 'apply_over_axes prod axis 1'
piecewise_x = np.array([0, 1, 2, 3])
assert np.piecewise(
    piecewise_x,
    [np.array([True, False, False, False]), np.array([False, True, True, False])],
    [10, lambda x: x + 20, 99],
).tolist() == [10, 21, 22, 99], 'piecewise scalar callable and default'
assert np.piecewise(axis_arr, [axis_arr > 3], [lambda x: x * 10, -1]).tolist() == [
    [-1, -1, -1],
    [40, 50, 60],
], 'piecewise broadcasts condition over array'
assert np.piecewise(
    np.array([1, 2, 3]),
    [np.array([True, True, False]), np.array([True, False, True])],
    [10, 20, 0],
).tolist() == [20, 10, 20], 'piecewise later conditions overwrite earlier matches'
assert np.pad(axis_arr, 1, mode='constant', constant_values=9).tolist() == [
    [9, 9, 9, 9, 9],
    [9, 1, 2, 3, 9],
    [9, 4, 5, 6, 9],
    [9, 9, 9, 9, 9],
], 'pad constant scalar width and value'
assert np.pad(axis_arr, ((1, 0), (2, 1)), mode='constant', constant_values=((7, 8), (9, 10))).tolist() == [
    [9, 9, 7, 7, 7, 10],
    [9, 9, 1, 2, 3, 10],
    [9, 9, 4, 5, 6, 10],
], 'pad constant per-axis widths and values'
assert np.pad(axis_arr, ((1, 1), (2, 1)), mode='edge').tolist() == [
    [1, 1, 1, 2, 3, 3],
    [1, 1, 1, 2, 3, 3],
    [4, 4, 4, 5, 6, 6],
    [4, 4, 4, 5, 6, 6],
], 'pad edge mode'
assert np.pad([1, 2, 3], 2, mode='reflect').tolist() == [3, 2, 1, 2, 3, 2, 1], 'pad reflect mode'
assert np.nanquantile([1.0, float('nan'), 3.0], 0.5) == 2.0, 'nanquantile median'
assert np.nanpercentile([1.0, float('nan'), 3.0], 50) == 2.0, 'nanpercentile median'
hist_counts, hist_edges = np.histogram([0, 1, 1, 2, 3], bins=3)
assert hist_counts.tolist() == [1, 2, 2], 'histogram counts'
assert hist_edges.tolist() == [0.0, 1.0, 2.0, 3.0], 'histogram edges'
assert np.histogram_bin_edges([0, 1, 1, 2, 3], bins=3).tolist() == [
    0.0,
    1.0,
    2.0,
    3.0,
], 'histogram_bin_edges'
hist2d_counts, hist2d_xedges, hist2d_yedges = np.histogram2d([0, 1, 1, 2], [0, 0, 1, 1], bins=2)
assert hist2d_counts.tolist() == [[1.0, 0.0], [1.0, 2.0]], 'histogram2d counts'
assert hist2d_xedges.tolist() == [0.0, 1.0, 2.0], 'histogram2d xedges'
assert hist2d_yedges.tolist() == [0.0, 0.5, 1.0], 'histogram2d yedges'
histdd_counts, histdd_edges = np.histogramdd(np.array([[0, 0], [1, 0], [1, 1], [2, 1]]), bins=2)
assert histdd_counts.tolist() == [[1.0, 0.0], [1.0, 2.0]], 'histogramdd counts'
assert [edge.tolist() for edge in histdd_edges] == [
    [0.0, 1.0, 2.0],
    [0.0, 0.5, 1.0],
], 'histogramdd edges'
assert np.little_endian == True, 'little_endian constant'
assert abs(np.euler_gamma - 0.5772156649015329) < 1e-15, 'euler_gamma constant'
