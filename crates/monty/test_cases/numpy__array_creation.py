# skip-cpython
# === np.array from list ===
import numpy as np

a = np.array([1, 2, 3])
assert len(a) == 3, 'array length'
assert a[0] == 1, 'first element'
assert a[1] == 2, 'second element'
assert a[2] == 3, 'third element'

# === np.array from nested list (2D) ===
b = np.array([[1, 2], [3, 4]])
assert b[0][0] == 1, '2d first element'
assert b[0][1] == 2, '2d second element'
assert b[1][0] == 3, '2d third element'
assert b[1][1] == 4, '2d fourth element'

# === np.zeros ===
z = np.zeros(3)
assert len(z) == 3, 'zeros length'
assert z[0] == 0.0, 'zeros first'
assert z[1] == 0.0, 'zeros second'
assert z[2] == 0.0, 'zeros third'

# === np.ones ===
o = np.ones(4)
assert len(o) == 4, 'ones length'
assert o[0] == 1.0, 'ones first'
assert o[3] == 1.0, 'ones last'

# === np.arange ===
r = np.arange(5)
assert len(r) == 5, 'arange length'
assert r[0] == 0, 'arange first'
assert r[4] == 4, 'arange last'

r2 = np.arange(2, 7)
assert len(r2) == 5, 'arange start stop length'
assert r2[0] == 2, 'arange start'
assert r2[4] == 6, 'arange last'

r3 = np.arange(0, 10, 2)
assert len(r3) == 5, 'arange step length'
assert r3[0] == 0, 'arange step first'
assert r3[4] == 8, 'arange step last'

# === np.linspace ===
ls = np.linspace(0, 1, 5)
assert len(ls) == 5, 'linspace length'
assert ls[0] == 0.0, 'linspace first'
assert ls[4] == 1.0, 'linspace last'

# === shape attribute ===
a1 = np.array([1, 2, 3])
assert a1.shape == (3,), '1d shape'

a2 = np.array([[1, 2], [3, 4], [5, 6]])
assert a2.shape == (3, 2), '2d shape'

# === dtype attribute ===
int_arr = np.array([1, 2, 3])
assert str(int_arr.dtype) == 'int64', 'int dtype'

float_arr = np.array([1.0, 2.0, 3.0])
assert str(float_arr.dtype) == 'float64', 'float dtype'

# === np.linspace edge cases ===
ls2 = np.linspace(0, 10, 3)
assert ls2.tolist() == [0.0, 5.0, 10.0], 'linspace 3 points'

ls_single = np.linspace(5, 5, 1)
assert ls_single.tolist() == [5.0], 'linspace single point'

# === np.arange with float step ===
r_float = np.arange(0, 1, 0.5)
assert len(r_float) == 2, 'arange float step length'
assert r_float[0] == 0.0, 'arange float step first'
assert r_float[1] == 0.5, 'arange float step second'

# === Float array from mixed list ===
mixed = np.array([1, 2.5, 3])
assert str(mixed.dtype) == 'float64', 'mixed promotes to float'
