# skip-cpython
import numpy as np


# === Broadcast shape helpers ===
assert np.broadcast_shapes((3, 1), (1, 4)) == (3, 4), 'broadcast_shapes should combine singleton axes'
assert np.broadcast_to(np.array([1, 2, 3]), (2, 3)).tolist() == [[1, 2, 3], [1, 2, 3]], (
    'broadcast_to should materialize leading singleton axes'
)
assert np.broadcast_to(5, (2, 2)).tolist() == [[5, 5], [5, 5]], 'broadcast_to should accept scalar input'

broadcasted = np.broadcast_arrays(np.array([[1], [2]]), np.array([10, 20, 30]))
assert len(broadcasted) == 2, 'broadcast_arrays should return one result per input'
assert [array.tolist() for array in broadcasted] == [
    [[1, 1, 1], [2, 2, 2]],
    [[10, 20, 30], [10, 20, 30]],
], 'broadcast_arrays should materialize all inputs to the shared shape'
assert list(np.broadcast(np.array([1, 2]), 10)) == [(1, 10), (2, 10)], (
    'broadcast subset should provide NumPy-compatible iteration payloads'
)


# === Ufunc and operator broadcasting ===
column = np.array([[1], [2]])
row = np.array([10, 20, 30])
assert (column + row).tolist() == [[11, 21, 31], [12, 22, 32]], 'ndarray operators should broadcast'
assert np.add(column, row).tolist() == [[11, 21, 31], [12, 22, 32]], 'np.add should broadcast arrays'
assert np.maximum(column, row).tolist() == [[10, 20, 30], [10, 20, 30]], 'pairwise math should broadcast'
assert (column < np.array([2, 2, 2])).tolist() == [[True, True, True], [False, False, False]], (
    'comparisons should broadcast'
)
assert np.logical_and(np.array([[True], [False]]), np.array([True, False, True])).tolist() == [
    [True, False, True],
    [False, False, False],
], 'logical ufuncs should broadcast'


# === Selection and testing helpers ===
assert np.where(np.array([[True], [False]]), np.array([1, 2, 3]), 0).tolist() == [
    [1, 2, 3],
    [0, 0, 0],
], 'where should broadcast condition and choices'
assert np.isclose(np.array([[1.0], [2.0]]), np.array([1.0, 3.0, 2.0])).tolist() == [
    [True, False, False],
    [False, False, True],
], 'isclose should broadcast and preserve result shape'
assert not np.allclose(np.array([[1.0], [2.0]]), np.array([1.0, 2.0, 2.0])), (
    'allclose should compare the broadcasted result'
)
assert np.array_equiv(np.array([[1], [1]]), np.array([1, 1, 1])), 'array_equiv should use broadcast equality'


# === Integer and bitwise broadcasting ===
assert np.gcd(np.array([[6], [10]]), np.array([4, 5, 6])).tolist() == [
    [2, 1, 6],
    [2, 5, 2],
], 'integer ufuncs should broadcast'
assert np.bitwise_and(np.array([[True], [False]]), np.array([True, False])).tolist() == [
    [True, False],
    [False, False],
], 'bitwise boolean ufuncs should broadcast'


# === Broadcast errors ===
try:
    np.add(np.ones((2, 3)), np.ones((2,)))
    assert False, 'expected incompatible broadcast to fail'
except ValueError as exc:
    assert str(exc) == 'operands could not be broadcast together with shapes (2,3) (2,) ', (
        'broadcast errors should match NumPy ufunc shape messages'
    )

try:
    np.broadcast_shapes((2, 3), (2,))
    assert False, 'expected incompatible broadcast_shapes input to fail'
except ValueError as exc:
    assert str(exc) == (
        'shape mismatch: objects cannot be broadcast to a single shape.  '
        'Mismatch is between arg 0 with shape (2, 3) and arg 1 with shape (2,).'
    ), 'broadcast_shapes should match NumPy public helper shape messages'
