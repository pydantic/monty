# skip-cpython
# === Aggregation methods ===
import numpy as np

a = np.array([1, 2, 3, 4, 5])

# === sum ===
assert a.sum() == 15, 'sum'
assert np.sum(a) == 15, 'np.sum'

# === mean ===
assert a.mean() == 3.0, 'mean'
assert np.mean(a) == 3.0, 'np.mean'

# === min / max ===
assert a.min() == 1, 'min'
assert a.max() == 5, 'max'
assert np.min(a) == 1, 'np.min'
assert np.max(a) == 5, 'np.max'

# === std ===
b = np.array([2, 4, 4, 4, 5, 5, 7, 9])
assert b.mean() == 5.0, 'mean for std'
assert b.std() == 2.0, 'std'

# === reshape ===
c = np.array([1, 2, 3, 4, 5, 6])
d = c.reshape(2, 3)
assert d.shape == (2, 3), 'reshape shape'
assert d[0][0] == 1, 'reshape [0][0]'
assert d[0][2] == 3, 'reshape [0][2]'
assert d[1][0] == 4, 'reshape [1][0]'
assert d[1][2] == 6, 'reshape [1][2]'

# === flatten ===
e = d.flatten()
assert e.shape == (6,), 'flatten shape'
assert e[0] == 1, 'flatten first'
assert e[5] == 6, 'flatten last'

# === tolist ===
f = np.array([1, 2, 3])
result = f.tolist()
assert result == [1, 2, 3], 'tolist'
assert type(result) == list, 'tolist returns list'

# === argmin / argmax ===
g = np.array([3, 1, 4, 1, 5])
assert g.argmin() == 1, 'argmin'
assert g.argmax() == 4, 'argmax'

# === cumsum ===
h = np.array([1, 2, 3, 4])
cs = h.cumsum()
assert cs.tolist() == [1, 3, 6, 10], 'cumsum method'

# === abs (via np.abs, not method) ===
neg = np.array([-1, 2, -3])
assert np.abs(neg).tolist() == [1, 2, 3], 'np.abs function'

# === np.abs / np.sqrt / np.exp / np.ceil / np.floor on plain lists ===
assert np.abs([-1, 2, -3]).tolist() == [1, 2, 3], 'np.abs(list)'
assert np.sqrt([1.0, 4.0, 9.0]).tolist() == [1.0, 2.0, 3.0], 'np.sqrt(list)'
assert np.exp([0.0]).tolist() == [1.0], 'np.exp(list)'
assert np.ceil([1.2, 2.7]).tolist() == [2.0, 3.0], 'np.ceil(list)'
assert np.floor([1.8, 2.3]).tolist() == [1.0, 2.0], 'np.floor(list)'

# === round ===
floats = np.array([1.234, 2.567, 3.891])
assert floats.round(1).tolist() == [1.2, 2.6, 3.9], 'round method'

# === clip ===
arr = np.array([1, 5, 10, 15, 20])
assert arr.clip(5, 15).tolist() == [5, 5, 10, 15, 15], 'clip method'

# === sort method (returns new sorted array) ===
unsorted = np.array([3, 1, 4, 1, 5])
sorted_arr = np.sort(unsorted)
assert sorted_arr.tolist() == [1, 1, 3, 4, 5], 'sort'

# === np.mean / np.sum / np.min / np.max on plain lists ===
plain = [10, 20, 30, 40, 50]
assert np.mean(plain) == 30.0, 'np.mean(list)'
assert np.sum(plain) == 150, 'np.sum(list)'
assert np.min(plain) == 10, 'np.min(list)'
assert np.max(plain) == 50, 'np.max(list)'

# Float list
flist = [1.5, 2.5, 3.5]
assert np.mean(flist) == 2.5, 'np.mean(float list)'
assert np.sum(flist) == 7.5, 'np.sum(float list)'

# Single element
assert np.mean([42]) == 42.0, 'np.mean(single)'
assert np.sum([42]) == 42, 'np.sum(single)'

# np.std on list
assert np.std([2, 4, 4, 4, 5, 5, 7, 9]) == 2.0, 'np.std(list)'

# === ndarray attributes ===
arr_attr = np.array([1, 2, 3, 4, 5])
assert arr_attr.shape == (5,), 'ndarray shape 1d'
assert str(arr_attr.dtype) == 'int64', 'ndarray dtype int'

arr_float = np.array([1.0, 2.0, 3.0])
assert str(arr_float.dtype) == 'float64', 'ndarray dtype float'
assert arr_float.shape == (3,), 'ndarray float shape'

# 2D array attributes
arr_2d = np.array([1, 2, 3, 4, 5, 6]).reshape(2, 3)
assert arr_2d.shape == (2, 3), 'ndarray shape 2d'

# === unique ===
dup = np.array([3, 1, 2, 1, 3, 2])
u = np.unique(dup)
assert u.tolist() == [1, 2, 3], 'np.unique'

# === concatenate ===
c1 = np.array([1, 2, 3])
c2 = np.array([4, 5, 6])
cat = np.concatenate([c1, c2])
assert cat.tolist() == [1, 2, 3, 4, 5, 6], 'np.concatenate'

# === np.where ===
cond = np.array([True, False, True, False])
result_w = np.where(cond, 10, 20)
assert result_w.tolist() == [10, 20, 10, 20], 'np.where bool array'

# === np.maximum / np.minimum ===
m1 = np.array([1, 5, 3])
m2 = np.array([4, 2, 6])
assert np.maximum(m1, m2).tolist() == [4, 5, 6], 'np.maximum'
assert np.minimum(m1, m2).tolist() == [1, 2, 3], 'np.minimum'

# === ndarray repr ===
assert repr(np.array([1, 2, 3])) == 'array([1, 2, 3])', 'ndarray int repr'
assert repr(np.array([1.5, 2.5])) == 'array([1.5, 2.5])', 'ndarray float repr'

# === ndarray bool (truthiness) ===
# numpy only allows bool() on single-element arrays
assert bool(np.array([1])) == True, 'single-element truthy'
assert bool(np.array([0])) == False, 'single-element falsy'

# === ndarray len ===
assert len(np.array([1, 2, 3])) == 3, 'ndarray len'

# === ndarray type ===
assert type(np.array([1])).__name__ == 'ndarray', 'ndarray type name'

# === np.where with array x and scalar y ===
cond2 = np.array([True, False, True])
arr_x = np.array([10, 20, 30])
result_w2 = np.where(cond2, arr_x, 0)
assert result_w2.tolist() == [10, 0, 30], 'np.where array x scalar y'

# === np.cumsum on array ===
assert np.cumsum(np.array([1, 2, 3])).tolist() == [1, 3, 6], 'np.cumsum on array'
