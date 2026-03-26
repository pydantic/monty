# skip-cpython
# === Boolean comparisons ===
import numpy as np

a = np.array([1, 2, 3, 4, 5])

# === Greater than ===
mask = a > 3
assert mask.tolist() == [False, False, False, True, True], 'gt mask'

# === Less than ===
mask2 = a < 3
assert mask2.tolist() == [True, True, False, False, False], 'lt mask'

# === Equal ===
mask3 = a == 3
assert mask3.tolist() == [False, False, True, False, False], 'eq mask'

# === Greater than or equal ===
mask4 = a >= 3
assert mask4.tolist() == [False, False, True, True, True], 'gte mask'

# === Less than or equal ===
mask5 = a <= 3
assert mask5.tolist() == [True, True, True, False, False], 'lte mask'

# === Not equal ===
mask6 = a != 3
assert mask6.tolist() == [True, True, False, True, True], 'ne mask'

# === Boolean indexing ===
filtered = a[a > 3]
assert filtered.tolist() == [4, 5], 'boolean indexing'

filtered2 = a[a <= 2]
assert filtered2.tolist() == [1, 2], 'boolean indexing lte'

# === any / all ===
assert (a > 0).all(), 'all positive'
assert not (a > 3).all(), 'not all gt 3'
assert (a > 3).any(), 'any gt 3'
assert not (a > 10).any(), 'none gt 10'

# === Array-to-array comparisons ===
x = np.array([1, 5, 3, 7, 2])
y = np.array([2, 4, 3, 8, 1])

assert (x == y).tolist() == [False, False, True, False, False], 'arr == arr'
assert (x != y).tolist() == [True, True, False, True, True], 'arr != arr'
assert (x > y).tolist() == [False, True, False, False, True], 'arr > arr'
assert (x < y).tolist() == [True, False, False, True, False], 'arr < arr'
assert (x >= y).tolist() == [False, True, True, False, True], 'arr >= arr'
assert (x <= y).tolist() == [True, False, True, True, False], 'arr <= arr'

# === Float comparisons ===
fa = np.array([1.5, 2.5, 3.5])
assert (fa > 2.0).tolist() == [False, True, True], 'float gt scalar'
assert (fa <= 2.5).tolist() == [True, True, False], 'float lte scalar'
assert (fa == 2.5).tolist() == [False, True, False], 'float eq scalar'
assert (fa != 2.5).tolist() == [True, False, True], 'float ne scalar'
