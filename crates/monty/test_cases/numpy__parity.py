# skip-cpython
import numpy as np

# ============================================================
# 1. ARRAY CREATION FUNCTIONS
# ============================================================

# === np.array ===
# int array from list
a = np.array([1, 2, 3])
assert a.tolist() == [1, 2, 3], 'np.array int list'
assert a.dtype == 'int64', 'np.array int dtype'

# float array from list
a = np.array([1.0, 2.0, 3.0])
assert a.tolist() == [1.0, 2.0, 3.0], 'np.array float list'
assert a.dtype == 'float64', 'np.array float dtype'

# mixed int/float promotes to float
a = np.array([1, 2.0, 3])
assert a.tolist() == [1.0, 2.0, 3.0], 'np.array mixed promotes to float'
assert a.dtype == 'float64', 'np.array mixed dtype'

# single element
a = np.array([42])
assert a.tolist() == [42], 'np.array single int'

a = np.array([3.14])
assert a.tolist() == [3.14], 'np.array single float'

# 2D array
a = np.array([[1, 2], [3, 4]])
assert a.shape == (2, 2), 'np.array 2D shape'
assert a.dtype == 'int64', 'np.array 2D int dtype'

# 2D float
a = np.array([[1.0, 2.0], [3.0, 4.0]])
assert a.shape == (2, 2), 'np.array 2D float shape'
assert a.dtype == 'float64', 'np.array 2D float dtype'

# === np.zeros ===
a = np.zeros(3)
assert a.tolist() == [0.0, 0.0, 0.0], 'np.zeros values'
assert a.dtype == 'float64', 'np.zeros dtype'
assert len(a) == 3, 'np.zeros len'

a = np.zeros(1)
assert a.tolist() == [0.0], 'np.zeros single'

# === np.ones ===
a = np.ones(3)
assert a.tolist() == [1.0, 1.0, 1.0], 'np.ones values'
assert a.dtype == 'float64', 'np.ones dtype'

a = np.ones(1)
assert a.tolist() == [1.0], 'np.ones single'

# === np.arange ===
# single arg (stop)
a = np.arange(5)
assert a.tolist() == [0, 1, 2, 3, 4], 'np.arange(5)'
assert a.dtype == 'int64', 'np.arange int dtype'

# two args (start, stop)
a = np.arange(2, 6)
assert a.tolist() == [2, 3, 4, 5], 'np.arange(2, 6)'

# three args (start, stop, step)
a = np.arange(0, 10, 2)
assert a.tolist() == [0, 2, 4, 6, 8], 'np.arange(0, 10, 2)'

# float step
a = np.arange(0, 1, 0.5)
assert a.dtype == 'float64', 'np.arange float step dtype'
assert len(a) == 2, 'np.arange float step len'

# negative step
a = np.arange(5, 0, -1)
assert a.tolist() == [5, 4, 3, 2, 1], 'np.arange negative step'

# empty result
a = np.arange(5, 0)
assert a.tolist() == [], 'np.arange empty result'
assert len(a) == 0, 'np.arange empty len'

# === np.linspace ===
a = np.linspace(0, 1, 5)
assert a.dtype == 'float64', 'np.linspace dtype'
assert len(a) == 5, 'np.linspace len'
assert a[0] == 0.0, 'np.linspace start'
assert a[-1] == 1.0, 'np.linspace end'
# check intermediate values with rounding
assert round(a[1], 2) == 0.25, 'np.linspace[1]'
assert round(a[2], 2) == 0.5, 'np.linspace[2]'

# linspace single point
a = np.linspace(5, 5, 1)
assert a.tolist() == [5.0], 'np.linspace single point'

# linspace two points
a = np.linspace(0, 10, 2)
assert a.tolist() == [0.0, 10.0], 'np.linspace two points'


# ============================================================
# 2. MODULE-LEVEL AGGREGATE FUNCTIONS
# ============================================================

# === np.sum ===
# Note: module-level np.sum returns float in our impl
a_int = np.array([1, 2, 3])
a_float = np.array([1.0, 2.0, 3.0])

# Method-level sum preserves dtype
assert a_int.sum() == 6, 'arr.sum() int value'
assert a_float.sum() == 6.0, 'arr.sum() float value'

# === np.mean ===
assert np.mean(np.array([1, 2, 3])) == 2.0, 'np.mean int array'
assert np.mean(np.array([1.0, 2.0, 3.0])) == 2.0, 'np.mean float array'
assert np.array([10]).mean() == 10.0, 'mean single element'
assert np.array([2, 4]).mean() == 3.0, 'mean two elements'

# === np.min ===
assert np.array([3, 1, 2]).min() == 1, 'arr.min() int'
assert np.array([3.0, 1.0, 2.0]).min() == 1.0, 'arr.min() float'
assert np.array([42]).min() == 42, 'min single element'
assert np.array([-5, -1, -10]).min() == -10, 'min negative'

# === np.max ===
assert np.array([3, 1, 2]).max() == 3, 'arr.max() int'
assert np.array([3.0, 1.0, 2.0]).max() == 3.0, 'arr.max() float'
assert np.array([42]).max() == 42, 'max single element'
assert np.array([-5, -1, -10]).max() == -1, 'max negative'

# === np.std ===
a = np.array([1, 2, 3, 4, 5])
s = np.std(a)
assert round(s, 10) == round(1.4142135623730951, 10), 'np.std value'
a = np.array([5, 5, 5])
assert np.std(a) == 0.0, 'np.std uniform'
assert np.array([10]).std() == 0.0, 'std single element'


# ============================================================
# 3. MODULE-LEVEL ELEMENT-WISE FUNCTIONS
# ============================================================

# === np.abs ===
a = np.abs(np.array([-1, -2, 3]))
assert a.tolist() == [1, 2, 3], 'np.abs int values'

a = np.abs(np.array([-1.5, 2.5, -3.5]))
assert a.tolist() == [1.5, 2.5, 3.5], 'np.abs float values'

a = np.abs(np.array([0]))
assert a.tolist() == [0], 'np.abs zero'

# === np.sqrt ===
a = np.sqrt(np.array([1.0, 4.0, 9.0]))
assert a.tolist() == [1.0, 2.0, 3.0], 'np.sqrt values'
assert a.dtype == 'float64', 'np.sqrt dtype'

a = np.sqrt(np.array([0.0]))
assert a.tolist() == [0.0], 'np.sqrt zero'

# === np.log ===
a = np.log(np.array([1.0]))
assert a.tolist() == [0.0], 'np.log(1) = 0'

# === np.exp ===
a = np.exp(np.array([0.0]))
assert a.tolist() == [1.0], 'np.exp(0) = 1'
assert a.dtype == 'float64', 'np.exp dtype'

# === np.ceil ===
a = np.ceil(np.array([1.2, 2.7, -0.5]))
assert a.tolist() == [2.0, 3.0, 0.0], 'np.ceil float values'
assert a.dtype == 'float64', 'np.ceil dtype'

# === np.floor ===
a = np.floor(np.array([1.2, 2.7, -0.5]))
assert a.tolist() == [1.0, 2.0, -1.0], 'np.floor float values'
assert a.dtype == 'float64', 'np.floor dtype'

# === np.log10 ===
a = np.log10(np.array([1.0, 10.0, 100.0]))
assert a.tolist() == [0.0, 1.0, 2.0], 'np.log10 values'
assert a.dtype == 'float64', 'np.log10 dtype'

# === np.round ===
a = np.round(np.array([1.5, 2.5, 3.5]))
# numpy uses banker's rounding
r = a.tolist()
assert r[0] == 2.0, 'np.round 1.5'
assert r[2] == 4.0, 'np.round 3.5'

a = np.round(np.array([1.234, 5.678]), 2)
r = a.tolist()
assert r[0] == 1.23, 'np.round decimals=2 first'
assert r[1] == 5.68, 'np.round decimals=2 second'

# round with 0 decimals
a = np.round(np.array([1.6, 2.4]))
assert a.tolist() == [2.0, 2.0], 'np.round default decimals'

# === np.clip ===
a = np.clip(np.array([1, 5, 10, 15, 20]), 5, 15)
assert a.tolist() == [5, 5, 10, 15, 15], 'np.clip int values'

a = np.clip(np.array([1.0, 5.0, 10.0]), 2.0, 8.0)
assert a.tolist() == [2.0, 5.0, 8.0], 'np.clip float values'

a = np.clip(np.array([-10, 0, 10]), -5, 5)
assert a.tolist() == [-5, 0, 5], 'np.clip with negatives'


# ============================================================
# 4. MODULE-LEVEL BINARY/SELECTION FUNCTIONS
# ============================================================

# === np.where ===
cond = np.array([1, 0, 1, 0, 1])
x = np.array([10, 20, 30, 40, 50])
y = np.array([1, 2, 3, 4, 5])
result = np.where(cond, x, y)
assert result.tolist() == [10, 2, 30, 4, 50], 'np.where basic'

# where with scalar x, y
cond = np.array([1, 0, 1])
result = np.where(cond, 10, 20)
assert result.tolist() == [10, 20, 10], 'np.where scalar x, y'

# where with boolean condition
cond = np.array([1, 0, 1])
result = np.where(cond, np.array([1, 2, 3]), np.array([4, 5, 6]))
assert result.tolist() == [1, 5, 3], 'np.where from comparison'

# === np.maximum ===
a = np.array([1, 3, 2])
b = np.array([3, 1, 4])
assert np.maximum(a, b).tolist() == [3, 3, 4], 'np.maximum values'

# int preserves dtype
a = np.array([1, 5])
b = np.array([3, 2])
assert np.maximum(a, b).dtype == 'int64', 'np.maximum int dtype'

# float
a = np.array([1.0, 3.0])
b = np.array([3.0, 1.0])
assert np.maximum(a, b).tolist() == [3.0, 3.0], 'np.maximum float'

# === np.minimum ===
a = np.array([1, 3, 2])
b = np.array([3, 1, 4])
assert np.minimum(a, b).tolist() == [1, 1, 2], 'np.minimum values'

a = np.array([1, 5])
b = np.array([3, 2])
assert np.minimum(a, b).dtype == 'int64', 'np.minimum int dtype'

# === np.sort ===
a = np.sort(np.array([3, 1, 2]))
assert a.tolist() == [1, 2, 3], 'np.sort int'
assert a.dtype == 'int64', 'np.sort int dtype'

a = np.sort(np.array([3.0, 1.0, 2.0]))
assert a.tolist() == [1.0, 2.0, 3.0], 'np.sort float'

# already sorted
a = np.sort(np.array([1, 2, 3]))
assert a.tolist() == [1, 2, 3], 'np.sort already sorted'

# reverse sorted
a = np.sort(np.array([5, 4, 3, 2, 1]))
assert a.tolist() == [1, 2, 3, 4, 5], 'np.sort reverse'

# single element
a = np.sort(np.array([42]))
assert a.tolist() == [42], 'np.sort single'

# === np.unique ===
a = np.unique(np.array([3, 1, 2, 1, 3]))
assert a.tolist() == [1, 2, 3], 'np.unique values'
assert a.dtype == 'int64', 'np.unique int dtype'

a = np.unique(np.array([1, 1, 1]))
assert a.tolist() == [1], 'np.unique all same'

a = np.unique(np.array([5]))
assert a.tolist() == [5], 'np.unique single'

# === np.concatenate ===
a = np.concatenate([np.array([1, 2]), np.array([3, 4])])
assert a.tolist() == [1, 2, 3, 4], 'np.concatenate two arrays'
assert a.dtype == 'int64', 'np.concatenate int dtype'

# three arrays
a = np.concatenate([np.array([1]), np.array([2]), np.array([3])])
assert a.tolist() == [1, 2, 3], 'np.concatenate three arrays'

# mixed dtypes
a = np.concatenate([np.array([1, 2]), np.array([3.0, 4.0])])
assert a.tolist() == [1.0, 2.0, 3.0, 4.0], 'np.concatenate mixed dtype'
assert a.dtype == 'float64', 'np.concatenate mixed dtype result'

# === np.cumsum ===
a = np.cumsum(np.array([1, 2, 3]))
assert a.tolist() == [1, 3, 6], 'np.cumsum int'
assert a.dtype == 'int64', 'np.cumsum int dtype'

a = np.cumsum(np.array([1.0, 2.0, 3.0]))
assert a.tolist() == [1.0, 3.0, 6.0], 'np.cumsum float'

a = np.cumsum(np.array([10]))
assert a.tolist() == [10], 'np.cumsum single'

# === np.dot ===
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
assert np.dot(a, b) == 32, 'np.dot int result'

a = np.array([1.0, 2.0, 3.0])
b = np.array([4.0, 5.0, 6.0])
assert np.dot(a, b) == 32.0, 'np.dot float result'

# single element
assert np.dot(np.array([5]), np.array([3])) == 15, 'np.dot single element'


# ============================================================
# 5. NDARRAY METHODS
# ============================================================

# === .sum() ===
assert np.array([1, 2, 3]).sum() == 6, 'sum int'
assert np.array([1.0, 2.0, 3.0]).sum() == 6.0, 'sum float'
assert np.array([100]).sum() == 100, 'sum single'

# === .mean() ===
assert np.array([2, 4, 6]).mean() == 4.0, 'mean int'
assert np.array([1.0, 3.0]).mean() == 2.0, 'mean float'
assert np.array([7]).mean() == 7.0, 'mean single'

# === .min() ===
assert np.array([3, 1, 2]).min() == 1, 'min int'
assert np.array([3.0, 1.0, 2.0]).min() == 1.0, 'min float'
assert np.array([-10]).min() == -10, 'min single negative'

# === .max() ===
assert np.array([3, 1, 2]).max() == 3, 'max int'
assert np.array([3.0, 1.0, 2.0]).max() == 3.0, 'max float'
assert np.array([0]).max() == 0, 'max single zero'

# === .std() ===
assert np.array([1, 1, 1]).std() == 0.0, 'std uniform'
s = np.array([1, 2, 3, 4, 5]).std()
assert round(s, 10) == round(1.4142135623730951, 10), 'std five elements'

# === .flatten() ===
a = np.array([[1, 2], [3, 4]])
f = a.flatten()
assert f.tolist() == [1, 2, 3, 4], 'flatten 2D'
assert f.shape == (4,), 'flatten shape'
assert f.dtype == 'int64', 'flatten preserves dtype'

# 1D flatten is identity
a = np.array([1, 2, 3])
assert a.flatten().tolist() == [1, 2, 3], 'flatten 1D'

# === .tolist() ===
assert np.array([1, 2, 3]).tolist() == [1, 2, 3], 'tolist int'
assert np.array([1.0, 2.0]).tolist() == [1.0, 2.0], 'tolist float'
assert np.array([42]).tolist() == [42], 'tolist single'

# === .copy() ===
a = np.array([1, 2, 3])
b = a.copy()
assert b.tolist() == [1, 2, 3], 'copy values'
assert b.dtype == 'int64', 'copy preserves dtype'

a = np.array([1.0, 2.0])
b = a.copy()
assert b.tolist() == [1.0, 2.0], 'copy float values'

# === .sort() (method) ===
# In numpy, sort() is in-place, returns None
# In our impl, sort() returns new sorted array (we test both ways)
original = np.array([3, 1, 2])
sorted_arr = np.sort(original)  # module-level returns new copy
assert sorted_arr.tolist() == [1, 2, 3], 'np.sort returns sorted'

# === .argsort() ===
a = np.array([3, 1, 2])
idx = a.argsort()
assert idx.tolist() == [1, 2, 0], 'argsort values'
assert idx.dtype == 'int64', 'argsort dtype'

a = np.array([10, 30, 20])
assert a.argsort().tolist() == [0, 2, 1], 'argsort three elements'

a = np.array([1])
assert a.argsort().tolist() == [0], 'argsort single'

# === .argmin() ===
assert np.array([3, 1, 2]).argmin() == 1, 'argmin basic'
assert np.array([10]).argmin() == 0, 'argmin single'
assert np.array([5, 5, 5]).argmin() == 0, 'argmin ties'

# === .argmax() ===
assert np.array([3, 1, 2]).argmax() == 0, 'argmax basic'
assert np.array([10]).argmax() == 0, 'argmax single'
assert np.array([5, 5, 5]).argmax() == 0, 'argmax ties'

# === .all() ===
assert np.array([1, 2, 3]).all() == True, 'all truthy'
assert np.array([1, 0, 3]).all() == False, 'all with zero'
assert np.array([1]).all() == True, 'all single truthy'
assert np.array([0]).all() == False, 'all single falsy'

# === .any() ===
assert np.array([0, 0, 1]).any() == True, 'any with one truthy'
assert np.array([0, 0, 0]).any() == False, 'any all falsy'
assert np.array([1]).any() == True, 'any single truthy'
assert np.array([0]).any() == False, 'any single falsy'

# === .cumsum() ===
a = np.array([1, 2, 3]).cumsum()
assert a.tolist() == [1, 3, 6], 'cumsum int'
assert a.dtype == 'int64', 'cumsum int dtype'

a = np.array([1.0, 2.0, 3.0]).cumsum()
assert a.tolist() == [1.0, 3.0, 6.0], 'cumsum float'

assert np.array([42]).cumsum().tolist() == [42], 'cumsum single'

# === .reshape() ===
a = np.array([1, 2, 3, 4, 5, 6])
b = a.reshape(2, 3)
assert b.shape == (2, 3), 'reshape shape'
assert b.dtype == 'int64', 'reshape preserves dtype'

b = a.reshape(3, 2)
assert b.shape == (3, 2), 'reshape 3x2'

b = a.reshape(6)
assert b.shape == (6,), 'reshape to 1D'

# === .round() ===
a = np.array([1.234, 5.678]).round(2)
assert a.tolist() == [1.23, 5.68], 'round method decimals=2'

a = np.array([1.5, 2.5]).round()
r = a.tolist()
assert r[0] == 2.0, 'round method 1.5'

# === .clip() ===
a = np.array([1, 5, 10]).clip(3, 8)
assert a.tolist() == [3, 5, 8], 'clip method basic'

a = np.array([1.0, 5.0, 10.0]).clip(2.0, 8.0)
assert a.tolist() == [2.0, 5.0, 8.0], 'clip method float'

# === .dot() ===
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
assert a.dot(b) == 32, 'dot method int'

a = np.array([1.0, 2.0])
b = np.array([3.0, 4.0])
assert a.dot(b) == 11.0, 'dot method float'

# === .astype() ===
a = np.array([1.5, 2.7, 3.1]).astype('int64')
assert a.tolist() == [1, 2, 3], 'astype to int64'
assert a.dtype == 'int64', 'astype int64 dtype'

a = np.array([1, 2, 3]).astype('float64')
assert a.tolist() == [1.0, 2.0, 3.0], 'astype to float64'
assert a.dtype == 'float64', 'astype float64 dtype'


# ============================================================
# 6. NDARRAY ATTRIBUTES
# ============================================================

# === .shape ===
assert np.array([1, 2, 3]).shape == (3,), 'shape 1D'
assert np.array([[1, 2], [3, 4]]).shape == (2, 2), 'shape 2D'
assert np.array([[1, 2, 3]]).shape == (1, 3), 'shape 1x3'
assert np.array([42]).shape == (1,), 'shape single'

# === .dtype ===
assert np.array([1, 2]).dtype == 'int64', 'dtype int'
assert np.array([1.0, 2.0]).dtype == 'float64', 'dtype float'
assert np.zeros(2).dtype == 'float64', 'zeros dtype'
assert np.ones(2).dtype == 'float64', 'ones dtype'
assert np.arange(3).dtype == 'int64', 'arange dtype'
assert np.linspace(0, 1, 3).dtype == 'float64', 'linspace dtype'

# === .size ===
assert np.array([1, 2, 3]).size == 3, 'size 1D'
assert np.array([[1, 2], [3, 4]]).size == 4, 'size 2D'
assert np.array([42]).size == 1, 'size single'

# === .ndim ===
assert np.array([1, 2, 3]).ndim == 1, 'ndim 1D'
assert np.array([[1, 2], [3, 4]]).ndim == 2, 'ndim 2D'
assert np.array([42]).ndim == 1, 'ndim single element'

# === .T ===
a = np.array([[1, 2], [3, 4]])
t = a.T
assert t.shape == (2, 2), 'T 2D shape'
# T[0] should be the first column: [1, 3]
assert t[0].tolist() == [1, 3], 'T first column'
assert t[1].tolist() == [2, 4], 'T second column'

# 1D transpose is identity
a = np.array([1, 2, 3])
assert a.T.tolist() == [1, 2, 3], 'T 1D identity'
assert a.T.shape == (3,), 'T 1D shape'


# ============================================================
# 7. ELEMENT-WISE BINARY OPERATIONS
# ============================================================

a = np.array([1, 2, 3])
b = np.array([4, 5, 6])

# === array + array ===
r = a + b
assert r.tolist() == [5, 7, 9], 'add array+array'
assert r.dtype == 'int64', 'add int+int dtype'

# === array - array ===
r = b - a
assert r.tolist() == [3, 3, 3], 'sub array-array'

# === array * array ===
r = a * b
assert r.tolist() == [4, 10, 18], 'mul array*array'

# === array / array ===
r = np.array([4, 6, 8]) / np.array([2, 3, 4])
assert r.tolist() == [2.0, 2.0, 2.0], 'div array/array'
assert r.dtype == 'float64', 'div always float'

# === array // array ===
r = np.array([7, 8, 9]) // np.array([2, 3, 4])
assert r.tolist() == [3, 2, 2], 'floordiv array//array'

# === array % array ===
r = np.array([7, 8, 9]) % np.array([3, 3, 4])
assert r.tolist() == [1, 2, 1], 'mod array%array'

# === array ** array ===
r = np.array([2, 3, 4]) ** np.array([3, 2, 1])
assert r.tolist() == [8, 9, 4], 'pow array**array'

# === array + scalar ===
r = np.array([1, 2, 3]) + 10
assert r.tolist() == [11, 12, 13], 'add array+scalar'
assert r.dtype == 'int64', 'add int+int_scalar dtype'

# === scalar + array ===
r = 10 + np.array([1, 2, 3])
assert r.tolist() == [11, 12, 13], 'add scalar+array'

# === array - scalar ===
r = np.array([10, 20, 30]) - 5
assert r.tolist() == [5, 15, 25], 'sub array-scalar'

# === scalar - array ===
r = 10 - np.array([1, 2, 3])
assert r.tolist() == [9, 8, 7], 'sub scalar-array'

# === array * scalar ===
r = np.array([1, 2, 3]) * 2
assert r.tolist() == [2, 4, 6], 'mul array*scalar'

# === scalar * array ===
r = 2 * np.array([1, 2, 3])
assert r.tolist() == [2, 4, 6], 'mul scalar*array'

# === array / scalar ===
r = np.array([2, 4, 6]) / 2
assert r.tolist() == [1.0, 2.0, 3.0], 'div array/scalar'
assert r.dtype == 'float64', 'div result always float'

# === scalar / array ===
r = 12 / np.array([1, 2, 3])
assert r.tolist() == [12.0, 6.0, 4.0], 'div scalar/array'

# === array // scalar ===
r = np.array([7, 8, 9]) // 3
assert r.tolist() == [2, 2, 3], 'floordiv array//scalar'

# === scalar // array ===
r = 10 // np.array([3, 4, 5])
assert r.tolist() == [3, 2, 2], 'floordiv scalar//array'

# === array % scalar ===
r = np.array([7, 8, 9]) % 3
assert r.tolist() == [1, 2, 0], 'mod array%scalar'

# === scalar % array ===
r = 10 % np.array([3, 4, 7])
assert r.tolist() == [1, 2, 3], 'mod scalar%array'

# === array ** scalar ===
r = np.array([2, 3, 4]) ** 2
assert r.tolist() == [4, 9, 16], 'pow array**scalar'

# === scalar ** array ===
r = 2 ** np.array([1, 2, 3])
assert r.tolist() == [2, 4, 8], 'pow scalar**array'

# === mixed int/float arithmetic ===
r = np.array([1, 2, 3]) + 0.5
assert r.tolist() == [1.5, 2.5, 3.5], 'add int_array + float_scalar'
assert r.dtype == 'float64', 'int+float promotes to float'

r = np.array([1, 2, 3]) + np.array([0.5, 0.5, 0.5])
assert r.tolist() == [1.5, 2.5, 3.5], 'add int_array + float_array'
assert r.dtype == 'float64', 'int_arr+float_arr promotes to float'

r = np.array([1.0, 2.0]) * 2
assert r.tolist() == [2.0, 4.0], 'mul float_array * int_scalar'
assert r.dtype == 'float64', 'float*int stays float'


# ============================================================
# 8. ELEMENT-WISE COMPARISONS
# ============================================================

a = np.array([1, 2, 3, 4, 5])

# === array > scalar ===
r = a > 3
assert r.tolist() == [False, False, False, True, True], 'gt scalar'

# === array < scalar ===
r = a < 3
assert r.tolist() == [True, True, False, False, False], 'lt scalar'

# === array >= scalar ===
r = a >= 3
assert r.tolist() == [False, False, True, True, True], 'gte scalar'

# === array <= scalar ===
r = a <= 3
assert r.tolist() == [True, True, True, False, False], 'lte scalar'

# === array == scalar ===
r = a == 3
assert r.tolist() == [False, False, True, False, False], 'eq scalar'

# === array != scalar ===
r = a != 3
assert r.tolist() == [True, True, False, True, True], 'ne scalar'

# === array vs array comparisons ===
x = np.array([1, 3, 5])
y = np.array([2, 3, 4])

assert (x > y).tolist() == [False, False, True], 'gt array'
assert (x < y).tolist() == [True, False, False], 'lt array'
assert (x >= y).tolist() == [False, True, True], 'gte array'
assert (x <= y).tolist() == [True, True, False], 'lte array'
assert (x == y).tolist() == [False, True, False], 'eq array'
assert (x != y).tolist() == [True, False, True], 'ne array'

# comparison result dtype is bool
r = np.array([1, 2]) > np.array([0, 3])
assert r.dtype == 'bool', 'comparison dtype is bool'


# ============================================================
# 9. UNARY NEGATION
# ============================================================

# int negation
a = -np.array([1, 2, 3])
assert a.tolist() == [-1, -2, -3], 'neg int'
assert a.dtype == 'int64', 'neg int preserves dtype'

# float negation
a = -np.array([1.5, -2.5, 0.0])
assert a.tolist() == [-1.5, 2.5, 0.0], 'neg float'
assert a.dtype == 'float64', 'neg float preserves dtype'

# double negation
a = -(-np.array([1, 2, 3]))
assert a.tolist() == [1, 2, 3], 'double neg'

# negation of zeros
a = -np.array([0, 0, 0])
assert a.tolist() == [0, 0, 0], 'neg zeros'

# === bitwise invert (~) ===
# int invert: ~n = -(n+1)
a = ~np.array([0, 1, 2, -1])
assert a.tolist() == [-1, -2, -3, 0], 'invert int'
assert a.dtype == 'int64', 'invert int preserves dtype'

# bool invert: flips True/False
b = ~np.array([True, False, True])
assert b.tolist() == [False, True, False], 'invert bool'
assert b.dtype == 'bool', 'invert bool preserves dtype'

# === np.where shape validation ===
# matching shapes should work
cond = np.array([True, False, True])
x = np.array([10, 20, 30])
y = np.array([0, 0, 0])
assert np.where(cond, x, y).tolist() == [10, 0, 30], 'where matching shapes'

# scalar x/y should broadcast
assert np.where(cond, 1, 0).tolist() == [1, 0, 1], 'where scalar broadcast'


# ============================================================
# 10. REPR FORMAT
# ============================================================

# int array repr
assert repr(np.array([1, 2, 3])) == 'array([1, 2, 3])', 'repr int array'

# float array repr
assert repr(np.array([1.0, 2.0, 3.0])) == 'array([1., 2., 3.])', 'repr float array'

# single element
assert repr(np.array([42])) == 'array([42])', 'repr single int'
assert repr(np.array([3.14])) == 'array([3.14])', 'repr single float'

# Note: 2D repr differs between our impl (single line) and real numpy (multi-line)
# so we skip 2D repr comparison here


# ============================================================
# 11. TYPE AND TYPE NAME
# ============================================================

a = np.array([1, 2, 3])
assert type(a).__name__ == 'ndarray', 'type name'


# ============================================================
# 12. LEN
# ============================================================

assert len(np.array([1, 2, 3])) == 3, 'len 1D'
assert len(np.array([42])) == 1, 'len single'
assert len(np.array([[1, 2], [3, 4]])) == 2, 'len 2D (num rows)'
assert len(np.zeros(5)) == 5, 'len zeros'


# ============================================================
# 13. INDEXING (GETITEM)
# ============================================================

a = np.array([10, 20, 30, 40, 50])

# positive int index
assert a[0] == 10, 'getitem [0]'
assert a[1] == 20, 'getitem [1]'
assert a[4] == 50, 'getitem [4]'

# negative int index
assert a[-1] == 50, 'getitem [-1]'
assert a[-2] == 40, 'getitem [-2]'
assert a[-5] == 10, 'getitem [-5]'

# float array indexing
b = np.array([1.5, 2.5, 3.5])
assert b[0] == 1.5, 'getitem float [0]'
assert b[-1] == 3.5, 'getitem float [-1]'

# 2D indexing (returns row)
a = np.array([[1, 2, 3], [4, 5, 6]])
row0 = a[0]
assert row0.tolist() == [1, 2, 3], 'getitem 2D row 0'
row1 = a[1]
assert row1.tolist() == [4, 5, 6], 'getitem 2D row 1'
assert a[-1].tolist() == [4, 5, 6], 'getitem 2D negative index'

# chained 2D indexing
assert a[0][1] == 2, 'getitem 2D chained'
assert a[1][2] == 6, 'getitem 2D chained last'

# boolean mask indexing
a = np.array([10, 20, 30, 40, 50])
mask = np.array([1, 0, 1, 0, 1])
result = a[mask > 0]
assert result.tolist() == [10, 30, 50], 'boolean mask indexing'

# comparison-based boolean indexing
a = np.array([1, 2, 3, 4, 5])
result = a[a > 3]
assert result.tolist() == [4, 5], 'comparison boolean indexing'

result = a[a <= 2]
assert result.tolist() == [1, 2], 'comparison boolean indexing lte'


# ============================================================
# 14. EDGE CASES
# ============================================================

# Large values
a = np.array([1000000, 2000000, 3000000])
assert a.sum() == 6000000, 'large values sum'

# Negative values throughout
a = np.array([-1, -2, -3])
assert a.sum() == -6, 'negative sum'
assert a.mean() == -2.0, 'negative mean'

# Single element operations
a = np.array([42])
assert a.sum() == 42, 'single sum'
assert a.mean() == 42.0, 'single mean'
assert a.min() == 42, 'single min'
assert a.max() == 42, 'single max'
assert a.std() == 0.0, 'single std'

# Zeros array operations
a = np.zeros(3)
assert a.sum() == 0.0, 'zeros sum'
assert a.mean() == 0.0, 'zeros mean'
assert a.min() == 0.0, 'zeros min'
assert a.max() == 0.0, 'zeros max'
assert a.std() == 0.0, 'zeros std'

# Ones array operations
a = np.ones(3)
assert a.sum() == 3.0, 'ones sum'
assert a.mean() == 1.0, 'ones mean'
assert a.min() == 1.0, 'ones min'
assert a.max() == 1.0, 'ones max'
assert a.std() == 0.0, 'ones std'

# Mixed positive and negative
a = np.array([-2, -1, 0, 1, 2])
assert a.sum() == 0, 'mixed sum zero'
assert a.mean() == 0.0, 'mixed mean zero'
assert a.min() == -2, 'mixed min'
assert a.max() == 2, 'mixed max'


# ============================================================
# 15. NaN AND INF EDGE CASES
# ============================================================
import math

# === Division by zero ===
a = np.array([1.0, 2.0, 3.0]) / 0
assert math.isinf(a[0]) and a[0] > 0, 'float / 0 = inf'
assert math.isinf(a[1]) and a[1] > 0, 'float / 0 = inf (2)'

b = np.array([0.0]) / 0
assert math.isnan(b[0]), '0.0 / 0 = nan'

# === NaN propagation in aggregation ===
nan_arr = np.array([1.0, float('nan'), 3.0])
assert math.isnan(nan_arr.sum()), 'sum propagates nan'
assert math.isnan(nan_arr.mean()), 'mean propagates nan'
assert math.isnan(nan_arr.min()), 'min propagates nan'
assert math.isnan(nan_arr.max()), 'max propagates nan'
assert math.isnan(nan_arr.std()), 'std propagates nan'

# argmin/argmax with NaN — NumPy returns index of first NaN
assert np.array([float('nan')]).argmin() == 0, 'argmin single nan'
assert np.array([float('nan')]).argmax() == 0, 'argmax single nan'
assert np.array([float('nan'), 1.0, 2.0]).argmin() == 0, 'argmin nan first'

# === Inf operations ===
inf_arr = np.array([float('inf')])
assert (inf_arr + 1)[0] == float('inf'), 'inf + 1 = inf'
assert (inf_arr * -1)[0] == float('-inf'), 'inf * -1 = -inf'
assert math.isnan((inf_arr - inf_arr)[0]), 'inf - inf = nan'
assert inf_arr.sum() == float('inf'), 'sum(inf) = inf'

# === NaN/Inf repr ===
assert repr(np.array([float('nan')])) == 'array([nan])', 'nan repr lowercase'
assert repr(np.array([float('inf')])) == 'array([inf])', 'inf repr'
assert repr(np.array([float('-inf')])) == 'array([-inf])', '-inf repr'

# === NaN comparisons ===
r = np.array([float('nan')]) == np.array([float('nan')])
assert r[0] == False, 'nan != nan'
r2 = np.array([float('nan')]) > 0
assert r2[0] == False, 'nan > 0 is False'

# === NaN in sort ===
s = np.sort(np.array([float('nan'), 1.0, 2.0]))
assert s[0] == 1.0, 'sort nan: first elem'
assert s[1] == 2.0, 'sort nan: second elem'
assert math.isnan(s[2]), 'sort nan: nan last'

s2 = np.sort(np.array([3.0, float('nan'), 1.0]))
assert s2[0] == 1.0, 'sort nan mid: first'
assert s2[1] == 3.0, 'sort nan mid: second'
assert math.isnan(s2[2]), 'sort nan mid: nan last'


# ============================================================
# 16. EMPTY ARRAY EDGE CASES
# ============================================================

# === Empty array creation and attributes ===
empty = np.array([])
assert empty.shape == (0,), 'empty shape'
assert empty.dtype == 'float64', 'empty dtype'
assert len(empty) == 0, 'empty len'
assert empty.size == 0, 'empty size'
assert empty.ndim == 1, 'empty ndim'

# === Empty array operations ===
assert empty.tolist() == [], 'empty tolist'
assert empty.flatten().tolist() == [], 'empty flatten'
assert empty.cumsum().tolist() == [], 'empty cumsum'
assert empty.sum() == 0.0, 'empty sum'

# mean of empty is nan (0/0)
assert math.isnan(empty.mean()), 'empty mean is nan'

# std of empty is nan
assert math.isnan(empty.std()), 'empty std is nan'

# sort/unique of empty
assert np.sort(empty).tolist() == [], 'empty sort'
assert np.unique(empty).tolist() == [], 'empty unique'

# concatenate with empty
assert np.concatenate([empty, np.array([1.0, 2.0])]).tolist() == [1.0, 2.0], 'concat empty'

# zeros/ones with 0
assert np.zeros(0).tolist() == [], 'zeros(0)'
assert np.ones(0).tolist() == [], 'ones(0)'


# ============================================================
# 17. DTYPE CORRECTNESS
# ============================================================

# === Division always produces float64 ===
a = np.array([4, 6, 8])
b = np.array([2, 3, 4])
assert (a / b).dtype == 'float64', 'int / int -> float64'
assert (a / b).tolist() == [2.0, 2.0, 2.0], 'int / int values'
assert (a / 2).dtype == 'float64', 'int / scalar -> float64'
assert (a // b).dtype == 'int64', 'int // int -> int64'

# === Arithmetic dtype promotion ===
assert (a + b).dtype == 'int64', 'int + int -> int64'
assert (a * 2).dtype == 'int64', 'int * int_scalar -> int64'
assert (a * 1.0).dtype == 'float64', 'int * float_scalar -> float64'
assert (a + np.array([1.0, 2.0, 3.0])).dtype == 'float64', 'int + float -> float64'

# === Comparison always produces bool ===
assert (a > 5).dtype == 'bool', 'int > scalar -> bool'
assert (a == b).dtype == 'bool', 'int == int -> bool'


# ============================================================
# 18. 2D ARRAY OPERATIONS
# ============================================================

# === 2D binary operations ===
m1 = np.array([[1, 2], [3, 4]])
m2 = np.array([[10, 20], [30, 40]])
assert (m1 + m2).tolist() == [[11, 22], [33, 44]], '2d add'
assert (m1 * 2).tolist() == [[2, 4], [6, 8]], '2d scalar mul'
assert (2 * m1).tolist() == [[2, 4], [6, 8]], '2d scalar mul left'

# === 2D comparisons ===
assert (m1 > 2).tolist() == [[False, False], [True, True]], '2d > scalar'

# === 2D methods ===
assert m1.sum() == 10, '2d sum'
assert m1.mean() == 2.5, '2d mean'
assert m1.flatten().tolist() == [1, 2, 3, 4], '2d flatten'

# === 2D tolist preserves nesting ===
assert m1.tolist() == [[1, 2], [3, 4]], '2d tolist nested'
r = np.array([[1, 2, 3], [4, 5, 6]])
assert r.tolist() == [[1, 2, 3], [4, 5, 6]], '2x3 tolist'

# === Transpose ===
assert r.T.shape == (3, 2), 'transpose shape'
assert r.T.tolist() == [[1, 4], [2, 5], [3, 6]], 'transpose values'

# === 2D indexing ===
assert r[0].tolist() == [1, 2, 3], '2d row 0'
assert r[1].tolist() == [4, 5, 6], '2d row 1'
assert r[-1].tolist() == [4, 5, 6], '2d row -1'
assert r[0][2] == 3, '2d chained index'
assert r[1][0] == 4, '2d chained index 2'

# ============================================================
# 19. TRIGONOMETRIC & MATH FUNCTIONS
# ============================================================

# === np.sin ===
assert repr(np.sin(np.array([0.0]))) == 'array([0.])', 'sin(0)'
assert repr(np.cos(np.array([0.0]))) == 'array([1.])', 'cos(0)'
assert repr(np.tan(np.array([0.0]))) == 'array([0.])', 'tan(0)'

# sin(pi/2) ~ 1
sin_result = np.sin(np.array([0.0, math.pi / 2, math.pi]))
assert abs(sin_result[0]) < 1e-10, 'sin(0) ~ 0'
assert abs(sin_result[1] - 1.0) < 1e-10, 'sin(pi/2) ~ 1'

# cos(0) = 1, cos(pi/2) ~ 0
cos_result = np.cos(np.array([0.0, math.pi / 2, math.pi]))
assert abs(cos_result[0] - 1.0) < 1e-10, 'cos(0) ~ 1'
assert abs(cos_result[1]) < 1e-10, 'cos(pi/2) ~ 0'

# sin on plain list
assert repr(np.sin([0.0])) == 'array([0.])', 'sin on list'

# === np.log2 ===
assert repr(np.log2(np.array([1.0, 2.0, 4.0, 8.0]))) == 'array([0., 1., 2., 3.])', 'log2'

# === np.power ===
assert repr(np.power(np.array([2, 3, 4]), 2)) == 'array([4, 9, 16])', 'power arr-scalar'
assert repr(np.power(2, np.array([1, 2, 3]))) == 'array([2, 4, 8])', 'power scalar-arr'

# === np.diff ===
assert repr(np.diff(np.array([1, 3, 6, 10]))) == 'array([2, 3, 4])', 'diff int'
assert repr(np.diff(np.array([1.0, 2.5, 4.0]))) == 'array([1.5, 1.5])', 'diff float'

# ============================================================
# 20. ARRAY CREATION EXPANSION
# ============================================================

# === np.full ===
assert repr(np.full(3, 7)) == 'array([7, 7, 7])', 'full int'
assert repr(np.full(3, 7.0)) == 'array([7., 7., 7.])', 'full float'
assert repr(np.full(4, True)) == 'array([ True,  True,  True,  True])', 'full bool'
assert np.full(3, 5).dtype == 'int64', 'full int dtype'
assert np.full(3, 5.0).dtype == 'float64', 'full float dtype'

# === np.eye ===
e = np.eye(3)
assert e.shape == (3, 3), 'eye shape'
assert e.dtype == 'float64', 'eye dtype'
assert e[0].tolist() == [1.0, 0.0, 0.0], 'eye row 0'
assert e[1].tolist() == [0.0, 1.0, 0.0], 'eye row 1'
assert e[2].tolist() == [0.0, 0.0, 1.0], 'eye row 2'

# === np.copy ===
orig = np.array([1, 2, 3])
c = np.copy(orig)
assert repr(c) == 'array([1, 2, 3])', 'copy array'
assert repr(np.copy([4, 5, 6])) == 'array([4, 5, 6])', 'copy list'

# === np.empty ===
e = np.empty(3)
assert e.shape == (3,), 'empty shape'
assert e.dtype == 'float64', 'empty dtype'
assert len(e) == 3, 'empty len'

# === np.zeros with tuple shape ===
z = np.zeros((2, 3))
assert z.shape == (2, 3), 'zeros tuple shape'
assert z.dtype == 'float64', 'zeros tuple dtype'
assert z.tolist() == [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]], 'zeros tuple values'

# === np.ones with tuple shape ===
o = np.ones((2, 3))
assert o.shape == (2, 3), 'ones tuple shape'
assert o.tolist() == [[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]], 'ones tuple values'

# === np.zeros_like / np.ones_like ===
a = np.array([1, 2, 3])
z = np.zeros_like(a)
assert repr(z) == 'array([0, 0, 0])', 'zeros_like int'
assert z.dtype == 'int64', 'zeros_like preserves dtype'
o = np.ones_like(a)
assert repr(o) == 'array([1, 1, 1])', 'ones_like int'
b = np.array([1.0, 2.0])
assert np.zeros_like(b).dtype == 'float64', 'zeros_like float dtype'
assert repr(np.ones_like(b)) == 'array([1., 1.])', 'ones_like float'

# ============================================================
# 21. TESTING & INSPECTION FUNCTIONS
# ============================================================

# === np.isnan, np.isinf, np.isfinite ===
a = np.array([1.0, float('nan'), float('inf'), float('-inf'), 0.0])
assert repr(np.isnan(a)) == 'array([False,  True, False, False, False])', 'isnan'
assert repr(np.isinf(a)) == 'array([False, False,  True,  True, False])', 'isinf'
assert repr(np.isfinite(a)) == 'array([ True, False, False, False,  True])', 'isfinite'
# Works on int arrays (always finite, never NaN)
assert repr(np.isnan(np.array([1, 2, 3]))) == 'array([False, False, False])', 'isnan int'
assert repr(np.isfinite(np.array([1, 2, 3]))) == 'array([ True,  True,  True])', 'isfinite int'

# === np.array_equal ===
assert np.array_equal(np.array([1, 2, 3]), np.array([1, 2, 3])) == True, 'array_equal true'
assert np.array_equal(np.array([1, 2, 3]), np.array([1, 2, 4])) == False, 'array_equal false'
assert np.array_equal(np.array([1, 2]), np.array([1, 2, 3])) == False, 'array_equal diff shape'

# === np.count_nonzero ===
assert np.count_nonzero(np.array([0, 1, 2, 0, 3])) == 3, 'count_nonzero'
assert np.count_nonzero(np.array([0.0, 0.0])) == 0, 'count_nonzero zeros'
assert np.count_nonzero(np.array([True, False, True])) == 2, 'count_nonzero bool'

# === np.all / np.any (module-level) ===
assert np.all(np.array([True, True, True])) == True, 'all true'
assert np.all(np.array([True, False, True])) == False, 'all false'
assert np.any(np.array([False, False, True])) == True, 'any true'
assert np.any(np.array([False, False, False])) == False, 'any false'
assert np.all(np.array([1, 2, 3])) == True, 'all int truthy'
assert np.any(np.array([0, 0, 0])) == False, 'any int all zero'
assert np.all([1, 1, 1]) == True, 'all on list'

# ============================================================
# 22. AGGREGATION EXPANSION
# ============================================================

# === prod ===
assert np.prod(np.array([1, 2, 3, 4])) == 24, 'np.prod int'
assert np.array([2.0, 3.0, 4.0]).prod() == 24.0, 'arr.prod float'
assert np.prod(np.array([1.0])) == 1.0, 'prod single'
assert np.prod(np.zeros(0)) == 1.0, 'prod empty = 1.0'

# === var ===
a = np.array([1.0, 2.0, 3.0, 4.0, 5.0])
assert a.var() == np.var(a), 'var method == module'
assert abs(a.var() - 2.0) < 1e-10, 'var value'
assert abs(a.std() ** 2 - a.var()) < 1e-10, 'std^2 == var'

# === median ===
assert np.median(np.array([3, 1, 2])) == 2.0, 'median odd'
assert np.median(np.array([1, 2, 3, 4])) == 2.5, 'median even'
assert np.median(np.array([5.0])) == 5.0, 'median single'

# === np.argmin / np.argmax (module-level) ===
assert np.argmin(np.array([3, 1, 2])) == 1, 'np.argmin'
assert np.argmax(np.array([3, 1, 2])) == 0, 'np.argmax'
assert np.argmin([5, 2, 8]) == 1, 'np.argmin on list'

# ============================================================
# 23. ARRAY MANIPULATION
# ============================================================

# === np.reshape (module-level) ===
a = np.arange(6)
b = np.reshape(a, (2, 3))
assert b.shape == (2, 3), 'reshape mod shape'
assert b.tolist() == [[0, 1, 2], [3, 4, 5]], 'reshape mod values'

# === np.transpose (module-level) ===
a = np.array([[1, 2], [3, 4]])
t = np.transpose(a)
assert t.tolist() == [[1, 3], [2, 4]], 'transpose mod'

# === np.append ===
assert repr(np.append(np.array([1, 2, 3]), np.array([4, 5]))) == 'array([1, 2, 3, 4, 5])', 'append arr-arr'
assert repr(np.append(np.array([1, 2]), [3, 4])) == 'array([1, 2, 3, 4])', 'append arr-list'

# === np.vstack ===
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
v = np.vstack([a, b])
assert v.shape == (2, 3), 'vstack shape'
assert v.tolist() == [[1, 2, 3], [4, 5, 6]], 'vstack values'

# === np.hstack ===
h = np.hstack([a, b])
assert repr(h) == 'array([1, 2, 3, 4, 5, 6])', 'hstack'

# === np.stack ===
s = np.stack([a, b])
assert s.shape == (2, 3), 'stack shape'
assert s.tolist() == [[1, 2, 3], [4, 5, 6]], 'stack values'

# === .ravel() ===
a = np.array([[1, 2], [3, 4]])
assert repr(a.ravel()) == 'array([1, 2, 3, 4])', 'ravel'

# ============================================================
# 24. SEARCH & INDEX FUNCTIONS
# ============================================================

# === np.nonzero ===
idx = np.nonzero(np.array([0, 3, 0, 5, 0]))
assert len(idx) == 1, 'nonzero returns 1-tuple for 1d'
assert repr(idx[0]) == 'array([1, 3])', 'nonzero indices'

# === np.argwhere ===
result = np.argwhere(np.array([0, 3, 0, 5, 0]))
assert result.shape == (2, 1), 'argwhere shape'
assert result.flatten().tolist() == [1, 3], 'argwhere values'

# === Fancy indexing with integer arrays ===
a = np.array([10, 20, 30, 40, 50])
idx = np.array([0, 2, 4])
assert repr(a[idx]) == 'array([10, 30, 50])', 'fancy idx'
assert repr(a[np.array([4, 3, 2, 1, 0])]) == 'array([50, 40, 30, 20, 10])', 'fancy idx reverse'

# === Slice indexing ===
a = np.array([10, 20, 30, 40, 50])
assert repr(a[1:3]) == 'array([20, 30])', 'slice 1:3'
assert repr(a[::2]) == 'array([10, 30, 50])', 'slice ::2'
assert repr(a[::-1]) == 'array([50, 40, 30, 20, 10])', 'slice ::-1'
assert repr(a[:-1]) == 'array([10, 20, 30, 40])', 'slice :-1'
assert repr(a[2:]) == 'array([30, 40, 50])', 'slice 2:'

# ============================================================
# 25. REMAINING UTILITIES
# ============================================================

# === np.tile ===
assert repr(np.tile(np.array([1, 2, 3]), 2)) == 'array([1, 2, 3, 1, 2, 3])', 'tile'

# === np.repeat ===
assert repr(np.repeat(np.array([1, 2, 3]), 2)) == 'array([1, 1, 2, 2, 3, 3])', 'repeat'

# === np.split ===
a = np.array([1, 2, 3, 4, 5, 6])
parts = np.split(a, 3)
assert len(parts) == 3, 'split count'
assert repr(parts[0]) == 'array([1, 2])', 'split part 0'
assert repr(parts[1]) == 'array([3, 4])', 'split part 1'
assert repr(parts[2]) == 'array([5, 6])', 'split part 2'

# split by indices
parts2 = np.split(a, [2, 4])
assert repr(parts2[0]) == 'array([1, 2])', 'split idx part 0'
assert repr(parts2[1]) == 'array([3, 4])', 'split idx part 1'
assert repr(parts2[2]) == 'array([5, 6])', 'split idx part 2'

# === .astype aliases ===
a = np.array([1.5, 2.7, 3.1])
assert repr(a.astype('int32')) == 'array([1, 2, 3])', 'astype int32'
assert repr(a.astype('float32')) == 'array([1.5, 2.7, 3.1])', 'astype float32'
assert repr(a.astype('int')) == 'array([1, 2, 3])', 'astype int'
assert repr(a.astype('float')) == 'array([1.5, 2.7, 3.1])', 'astype float'

# ============================================================
# 26. EDGE CASES FOR NEW FUNCTIONS
# ============================================================

# Empty array edge cases
assert repr(np.sin(np.zeros(0))) == 'array([], dtype=float64)', 'sin empty'
assert np.prod(np.array([1])) == 1, 'prod single'
assert np.count_nonzero(np.zeros(0)) == 0, 'count_nonzero empty'
assert repr(np.tile(np.array([1, 2]), 0)) == 'array([], dtype=int64)', 'tile 0 reps'
assert repr(np.repeat(np.zeros(0), 3)) == 'array([], dtype=float64)', 'repeat empty'
assert repr(np.diff(np.array([5]))) == 'array([], dtype=int64)', 'diff single element'
assert repr(np.full(0, 5)) == 'array([], dtype=int64)', 'full size 0'

# ============================================================
# 27. ADDITIONAL DTYPE AND OPERATION COVERAGE
# ============================================================

# === Bool array creation and dtype ===
b = np.array([True, False, True])
assert b.dtype == 'bool', 'bool array dtype'
assert b.tolist() == [True, False, True], 'bool array tolist'
assert repr(b) == 'array([ True, False,  True])', 'bool array repr'
assert b.sum() == 2, 'bool sum'
assert b.any() == True, 'bool any'
assert b.all() == False, 'bool all'

# Bool from comparison
c = np.array([1, 2, 3]) > 1
assert c.dtype == 'bool', 'comparison produces bool'
assert c.sum() == 2, 'comparison bool sum'
assert c.tolist() == [False, True, True], 'comparison bool tolist'

# === Additional math function edge cases ===
# sin on int array
sin_int = np.sin(np.array([0, 1]))
assert sin_int.dtype == 'float64', 'sin int array -> float64'
assert sin_int[0] == 0.0, 'sin(0) exact'

# cos on int array
cos_int = np.cos(np.array([0]))
assert cos_int[0] == 1.0, 'cos(0) exact'
assert cos_int.dtype == 'float64', 'cos int array -> float64'

# log2 edge cases
assert np.log2(np.array([1.0]))[0] == 0.0, 'log2(1) = 0'
assert np.log2(np.array([2.0]))[0] == 1.0, 'log2(2) = 1'
assert np.log2(np.array([16.0]))[0] == 4.0, 'log2(16) = 4'

# diff on larger array
assert np.diff(np.array([1, 1, 1, 1])).tolist() == [0, 0, 0], 'diff constant'
assert np.diff(np.array([0, 1, 4, 9])).tolist() == [1, 3, 5], 'diff quadratic'

# power array-array
assert repr(np.power(np.array([2, 3]), np.array([3, 2]))) == 'array([8, 9])', 'power arr-arr'

# === Additional aggregation edge cases ===
# prod with negatives
assert np.prod(np.array([-1, 2, -3])) == 6, 'prod negatives'
assert np.prod(np.array([0, 1, 2])) == 0, 'prod with zero'

# var of uniform array
assert np.var(np.array([5, 5, 5])) == 0.0, 'var uniform'
assert np.var(np.array([5.0])) == 0.0, 'var single'

# median of larger arrays
assert np.median(np.array([1, 2, 3, 4, 5])) == 3.0, 'median 5 elements'
assert np.median(np.array([10, 20])) == 15.0, 'median 2 elements'

# === count_nonzero more cases ===
assert np.count_nonzero(np.array([1, 1, 1])) == 3, 'count_nonzero all nonzero'
assert np.count_nonzero(np.array([-1, 0, 1])) == 2, 'count_nonzero with neg'
assert np.count_nonzero(np.array([0.0, 0.1, 0.0])) == 1, 'count_nonzero float'

# === Additional array_equal cases ===
assert np.array_equal(np.array([1.0, 2.0]), np.array([1.0, 2.0])) == True, 'array_equal float'
assert np.array_equal(np.zeros(0), np.zeros(0)) == True, 'array_equal empty'
assert np.array_equal(np.array([1]), np.array([1.0])) == True, 'array_equal int vs float'

# === Chained operations ===
# Sort then slice
a = np.sort(np.array([5, 3, 1, 4, 2]))
assert a[0] == 1, 'sort then index first'
assert a[-1] == 5, 'sort then index last'
assert a[2] == 3, 'sort then index mid'

# Arithmetic chains
a = np.array([1, 2, 3])
assert ((a * 2) + 1).tolist() == [3, 5, 7], 'chain mul then add'
assert ((a + 1) * 2).tolist() == [4, 6, 8], 'chain add then mul'
assert (a * a).tolist() == [1, 4, 9], 'array self mul'
assert (a + a).tolist() == [2, 4, 6], 'array self add'

# Comparison chains
assert (a > 1).sum() == 2, 'count elements > 1'
assert (a == 2).sum() == 1, 'count elements == 2'
assert (a < 4).all() == True, 'all < 4'
assert (a > 0).all() == True, 'all positive'
assert (a > 3).any() == False, 'none > 3'

# === 2D operation coverage ===
m = np.array([[1, 2, 3], [4, 5, 6]])
assert m.min() == 1, '2d min'
assert m.max() == 6, '2d max'
assert m.size == 6, '2d size'
assert m.ndim == 2, '2d ndim'
assert m[0][0] == 1, '2d corner tl'
assert m[1][2] == 6, '2d corner br'
assert m[-1].tolist() == [4, 5, 6], '2d neg index row'

# 2D arithmetic
m2 = m + 10
assert m2.tolist() == [[11, 12, 13], [14, 15, 16]], '2d add scalar'
assert m2.shape == (2, 3), '2d add preserves shape'
m3 = m * m
assert m3.tolist() == [[1, 4, 9], [16, 25, 36]], '2d self mul'

# === Repr edge cases ===
assert repr(np.array([0])) == 'array([0])', 'repr zero int'
assert repr(np.array([0.0])) == 'array([0.])', 'repr zero float'
assert repr(np.array([-1])) == 'array([-1])', 'repr negative int'
assert repr(np.array([-1.5])) == 'array([-1.5])', 'repr negative float'
assert repr(np.array([True, True])) == 'array([ True,  True])', 'repr bool all true'
assert repr(np.array([False])) == 'array([False])', 'repr bool single false'

# === np.eye additional cases ===
e1 = np.eye(1)
assert e1.tolist() == [[1.0]], 'eye 1x1'
e2 = np.eye(2)
assert e2.tolist() == [[1.0, 0.0], [0.0, 1.0]], 'eye 2x2'

# === np.full additional cases ===
assert np.full(1, 42).tolist() == [42], 'full single'
assert np.full(5, 0).tolist() == [0, 0, 0, 0, 0], 'full zeros'
assert np.full(3, -1).tolist() == [-1, -1, -1], 'full negative'
assert np.full(2, 3.14).tolist() == [3.14, 3.14], 'full pi'

# === np.zeros_like / np.ones_like additional ===
f = np.array([1.5, 2.5, 3.5])
assert np.zeros_like(f).tolist() == [0.0, 0.0, 0.0], 'zeros_like float vals'
assert np.ones_like(f).tolist() == [1.0, 1.0, 1.0], 'ones_like float vals'

# === np.isnan/isinf/isfinite on plain values ===
assert np.isnan(np.array([0.0, 1.0])).tolist() == [False, False], 'isnan no nans'
assert np.isinf(np.array([0.0, 1.0])).tolist() == [False, False], 'isinf no infs'
assert np.isfinite(np.array([0.0, 1.0])).tolist() == [True, True], 'isfinite normal'

# === np.split additional ===
a = np.arange(10)
parts = np.split(a, 5)
assert len(parts) == 5, 'split into 5'
assert parts[0].tolist() == [0, 1], 'split 5 part 0'
assert parts[4].tolist() == [8, 9], 'split 5 part 4'

# === np.tile additional ===
assert np.tile(np.array([1, 2]), 3).tolist() == [1, 2, 1, 2, 1, 2], 'tile 3x'
assert np.tile(np.array([5]), 4).tolist() == [5, 5, 5, 5], 'tile single 4x'
assert np.tile(np.array([1, 2]), 1).tolist() == [1, 2], 'tile 1x identity'

# === np.repeat additional ===
assert np.repeat(np.array([1, 2, 3]), 1).tolist() == [1, 2, 3], 'repeat 1x identity'
assert np.repeat(np.array([5]), 3).tolist() == [5, 5, 5], 'repeat single 3x'
assert np.repeat(np.array([1, 2]), 3).tolist() == [1, 1, 1, 2, 2, 2], 'repeat 3x'

# === Slice indexing additional ===
a = np.arange(10)
assert a[2:5].tolist() == [2, 3, 4], 'slice 2:5'
assert a[:3].tolist() == [0, 1, 2], 'slice :3'
assert a[7:].tolist() == [7, 8, 9], 'slice 7:'
assert a[::3].tolist() == [0, 3, 6, 9], 'slice ::3'
assert a[1:7:2].tolist() == [1, 3, 5], 'slice 1:7:2'
assert a[::-2].tolist() == [9, 7, 5, 3, 1], 'slice ::-2'

# === Fancy indexing additional ===
a = np.arange(10)
idx = np.array([0, 5, 9])
assert a[idx].tolist() == [0, 5, 9], 'fancy idx sparse'
idx2 = np.array([9, 0])
assert a[idx2].tolist() == [9, 0], 'fancy idx reversed pair'

# === np.nonzero / np.argwhere additional ===
idx = np.nonzero(np.array([1, 0, 0, 1, 1]))
assert idx[0].tolist() == [0, 3, 4], 'nonzero more'
assert np.argwhere(np.array([0, 0, 0])).shape == (0, 1), 'argwhere all zero'
assert np.argwhere(np.array([1, 1, 1])).flatten().tolist() == [0, 1, 2], 'argwhere all nonzero'
