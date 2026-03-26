# skip-cpython
# === NumPy math functions ===
import numpy as np

# === np.abs ===
a = np.array([-1, -2, 3, -4, 5])
result = np.abs(a)
assert result.tolist() == [1, 2, 3, 4, 5], 'np.abs'

# === np.sqrt ===
b = np.array([4, 9, 16, 25])
result = np.sqrt(b)
assert result.tolist() == [2.0, 3.0, 4.0, 5.0], 'np.sqrt'

# === np.log (natural log) ===
c = np.array([1, 10, 100])
log_result = np.log(c)
assert abs(log_result[0] - 0.0) < 0.001, 'np.log(1)'
assert abs(log_result[1] - 2.302585) < 0.001, 'np.log(10)'

# === np.exp ===
d = np.array([0, 1, 2])
exp_result = np.exp(d)
assert abs(exp_result[0] - 1.0) < 0.001, 'np.exp(0)'
assert abs(exp_result[1] - 2.71828) < 0.001, 'np.exp(1)'

# === np.round ===
e = np.array([1.234, 2.567, 3.891])
rounded = np.round(e, 1)
assert rounded.tolist() == [1.2, 2.6, 3.9], 'np.round'

# === np.clip ===
f = np.array([1, 5, 10, 15, 20])
clipped = np.clip(f, 5, 15)
assert clipped.tolist() == [5, 5, 10, 15, 15], 'np.clip'

# === np.where ===
g = np.array([1, 2, 3, 4, 5])
result = np.where(g > 3, g, 0)
assert result.tolist() == [0, 0, 0, 4, 5], 'np.where with arrays'

result2 = np.where(g > 3, 1, 0)
assert result2.tolist() == [0, 0, 0, 1, 1], 'np.where with scalars'

# === np.maximum / np.minimum (element-wise) ===
x = np.array([1, 5, 3])
y = np.array([2, 4, 6])
assert np.maximum(x, y).tolist() == [2, 5, 6], 'np.maximum'
assert np.minimum(x, y).tolist() == [1, 4, 3], 'np.minimum'

# === np.sort ===
unsorted = np.array([3, 1, 4, 1, 5])
assert np.sort(unsorted).tolist() == [1, 1, 3, 4, 5], 'np.sort'

# === np.unique ===
repeated = np.array([3, 1, 2, 1, 3, 2])
assert np.unique(repeated).tolist() == [1, 2, 3], 'np.unique'

# === np.concatenate ===
arr1 = np.array([1, 2, 3])
arr2 = np.array([4, 5, 6])
combined = np.concatenate([arr1, arr2])
assert combined.tolist() == [1, 2, 3, 4, 5, 6], 'np.concatenate'

# === np.cumsum ===
h = np.array([1, 2, 3, 4])
assert np.cumsum(h).tolist() == [1, 3, 6, 10], 'np.cumsum'

# === np.dot ===
a1 = np.array([1, 2, 3])
a2 = np.array([4, 5, 6])
assert np.dot(a1, a2) == 32, 'np.dot'

# === np.ceil / np.floor ===
vals = np.array([1.2, 2.7, 3.5])
assert np.ceil(vals).tolist() == [2.0, 3.0, 4.0], 'np.ceil'
assert np.floor(vals).tolist() == [1.0, 2.0, 3.0], 'np.floor'

# === np.log10 ===
log10_result = np.log10(np.array([1, 10, 100, 1000]))
assert log10_result[0] == 0.0, 'np.log10(1)'
assert log10_result[1] == 1.0, 'np.log10(10)'
assert log10_result[2] == 2.0, 'np.log10(100)'
assert log10_result[3] == 3.0, 'np.log10(1000)'
