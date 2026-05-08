# skip-cpython
# === Element-wise arithmetic ===
import numpy as np

a = np.array([1, 2, 3])
b = np.array([4, 5, 6])

# === Addition ===
c = a + b
assert c[0] == 5, 'add first'
assert c[1] == 7, 'add second'
assert c[2] == 9, 'add third'

# === Subtraction ===
d = b - a
assert d[0] == 3, 'sub first'
assert d[1] == 3, 'sub second'
assert d[2] == 3, 'sub third'

# === Multiplication ===
e = a * b
assert e[0] == 4, 'mul first'
assert e[1] == 10, 'mul second'
assert e[2] == 18, 'mul third'

# === Division ===
f = b / a
assert f[0] == 4.0, 'div first'
assert f[1] == 2.5, 'div second'
assert f[2] == 2.0, 'div third'

# === Scalar operations ===
g = a * 2
assert g[0] == 2, 'scalar mul first'
assert g[1] == 4, 'scalar mul second'
assert g[2] == 6, 'scalar mul third'

h = a + 10
assert h[0] == 11, 'scalar add first'
assert h[1] == 12, 'scalar add second'
assert h[2] == 13, 'scalar add third'

# === Power ===
p = a**2
assert p[0] == 1, 'pow first'
assert p[1] == 4, 'pow second'
assert p[2] == 9, 'pow third'

# === Floor division ===
fd = np.array([7, 8, 9]) // np.array([2, 3, 4])
assert fd[0] == 3, 'floordiv first'
assert fd[1] == 2, 'floordiv second'
assert fd[2] == 2, 'floordiv third'

# === Modulo ===
m = np.array([7, 8, 9]) % np.array([2, 3, 4])
assert m[0] == 1, 'mod first'
assert m[1] == 2, 'mod second'
assert m[2] == 1, 'mod third'

# === Negation ===
neg = -a
assert neg[0] == -1, 'neg first'
assert neg[1] == -2, 'neg second'
assert neg[2] == -3, 'neg third'

# === Scalar subtraction ===
sub_scalar = a - 1
assert sub_scalar[0] == 0, 'scalar sub first'
assert sub_scalar[1] == 1, 'scalar sub second'
assert sub_scalar[2] == 2, 'scalar sub third'

# === In-place addition (+=) ===
iadd = np.array([1, 2, 3])
iadd += np.array([10, 20, 30])
assert iadd.tolist() == [11, 22, 33], 'iadd arrays'
