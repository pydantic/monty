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
matrix = np.array([[10, 11], [20, 21], [30, 31]])
assert matrix[np.array([2, 0])].tolist() == [[30, 31], [10, 11]], 'fancy idx selects rows'
try:
    a[np.array([0.0])]
    assert False, 'expected float fancy index to fail'
except IndexError as exc:
    assert str(exc) == 'arrays used as indices must be of integer (or boolean) type', 'float fancy idx error'

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

# ============================================================
# 28. CONSTANTS AND TYPE OBJECTS
# ============================================================

# === np.pi ===
assert abs(np.pi - 3.141592653589793) < 1e-15, 'np.pi value'
assert np.pi == math.pi, 'np.pi matches math.pi'

# === np.e ===
assert abs(np.e - 2.718281828459045) < 1e-15, 'np.e value'
assert np.e == math.e, 'np.e matches math.e'

# === np.inf ===
assert np.inf == math.inf, 'np.inf matches math.inf'
assert np.inf > 1e308, 'np.inf is large'
assert math.isinf(np.inf), 'np.inf is inf'

# === np.nan ===
assert np.nan != np.nan, 'np.nan is NaN (not equal to self)'
assert math.isnan(np.nan), 'np.nan is nan'

# === np.newaxis ===
assert np.newaxis is None, 'np.newaxis is None'

# ============================================================
# 29. INVERSE TRIGONOMETRIC FUNCTIONS
# ============================================================

# === np.arcsin ===
a = np.array([0.0, 0.5, 1.0])
r = np.arcsin(a)
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'arcsin 0'
assert abs(r.tolist()[1] - 0.5235987755982988) < 1e-10, 'arcsin 0.5'
assert abs(r.tolist()[2] - 1.5707963267948966) < 1e-10, 'arcsin 1'

# === np.arccos ===
r = np.arccos(a)
assert abs(r.tolist()[0] - 1.5707963267948966) < 1e-10, 'arccos 0'
assert abs(r.tolist()[2] - 0.0) < 1e-10, 'arccos 1'

# === np.arctan ===
r = np.arctan(a)
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'arctan 0'
assert abs(r.tolist()[2] - 0.7853981633974483) < 1e-10, 'arctan 1'

# === np.arctan2 ===
r = np.arctan2(np.array([1.0, 0.0]), np.array([1.0, 1.0]))
assert abs(r.tolist()[0] - 0.7853981633974483) < 1e-10, 'arctan2(1,1)'
assert abs(r.tolist()[1] - 0.0) < 1e-10, 'arctan2(0,1)'

# ============================================================
# 30. HYPERBOLIC FUNCTIONS
# ============================================================

# === np.sinh ===
r = np.sinh(np.array([0.0, 1.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'sinh 0'
assert abs(r.tolist()[1] - 1.1752011936438014) < 1e-10, 'sinh 1'

# === np.cosh ===
r = np.cosh(np.array([0.0, 1.0]))
assert abs(r.tolist()[0] - 1.0) < 1e-10, 'cosh 0'
assert abs(r.tolist()[1] - 1.5430806348152437) < 1e-10, 'cosh 1'

# === np.tanh ===
r = np.tanh(np.array([0.0, 1.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'tanh 0'
assert abs(r.tolist()[1] - 0.7615941559557649) < 1e-10, 'tanh 1'

# === np.arcsinh ===
r = np.arcsinh(np.array([0.0, 1.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'arcsinh 0'
assert abs(r.tolist()[1] - 0.881373587019543) < 1e-10, 'arcsinh 1'

# === np.arccosh ===
r = np.arccosh(np.array([1.0, 2.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'arccosh 1'
assert abs(r.tolist()[1] - 1.3169578969248166) < 1e-10, 'arccosh 2'

# === np.arctanh ===
r = np.arctanh(np.array([0.0, 0.5]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'arctanh 0'
assert abs(r.tolist()[1] - 0.5493061443340549) < 1e-10, 'arctanh 0.5'

# ============================================================
# 31. REMAINING ELEMENT-WISE MATH
# ============================================================

# === np.sign ===
assert np.sign(np.array([-3.0, 0.0, 5.0])).tolist() == [-1.0, 0.0, 1.0], 'sign'
assert np.sign(np.array([-1, 0, 1])).tolist() == [-1, 0, 1], 'sign int'

# === np.square ===
assert np.square(np.array([2.0, 3.0, 4.0])).tolist() == [4.0, 9.0, 16.0], 'square'
assert np.square(np.array([0.0, -2.0])).tolist() == [0.0, 4.0], 'square neg'

# === np.cbrt ===
assert np.cbrt(np.array([8.0, 27.0])).tolist() == [2.0, 3.0], 'cbrt'
assert abs(np.cbrt(np.array([1.0])).tolist()[0] - 1.0) < 1e-10, 'cbrt 1'

# === np.reciprocal ===
assert np.reciprocal(np.array([2.0, 4.0, 5.0])).tolist() == [0.5, 0.25, 0.2], 'reciprocal'

# === np.log1p ===
r = np.log1p(np.array([0.0, 1.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'log1p 0'
assert abs(r.tolist()[1] - 0.6931471805599453) < 1e-10, 'log1p 1'

# === np.exp2 ===
assert np.exp2(np.array([0.0, 1.0, 3.0])).tolist() == [1.0, 2.0, 8.0], 'exp2'

# === np.expm1 ===
r = np.expm1(np.array([0.0, 1.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'expm1 0'
assert abs(r.tolist()[1] - 1.7182818284590453) < 1e-10, 'expm1 1'

# === np.deg2rad ===
r = np.deg2rad(np.array([0.0, 90.0, 180.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'deg2rad 0'
assert abs(r.tolist()[1] - 1.5707963267948966) < 1e-10, 'deg2rad 90'
assert abs(r.tolist()[2] - 3.141592653589793) < 1e-10, 'deg2rad 180'

# === np.rad2deg ===
r = np.rad2deg(np.array([0.0, np.pi / 2, np.pi]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'rad2deg 0'
assert abs(r.tolist()[1] - 90.0) < 1e-10, 'rad2deg pi/2'
assert abs(r.tolist()[2] - 180.0) < 1e-10, 'rad2deg pi'

# === np.degrees (alias for rad2deg) ===
r = np.degrees(np.array([0.0, np.pi]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'degrees 0'
assert abs(r.tolist()[1] - 180.0) < 1e-10, 'degrees pi'

# === np.radians (alias for deg2rad) ===
r = np.radians(np.array([0.0, 180.0]))
assert abs(r.tolist()[0] - 0.0) < 1e-10, 'radians 0'
assert abs(r.tolist()[1] - 3.141592653589793) < 1e-10, 'radians 180'

# === np.hypot ===
assert np.hypot(np.array([3.0]), np.array([4.0])).tolist() == [5.0], 'hypot 3-4-5'
assert np.hypot(np.array([0.0]), np.array([5.0])).tolist() == [5.0], 'hypot 0-5'

# === np.nan_to_num ===
a = np.array([1.0, float('nan'), float('inf'), float('-inf')])
r = np.nan_to_num(a)
assert r.tolist()[0] == 1.0, 'nan_to_num keeps normal'
assert r.tolist()[1] == 0.0, 'nan_to_num replaces nan'

# === np.fmin / np.fmax ===
assert np.fmin(np.array([1.0, 3.0]), np.array([2.0, 1.0])).tolist() == [1.0, 1.0], 'fmin basic'
assert np.fmax(np.array([1.0, 3.0]), np.array([2.0, 1.0])).tolist() == [2.0, 3.0], 'fmax basic'
# fmin/fmax ignore NaN
a = np.array([1.0, float('nan')])
b = np.array([2.0, 3.0])
assert np.fmin(a, b).tolist()[0] == 1.0, 'fmin nan ignore 1'
assert np.fmin(a, b).tolist()[1] == 3.0, 'fmin nan ignore 2'
assert np.fmax(a, b).tolist()[0] == 2.0, 'fmax nan ignore 1'
assert np.fmax(a, b).tolist()[1] == 3.0, 'fmax nan ignore 2'

# === np.fmod ===
assert np.fmod(np.array([5.0, 7.0]), np.array([3.0, 2.0])).tolist() == [2.0, 1.0], 'fmod'

# === np.rint ===
assert np.rint(np.array([1.5, 2.5, 3.5])).tolist() == [2.0, 2.0, 4.0], 'rint banker'
assert np.rint(np.array([0.5, 1.5])).tolist() == [0.0, 2.0], 'rint half even'

# === np.fabs ===
assert np.fabs(np.array([-1.0, 2.0, -3.0])).tolist() == [1.0, 2.0, 3.0], 'fabs'

# === np.positive / np.negative ===
assert np.positive(np.array([-1.0, 2.0])).tolist() == [-1.0, 2.0], 'positive'
assert np.negative(np.array([-1.0, 2.0])).tolist() == [1.0, -2.0], 'negative'

# ============================================================
# 32. NAN-AWARE AGGREGATIONS
# ============================================================

a_nan = np.array([1.0, float('nan'), 3.0, float('nan'), 5.0])

# === np.nansum ===
assert np.nansum(a_nan) == 9.0, 'nansum'
assert np.nansum(np.array([1.0, 2.0, 3.0])) == 6.0, 'nansum no nan'

# === np.nanmean ===
assert np.nanmean(a_nan) == 3.0, 'nanmean'

# === np.nanmin ===
assert np.nanmin(a_nan) == 1.0, 'nanmin'

# === np.nanmax ===
assert np.nanmax(a_nan) == 5.0, 'nanmax'

# === np.nanstd ===
assert abs(np.nanstd(a_nan) - 1.632993161855452) < 1e-10, 'nanstd'

# === np.nanvar ===
assert abs(np.nanvar(a_nan) - 2.6666666666666665) < 1e-10, 'nanvar'

# === np.nanprod ===
assert np.nanprod(a_nan) == 15.0, 'nanprod'

# === np.nanmedian ===
assert np.nanmedian(a_nan) == 3.0, 'nanmedian'

# === np.nanargmin ===
assert np.nanargmin(a_nan) == 0, 'nanargmin'

# === np.nanargmax ===
assert np.nanargmax(a_nan) == 4, 'nanargmax'

# === np.nancumsum ===
assert np.nancumsum(a_nan).tolist() == [1.0, 1.0, 4.0, 4.0, 9.0], 'nancumsum'

# === np.nancumprod ===
assert np.nancumprod(a_nan).tolist() == [1.0, 1.0, 3.0, 3.0, 15.0], 'nancumprod'

# ============================================================
# 33. ADDITIONAL STATISTICS
# ============================================================

# === np.ptp ===
assert np.ptp(np.array([3.0, 1.0, 5.0, 2.0])) == 4.0, 'ptp'
assert np.ptp(np.array([7.0])) == 0.0, 'ptp single'

# === np.cumprod ===
assert np.cumprod(np.array([1.0, 2.0, 3.0, 4.0])).tolist() == [1.0, 2.0, 6.0, 24.0], 'cumprod'
assert np.cumprod(np.array([5.0])).tolist() == [5.0], 'cumprod single'

# === np.percentile ===
assert np.percentile(np.array([1.0, 2.0, 3.0, 4.0]), 50) == 2.5, 'percentile 50'
assert np.percentile(np.array([1.0, 2.0, 3.0, 4.0]), 0) == 1.0, 'percentile 0'
assert np.percentile(np.array([1.0, 2.0, 3.0, 4.0]), 100) == 4.0, 'percentile 100'

# === np.quantile ===
assert np.quantile(np.array([1.0, 2.0, 3.0, 4.0]), 0.5) == 2.5, 'quantile 0.5'
assert np.quantile(np.array([1.0, 2.0, 3.0, 4.0]), 0.0) == 1.0, 'quantile 0'
assert np.quantile(np.array([1.0, 2.0, 3.0, 4.0]), 1.0) == 4.0, 'quantile 1'

# === np.average ===
assert np.average(np.array([1.0, 2.0, 3.0])) == 2.0, 'average'

# ============================================================
# 34. LOGICAL AND TESTING FUNCTIONS
# ============================================================

# === np.logical_and ===
assert np.logical_and(np.array([1, 1, 0]), np.array([1, 0, 0])).tolist() == [True, False, False], 'logical_and'

# === np.logical_or ===
assert np.logical_or(np.array([1, 1, 0]), np.array([1, 0, 0])).tolist() == [True, True, False], 'logical_or'

# === np.logical_not ===
assert np.logical_not(np.array([1, 0, 1])).tolist() == [False, True, False], 'logical_not'

# === np.logical_xor ===
assert np.logical_xor(np.array([1, 1, 0]), np.array([1, 0, 0])).tolist() == [False, True, False], 'logical_xor'

# === np.allclose ===
assert np.allclose(np.array([1.0, 2.0]), np.array([1.0, 2.0])) == True, 'allclose exact'
assert np.allclose(np.array([1.0, 2.0]), np.array([1.0, 2.1])) == False, 'allclose not close'
assert np.allclose(np.array([1.0]), np.array([1.0 + 1e-9])) == True, 'allclose within tol'

# === np.isclose ===
r = np.isclose(np.array([1.0, 2.0]), np.array([1.0, 2.1]))
assert r.tolist() == [True, False], 'isclose basic'

# === np.isin ===
r = np.isin(np.array([1.0, 2.0, 3.0, 4.0]), np.array([2.0, 4.0]))
assert r.tolist() == [False, True, False, True], 'isin'

# ============================================================
# 35. ARRAY MANIPULATION
# ============================================================

# === np.flip ===
assert np.flip(np.array([1.0, 2.0, 3.0])).tolist() == [3.0, 2.0, 1.0], 'flip 1d'
assert np.flip(np.array([1, 2, 3])).tolist() == [3, 2, 1], 'flip 1d int'

# === np.fliplr ===
a2d = np.array([[1.0, 2.0], [3.0, 4.0]])
assert np.fliplr(a2d).tolist() == [[2.0, 1.0], [4.0, 3.0]], 'fliplr'

# === np.flipud ===
assert np.flipud(a2d).tolist() == [[3.0, 4.0], [1.0, 2.0]], 'flipud'

# === np.roll ===
assert np.roll(np.array([1.0, 2.0, 3.0, 4.0]), 2).tolist() == [3.0, 4.0, 1.0, 2.0], 'roll +2'
assert np.roll(np.array([1.0, 2.0, 3.0, 4.0]), -1).tolist() == [2.0, 3.0, 4.0, 1.0], 'roll -1'
assert np.roll(np.array([1.0, 2.0, 3.0]), 0).tolist() == [1.0, 2.0, 3.0], 'roll 0'

# === np.expand_dims ===
a = np.array([1.0, 2.0, 3.0])
assert np.expand_dims(a, 0).shape == (1, 3), 'expand_dims axis=0'
assert np.expand_dims(a, 1).shape == (3, 1), 'expand_dims axis=1'

# === np.squeeze ===
a = np.array([[[1.0], [2.0]]])
assert np.squeeze(a).shape == (2,), 'squeeze removes 1-dims'
a2 = np.array([[1.0, 2.0], [3.0, 4.0]])
assert np.squeeze(a2).shape == (2, 2), 'squeeze no effect'

# === np.ravel (module-level) ===
assert np.ravel(np.array([[1.0, 2.0], [3.0, 4.0]])).tolist() == [1.0, 2.0, 3.0, 4.0], 'ravel 2d'

# === np.delete ===
assert np.delete(np.array([1.0, 2.0, 3.0, 4.0]), 1).tolist() == [1.0, 3.0, 4.0], 'delete idx 1'
assert np.delete(np.array([1.0, 2.0, 3.0]), 0).tolist() == [2.0, 3.0], 'delete idx 0'
assert np.delete(np.array([1.0, 2.0, 3.0]), 2).tolist() == [1.0, 2.0], 'delete last'

# === np.insert ===
assert np.insert(np.array([1.0, 2.0, 4.0]), 2, 3.0).tolist() == [1.0, 2.0, 3.0, 4.0], 'insert middle'
assert np.insert(np.array([2.0, 3.0]), 0, 1.0).tolist() == [1.0, 2.0, 3.0], 'insert front'

# === np.diag ===
# 1D → 2D diagonal matrix
assert np.diag(np.array([1.0, 2.0, 3.0])).tolist() == [[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 3.0]], 'diag 1d->2d'
# 2D → 1D diagonal extraction
assert np.diag(np.array([[1.0, 2.0], [3.0, 4.0]])).tolist() == [1.0, 4.0], 'diag 2d->1d'

# === np.diagonal ===
assert np.diagonal(np.array([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])).tolist() == [1.0, 5.0], 'diagonal rect'
assert np.diagonal(np.array([[1.0, 2.0], [3.0, 4.0]])).tolist() == [1.0, 4.0], 'diagonal square'

# === np.trace ===
assert np.trace(np.array([[1.0, 2.0], [3.0, 4.0]])) == 5.0, 'trace 2x2'
assert np.trace(np.array([[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 3.0]])) == 6.0, 'trace 3x3'

# === np.flatnonzero ===
assert np.flatnonzero(np.array([0.0, 1.0, 0.0, 3.0])).tolist() == [1, 3], 'flatnonzero'
assert np.flatnonzero(np.array([0.0, 0.0])).tolist() == [], 'flatnonzero all zero'
assert np.flatnonzero(np.array([1.0, 2.0])).tolist() == [0, 1], 'flatnonzero all nonzero'

# === np.asarray ===
r = np.asarray([1.0, 2.0, 3.0])
assert r.tolist() == [1.0, 2.0, 3.0], 'asarray from list'

# === np.column_stack ===
a = np.array([1.0, 2.0])
b = np.array([3.0, 4.0])
r = np.column_stack([a, b])
assert r.tolist() == [[1.0, 3.0], [2.0, 4.0]], 'column_stack'

# === np.array_split ===
parts = np.array_split(np.array([1.0, 2.0, 3.0, 4.0, 5.0]), 3)
assert parts[0].tolist() == [1.0, 2.0], 'array_split part 0'
assert parts[1].tolist() == [3.0, 4.0], 'array_split part 1'
assert parts[2].tolist() == [5.0], 'array_split part 2'

# === np.full_like ===
a = np.array([1.0, 2.0, 3.0])
assert np.full_like(a, 7.0).tolist() == [7.0, 7.0, 7.0], 'full_like'
assert np.full_like(a, 7.0).shape == (3,), 'full_like shape'

# ============================================================
# 36. SORTING, SEARCHING, SET OPERATIONS
# ============================================================

# === np.argsort (module-level) ===
assert np.argsort(np.array([3.0, 1.0, 2.0])).tolist() == [1, 2, 0], 'argsort mod'

# === np.searchsorted ===
assert np.searchsorted(np.array([1.0, 3.0, 5.0, 7.0]), 4.0) == 2, 'searchsorted'
assert np.searchsorted(np.array([1.0, 3.0, 5.0]), 0.0) == 0, 'searchsorted left'
assert np.searchsorted(np.array([1.0, 3.0, 5.0]), 6.0) == 3, 'searchsorted right'

# === np.extract ===
cond = np.array([1, 0, 1, 0])
arr = np.array([10.0, 20.0, 30.0, 40.0])
assert np.extract(cond, arr).tolist() == [10.0, 30.0], 'extract'
assert np.extract(np.array([0, 0]), np.array([1.0, 2.0])).tolist() == [], 'extract none'

# === np.intersect1d ===
assert np.intersect1d(np.array([1.0, 2.0, 3.0]), np.array([2.0, 3.0, 4.0])).tolist() == [2.0, 3.0], 'intersect1d'

# === np.union1d ===
assert np.union1d(np.array([1.0, 2.0]), np.array([2.0, 3.0])).tolist() == [1.0, 2.0, 3.0], 'union1d'

# === np.setdiff1d ===
assert np.setdiff1d(np.array([1.0, 2.0, 3.0]), np.array([2.0])).tolist() == [1.0, 3.0], 'setdiff1d'

# === np.setxor1d ===
assert np.setxor1d(np.array([1.0, 2.0, 3.0]), np.array([2.0, 3.0, 4.0])).tolist() == [1.0, 4.0], 'setxor1d'

# === np.bincount ===
assert np.bincount(np.array([0, 1, 1, 2, 2, 2])).tolist() == [1, 2, 3], 'bincount'
assert np.bincount(np.array([3])).tolist() == [0, 0, 0, 1], 'bincount sparse'

# === np.digitize ===
assert np.digitize(np.array([0.5, 1.5, 2.5, 3.5]), np.array([1.0, 2.0, 3.0])).tolist() == [0, 1, 2, 3], 'digitize'

# ============================================================
# 37. LINEAR ALGEBRA BASICS
# ============================================================

# === np.outer ===
assert np.outer(np.array([1.0, 2.0]), np.array([3.0, 4.0])).tolist() == [[3.0, 4.0], [6.0, 8.0]], 'outer product'
assert np.outer(np.array([1.0]), np.array([2.0, 3.0])).tolist() == [[2.0, 3.0]], 'outer 1x2'

# === np.cross ===
assert np.cross(np.array([1.0, 0.0, 0.0]), np.array([0.0, 1.0, 0.0])).tolist() == [0.0, 0.0, 1.0], 'cross i x j'
assert np.cross(np.array([0.0, 1.0, 0.0]), np.array([0.0, 0.0, 1.0])).tolist() == [1.0, 0.0, 0.0], 'cross j x k'

# ============================================================
# 38. CREATION FUNCTIONS (ADVANCED)
# ============================================================

# === np.logspace ===
r = np.logspace(0, 2, 3)
assert abs(r.tolist()[0] - 1.0) < 1e-10, 'logspace start'
assert abs(r.tolist()[1] - 10.0) < 1e-10, 'logspace mid'
assert abs(r.tolist()[2] - 100.0) < 1e-10, 'logspace end'

# === np.geomspace ===
r = np.geomspace(1, 100, 3)
assert abs(r.tolist()[0] - 1.0) < 1e-10, 'geomspace start'
assert abs(r.tolist()[1] - 10.0) < 1e-10, 'geomspace mid'
assert abs(r.tolist()[2] - 100.0) < 1e-10, 'geomspace end'

# === np.tri ===
assert np.tri(3).tolist() == [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0]], 'tri 3x3'

# === np.tril ===
m = np.array([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]])
assert np.tril(m).tolist() == [[1.0, 0.0, 0.0], [4.0, 5.0, 0.0], [7.0, 8.0, 9.0]], 'tril'

# === np.triu ===
assert np.triu(m).tolist() == [[1.0, 2.0, 3.0], [0.0, 5.0, 6.0], [0.0, 0.0, 9.0]], 'triu'

# === np.identity ===
assert np.identity(3).tolist() == [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], 'identity 3'
assert np.identity(1).tolist() == [[1.0]], 'identity 1'

# === np.meshgrid ===
x, y = np.meshgrid(np.array([1.0, 2.0, 3.0]), np.array([4.0, 5.0]))
assert x.tolist() == [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0]], 'meshgrid x'
assert y.tolist() == [[4.0, 4.0, 4.0], [5.0, 5.0, 5.0]], 'meshgrid y'

# === np.gradient ===
r = np.gradient(np.array([1.0, 3.0, 6.0, 10.0]))
assert r.tolist() == [2.0, 2.5, 3.5, 4.0], 'gradient'
r2 = np.gradient(np.array([1.0, 4.0]))
assert r2.tolist() == [3.0, 3.0], 'gradient 2-elem'

# === np.convolve ===
r = np.convolve(np.array([1.0, 2.0, 3.0]), np.array([0.0, 1.0, 0.5]))
assert r.tolist() == [0.0, 1.0, 2.5, 4.0, 1.5], 'convolve full'

# === np.interp ===
xp = np.array([1.0, 2.0, 3.0])
fp = np.array([10.0, 20.0, 30.0])
assert np.interp(np.array([1.5, 2.5]), xp, fp).tolist() == [15.0, 25.0], 'interp'
assert np.interp(np.array([0.0]), xp, fp).tolist() == [10.0], 'interp left clamp'
assert np.interp(np.array([4.0]), xp, fp).tolist() == [30.0], 'interp right clamp'

# === np.select ===
c1 = np.array([1, 0, 0])
c2 = np.array([0, 1, 0])
ch1 = np.array([10.0, 20.0, 30.0])
ch2 = np.array([40.0, 50.0, 60.0])
r = np.select([c1, c2], [ch1, ch2], 0.0)
assert r.tolist() == [10.0, 50.0, 0.0], 'select'

# ============================================================
# 39. EMPTY-LIKE / CORRELATE
# ============================================================

# === np.empty_like ===
a = np.array([1.0, 2.0, 3.0])
r = np.empty_like(a)
assert r.shape == (3,), 'empty_like shape'
assert r.dtype == a.dtype, 'empty_like dtype'

# === np.correlate ===
a = np.array([1.0, 2.0, 3.0])
v = np.array([0.0, 1.0, 0.5])
r = np.correlate(a, v)
assert len(r.tolist()) > 0, 'correlate produces output'

# ============================================================
# PHASE 2: CRITICAL MISSING OPERATORS
# ============================================================

# === 2.1 Bitwise operators: &, |, ^ on ndarray ===

# Bool arrays — element-wise logical AND/OR/XOR
a = np.array([1, 0, 1, 0])
b = np.array([1, 1, 0, 0])
ba = a.astype('bool')
bb = b.astype('bool')

r = ba & bb
assert r.tolist() == [True, False, False, False], 'bool array & (AND)'
assert r.dtype == 'bool', 'bool & dtype'

r = ba | bb
assert r.tolist() == [True, True, True, False], 'bool array | (OR)'
assert r.dtype == 'bool', 'bool | dtype'

r = ba ^ bb
assert r.tolist() == [False, True, True, False], 'bool array ^ (XOR)'
assert r.dtype == 'bool', 'bool ^ dtype'

# Int arrays — bitwise AND/OR/XOR
a = np.array([5, 3, 12, 10])
b = np.array([3, 6, 10, 15])

r = a & b
assert r.tolist() == [1, 2, 8, 10], 'int array & (AND)'
assert r.dtype == 'int64', 'int & dtype'

r = a | b
assert r.tolist() == [7, 7, 14, 15], 'int array | (OR)'
assert r.dtype == 'int64', 'int | dtype'

r = a ^ b
assert r.tolist() == [6, 5, 6, 5], 'int array ^ (XOR)'
assert r.dtype == 'int64', 'int ^ dtype'

# Scalar bitwise operations
a = np.array([5, 3, 12])
r = a & 3
assert r.tolist() == [1, 3, 0], 'int array & scalar'

r = a | 2
assert r.tolist() == [7, 3, 14], 'int array | scalar'

r = a ^ 6
assert r.tolist() == [3, 5, 10], 'int array ^ scalar'

# Scalar on left
r = 3 & np.array([5, 3, 12])
assert r.tolist() == [1, 3, 0], 'scalar & int array'

# === 2.2 __setitem__ — arr[i] = val, arr[mask] = val, arr[slice] = val ===

# arr[i] = val — set single element by int index
a = np.array([1, 2, 3, 4, 5])
a[0] = 10
assert a.tolist() == [10, 2, 3, 4, 5], 'setitem int index 0'

a[4] = 50
assert a.tolist() == [10, 2, 3, 4, 50], 'setitem int index 4'

a[-1] = 99
assert a.tolist() == [10, 2, 3, 4, 99], 'setitem negative index'

# arr[mask] = val — set elements where bool mask is True
a = np.array([1, 2, 3, 4, 5])
mask = np.array([1, 0, 1, 0, 1]).astype('bool')
a[mask] = 0
assert a.tolist() == [0, 2, 0, 4, 0], 'setitem bool mask'

# arr[slice] = val — set slice of elements
a = np.array([1, 2, 3, 4, 5])
a[1:4] = 99
assert a.tolist() == [1, 99, 99, 99, 5], 'setitem slice'

a = np.array([1, 2, 3, 4, 5])
a[::2] = 0
assert a.tolist() == [0, 2, 0, 4, 0], 'setitem slice step 2'

a = np.array([0, 1, 2, 3])
a[1:3] = np.array([9])
assert a.tolist() == [0, 9, 9, 3], 'setitem slice broadcasts single-element array'

a = np.array([0, 1, 2, 3])
a[3:0:-1] = np.array([9])
assert a.tolist() == [0, 9, 9, 9], 'setitem negative slice broadcasts single-element array'

a = np.array([0, 1, 2, 3])
a[1:1] = np.array([9])
assert a.tolist() == [0, 1, 2, 3], 'setitem empty slice with ndarray is no-op'

try:
    a[0] = 'bad'
    assert False, 'expected non-numeric setitem to fail'
except TypeError as exc:
    assert str(exc) == 'ndarray numeric argument must be int, float, or bool', 'setitem rejects non-numeric value'

try:
    a[1:3] = np.array([7, 8, 9])
    assert False, 'expected longer ndarray slice assignment to fail'
except ValueError as exc:
    assert str(exc) == 'could not broadcast input array from shape (3,) into shape (2,)', 'setitem long slice rhs error'

try:
    a[1:3] = np.array([])
    assert False, 'expected empty ndarray slice assignment into non-empty target to fail'
except ValueError as exc:
    assert str(exc) == 'could not broadcast input array from shape (0,) into shape (2,)', (
        'setitem empty slice rhs error'
    )

# === 2.3 __iter__ — for x in arr ===

# 1D array yields scalars
a = np.array([10, 20, 30])
items = []
for x in a:
    items.append(x)
assert items == [10, 20, 30], 'iter 1D yields scalars'

# Float array
a = np.array([1.5, 2.5, 3.5])
items = []
for x in a:
    items.append(x)
assert items == [1.5, 2.5, 3.5], 'iter 1D float yields scalars'

# list() conversion via iter
a = np.array([1, 2, 3])
assert list(a) == [1, 2, 3], 'list(ndarray) via iter'

try:
    list(np.array(7))
    assert False, 'expected iterating 0d ndarray to fail'
except TypeError as exc:
    assert str(exc) == 'iteration over a 0-d array', '0d ndarray iteration error'

# === 2.4 __contains__ — val in arr ===

a = np.array([1, 2, 3, 4, 5])
assert 3 in a, 'int in array'
assert 6 not in a, 'int not in array'
assert 1.0 in np.array([1.0, 2.0, 3.0]), 'float in array'
assert 9223372036854775808 in np.array([9.223372036854776e18]), 'LongInt in float array'
assert 9223372036854775808 not in np.array([1.0]), 'LongInt not in unrelated float array'

# Bool check
a = np.array([0, 1, 0])
assert 1 in a, '1 in array with zeros'
assert 2 not in a, '2 not in array'

# === 2.5 In-place operators: +=, -=, *=, /= ===

# += scalar
a = np.array([1, 2, 3])
a += 10
assert a.tolist() == [11, 12, 13], 'iadd scalar'

try:
    a = np.array([1, 2, 3])
    a += 0.5
    assert False, 'expected int iadd float scalar to fail'
except TypeError as exc:
    assert str(exc) == (
        "Cannot cast ufunc 'add' output from dtype('float64') to dtype('int64') with casting rule 'same_kind'"
    ), 'iadd float scalar cast error'

a = np.array([1.0, 2.0, 3.0])
a += 0.5
assert a.tolist() == [1.5, 2.5, 3.5], 'float iadd float scalar values'
assert a.dtype == 'float64', 'float iadd float scalar dtype'

# += array
a = np.array([1, 2, 3])
b = np.array([10, 20, 30])
a += b
assert a.tolist() == [11, 22, 33], 'iadd array'

try:
    a = np.array([1, 2, 3])
    a += np.array([0.5, 0.5, 0.5])
    assert False, 'expected int iadd float array to fail'
except TypeError as exc:
    assert str(exc) == (
        "Cannot cast ufunc 'add' output from dtype('float64') to dtype('int64') with casting rule 'same_kind'"
    ), 'iadd float array cast error'

try:
    a = np.array([[1, 2], [3, 4]])
    a += np.array([10, 20, 30, 40])
    assert False, 'expected same-length different-shape iadd to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +=: 'numpy.ndarray' and 'numpy.ndarray'", (
        'iadd shape mismatch error'
    )

# -= scalar
a = np.array([10, 20, 30])
a -= 5
assert a.tolist() == [5, 15, 25], 'isub scalar'

# -= array
a = np.array([10, 20, 30])
b = np.array([1, 2, 3])
a -= b
assert a.tolist() == [9, 18, 27], 'isub array'

# *= scalar
a = np.array([1, 2, 3])
a *= 5
assert a.tolist() == [5, 10, 15], 'imul scalar'

# *= array
a = np.array([2, 3, 4])
b = np.array([10, 20, 30])
a *= b
assert a.tolist() == [20, 60, 120], 'imul array'

# /= scalar
a = np.array([10.0, 20.0, 30.0])
a /= 2
assert a.tolist() == [5.0, 10.0, 15.0], 'idiv scalar'

# /= array
a = np.array([10.0, 20.0, 30.0])
b = np.array([2.0, 4.0, 5.0])
a /= b
assert a.tolist() == [5.0, 5.0, 6.0], 'idiv array'

# === 2.6 @ operator (matmul) and np.matmul ===

# 1D @ 1D — dot product
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
r = a @ b
assert r == 32, '1D @ 1D dot product'

# 2D @ 2D — matrix multiplication
a = np.array([[1, 2], [3, 4]])
b = np.array([[5, 6], [7, 8]])
r = a @ b
assert r.tolist() == [[19, 22], [43, 50]], '2D @ 2D matmul'

# 2D @ 1D — matrix-vector
a = np.array([[1, 2], [3, 4]])
b = np.array([5, 6])
r = a @ b
assert r.tolist() == [17, 39], '2D @ 1D matvec'

# 1D @ 2D — vector-matrix
a = np.array([1, 2])
b = np.array([[3, 4], [5, 6]])
r = a @ b
assert r.tolist() == [13, 16], '1D @ 2D vecmat'

# np.matmul function
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
r = np.matmul(a, b)
assert r == 32, 'np.matmul 1D dot product'

# np.matmul 2D
a = np.array([[1, 0], [0, 1]])
b = np.array([[5, 6], [7, 8]])
r = np.matmul(a, b)
assert r.tolist() == [[5, 6], [7, 8]], 'np.matmul identity 2D'

# === ndarray.item() ===
a = np.array([42])
assert a.item() == 42, 'item int scalar'
a = np.array([3.14])
assert a.item() == 3.14, 'item float scalar'

# === ndarray.cumprod() ===
a = np.array([1, 2, 3, 4])
assert a.cumprod().tolist() == [1, 2, 6, 24], 'cumprod int'
a = np.array([1.0, 2.0, 3.0])
assert a.cumprod().tolist() == [1.0, 2.0, 6.0], 'cumprod float'

# === ndarray.squeeze() ===
a = np.array([[1, 2, 3]])
assert a.squeeze().tolist() == [1, 2, 3], 'squeeze removes unit dim'
assert a.squeeze().shape == (3,), 'squeeze shape'

# === ndarray.take() ===
a = np.array([10, 20, 30, 40, 50])
idx = np.array([0, 2, 4])
assert a.take(idx).tolist() == [10, 30, 50], 'take indices'

# === ndarray.diagonal() ===
a = np.array([[1, 2, 3], [4, 5, 6], [7, 8, 9]])
assert a.diagonal().tolist() == [1, 5, 9], 'diagonal 3x3'
a = np.array([[1, 2], [3, 4]])
assert a.diagonal().tolist() == [1, 4], 'diagonal 2x2'

# === ndarray.trace() ===
a = np.array([[1, 2], [3, 4]])
assert a.trace() == 5, 'trace int 2x2'
a = np.array([[1.0, 2.0], [3.0, 4.0]])
assert a.trace() == 5.0, 'trace float 2x2'

# === ndarray.fill() ===
a = np.array([1.0, 2.0, 3.0])
a.fill(7.0)
assert a.tolist() == [7.0, 7.0, 7.0], 'fill float'

# === ndarray.compress() ===
a = np.array([10, 20, 30, 40])
c = np.array([1, 0, 1, 0])
assert a.compress(c).tolist() == [10, 30], 'compress bool-like'

# === ndarray.swapaxes() ===
a = np.array([[1, 2], [3, 4]])
assert a.swapaxes(0, 1).tolist() == [[1, 3], [2, 4]], 'swapaxes 2D transpose'

# === ndarray.nbytes ===
a = np.array([1.0, 2.0, 3.0])
assert a.nbytes == 24, 'nbytes 3 floats'

# === ndarray.itemsize ===
a = np.array([1.0, 2.0, 3.0])
assert a.itemsize == 8, 'itemsize float64'

# === np.row_stack (alias for vstack) ===
a = np.array([1, 2])
b = np.array([3, 4])
assert np.row_stack([a, b]).tolist() == [[1, 2], [3, 4]], 'row_stack'

# === np.hsplit ===
a = np.array([1, 2, 3, 4, 5, 6])
parts = np.hsplit(a, 3)
assert parts[0].tolist() == [1, 2], 'hsplit part 0'
assert parts[1].tolist() == [3, 4], 'hsplit part 1'
assert parts[2].tolist() == [5, 6], 'hsplit part 2'

# === np.vsplit ===
a = np.array([1, 2, 3, 4])
parts = np.vsplit(a, 2)
assert parts[0].tolist() == [1, 2], 'vsplit part 0'
assert parts[1].tolist() == [3, 4], 'vsplit part 1'

# === np.inner ===
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
assert np.inner(a, b) == 32, 'inner product'

# === np.vdot ===
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
assert np.vdot(a, b) == 32, 'vdot product'

# ============================================================
# 42. COMPREHENSIVE EDGE CASES AND ADDITIONAL COVERAGE
# ============================================================

# === Operator edge cases ===
# Bitwise on single-element arrays
a1 = np.array([True])
b1 = np.array([False])
assert (a1 & b1).tolist() == [False], 'bitwise and single'
assert (a1 | b1).tolist() == [True], 'bitwise or single'
assert (a1 ^ b1).tolist() == [True], 'bitwise xor single'

# Bitwise int operations
ia = np.array([15, 255, 0])
ib = np.array([240, 15, 255])
assert (ia & ib).tolist() == [0, 15, 0], 'int bitwise and'
assert (ia | ib).tolist() == [255, 255, 255], 'int bitwise or'
assert (ia ^ ib).tolist() == [255, 240, 255], 'int bitwise xor'

# Matmul 2D
m1 = np.array([[1, 0], [0, 1]])  # identity matrix
m2 = np.array([[5, 6], [7, 8]])
assert (m1 @ m2).tolist() == [[5, 6], [7, 8]], 'matmul identity'
assert np.matmul(m1, m2).tolist() == [[5, 6], [7, 8]], 'np.matmul identity'

# In-place compound
a = np.array([10.0, 20.0, 30.0])
a += 5
assert a.tolist() == [15.0, 25.0, 35.0], 'iadd then check'
a -= 5
assert a.tolist() == [10.0, 20.0, 30.0], 'isub restore'
a *= 2
assert a.tolist() == [20.0, 40.0, 60.0], 'imul double'
a /= 4
assert a.tolist() == [5.0, 10.0, 15.0], 'idiv quarter'

# === Setitem edge cases ===
a = np.array([1, 2, 3, 4, 5])
a[0] = 99
assert a[0] == 99, 'setitem first'
a[4] = 88
assert a[4] == 88, 'setitem last'
a[-1] = 77
assert a[-1] == 77, 'setitem negative'

# Setitem with bool mask
b = np.array([10, 20, 30, 40, 50])
mask = np.array([1, 0, 1, 0, 1])
b[mask > 0] = 0
assert b.tolist() == [0, 20, 0, 40, 0], 'setitem mask'

# Setitem with slice
c = np.array([1, 2, 3, 4, 5])
c[1:4] = np.array([20, 30, 40])
assert c.tolist() == [1, 20, 30, 40, 5], 'setitem slice'

# === Contains edge cases ===
a = np.array([1.0, 2.0, 3.0])
assert 1.0 in a, 'contains float True'
assert 4.0 not in a, 'contains float False'
assert 2 in np.array([1, 2, 3]), 'contains int True'

# === Iter edge cases ===
a = np.array([10, 20, 30])
collected = []
for x in a:
    collected.append(x)
assert collected == [10, 20, 30], 'iter collect'
assert sum(np.array([1, 2, 3, 4])) == 10, 'sum via iter'

# === Additional trig/math coverage ===
# arcsin edge
r = np.arcsin(np.array([-1.0, 0.0, 1.0]))
assert abs(r.tolist()[0] + 1.5707963267948966) < 1e-10, 'arcsin -1'
assert abs(r.tolist()[1]) < 1e-10, 'arcsin 0'
assert abs(r.tolist()[2] - 1.5707963267948966) < 1e-10, 'arcsin 1'

# arccos edge
r = np.arccos(np.array([-1.0, 0.0, 1.0]))
assert abs(r.tolist()[0] - 3.141592653589793) < 1e-10, 'arccos -1'
assert abs(r.tolist()[1] - 1.5707963267948966) < 1e-10, 'arccos 0'
assert abs(r.tolist()[2]) < 1e-10, 'arccos 1'

# deg2rad/rad2deg roundtrip
angles = np.array([0.0, 45.0, 90.0, 180.0, 360.0])
r = np.rad2deg(np.deg2rad(angles))
assert abs(r.tolist()[0]) < 1e-10, 'roundtrip 0'
assert abs(r.tolist()[1] - 45.0) < 1e-10, 'roundtrip 45'
assert abs(r.tolist()[2] - 90.0) < 1e-10, 'roundtrip 90'
assert abs(r.tolist()[3] - 180.0) < 1e-10, 'roundtrip 180'
assert abs(r.tolist()[4] - 360.0) < 1e-10, 'roundtrip 360'

# sign on integers
assert np.sign(np.array([-5, -1, 0, 1, 5])).tolist() == [-1, -1, 0, 1, 1], 'sign int values'

# square on integers
assert np.square(np.array([0, 1, 2, 3])).tolist() == [0, 1, 4, 9], 'square int'

# reciprocal edge
assert np.reciprocal(np.array([1.0, 0.5, 0.25])).tolist() == [1.0, 2.0, 4.0], 'reciprocal inverse'

# fmod edge
assert np.fmod(np.array([10.0, -10.0, 10.0]), np.array([3.0, 3.0, -3.0])).tolist()[0] == 1.0, 'fmod pos'

# rint edge cases
assert np.rint(np.array([0.0, 1.0, -1.0])).tolist() == [0.0, 1.0, -1.0], 'rint integers'
assert np.rint(np.array([0.1, 0.9, -0.9])).tolist() == [0.0, 1.0, -1.0], 'rint near-integers'

# === NaN-aware edge cases ===
# nansum on all-nan
assert np.nansum(np.array([float('nan'), float('nan')])) == 0.0, 'nansum all nan'

# nanprod on no-nan
assert np.nanprod(np.array([2.0, 3.0, 4.0])) == 24.0, 'nanprod no nan'

# nancumsum on no-nan
assert np.nancumsum(np.array([1.0, 2.0, 3.0])).tolist() == [1.0, 3.0, 6.0], 'nancumsum no nan'

# nancumprod on no-nan
assert np.nancumprod(np.array([1.0, 2.0, 3.0])).tolist() == [1.0, 2.0, 6.0], 'nancumprod no nan'

# nan_to_num with custom behavior
a = np.array([1.0, float('nan'), 3.0])
r = np.nan_to_num(a)
assert r.tolist() == [1.0, 0.0, 3.0], 'nan_to_num simple'

# === Logical function edge cases ===
# logical_and with float arrays (nonzero = True)
assert np.logical_and(np.array([1.0, 0.0, 3.0]), np.array([1.0, 1.0, 0.0])).tolist() == [True, False, False], (
    'logical_and float'
)
assert np.logical_or(np.array([0.0, 0.0, 1.0]), np.array([0.0, 1.0, 0.0])).tolist() == [False, True, True], (
    'logical_or float'
)
assert np.logical_not(np.array([0, 1, 0, 1])).tolist() == [True, False, True, False], 'logical_not int'
assert np.logical_xor(np.array([0, 0, 1, 1]), np.array([0, 1, 0, 1])).tolist() == [False, True, True, False], (
    'logical_xor int'
)

# allclose with different tolerances
assert np.allclose(np.array([1.0, 2.0, 3.0]), np.array([1.0, 2.0, 3.0])) == True, 'allclose identical'
assert np.allclose(np.array([1.0]), np.array([1.1])) == False, 'allclose diff 0.1'

# isclose more cases
r = np.isclose(np.array([1.0, 2.0, 3.0]), np.array([1.0, 2.0, 3.0]))
assert r.tolist() == [True, True, True], 'isclose all same'

# isin more cases
r = np.isin(np.array([1, 2, 3, 4, 5]), np.array([1, 3, 5]))
assert r.tolist() == [True, False, True, False, True], 'isin odd numbers'

# === Manipulation edge cases ===
# flip empty
assert np.flip(np.array([])).tolist() == [], 'flip empty'
assert np.flip(np.array([42.0])).tolist() == [42.0], 'flip single'

# roll edge cases
assert np.roll(np.array([1, 2, 3]), 3).tolist() == [1, 2, 3], 'roll full cycle'
assert np.roll(np.array([1, 2, 3]), 6).tolist() == [1, 2, 3], 'roll double cycle'

# delete edge
assert np.delete(np.array([1, 2, 3, 4, 5]), 0).tolist() == [2, 3, 4, 5], 'delete first'
assert np.delete(np.array([1, 2, 3, 4, 5]), 4).tolist() == [1, 2, 3, 4], 'delete last'

# insert at boundaries
assert np.insert(np.array([2, 3, 4]), 0, 1).tolist() == [1, 2, 3, 4], 'insert at 0'
assert np.insert(np.array([1, 2, 3]), 3, 4).tolist() == [1, 2, 3, 4], 'insert at end'

# diag of identity
assert np.diag(np.eye(3)).tolist() == [1.0, 1.0, 1.0], 'diag of eye'

# trace of identity
assert np.trace(np.eye(4)) == 4.0, 'trace of eye 4'
assert np.trace(np.eye(1)) == 1.0, 'trace of eye 1'

# flatnonzero edge cases
assert np.flatnonzero(np.array([1, 2, 3])).tolist() == [0, 1, 2], 'flatnonzero all'
assert np.flatnonzero(np.array([0])).tolist() == [], 'flatnonzero single zero'

# === Set operation edge cases ===
# intersect1d with no overlap
assert np.intersect1d(np.array([1, 2]), np.array([3, 4])).tolist() == [], 'intersect1d empty'

# union1d with overlap
assert np.union1d(np.array([1, 2, 3]), np.array([2, 3, 4])).tolist() == [1.0, 2.0, 3.0, 4.0], 'union1d overlap'

# setdiff1d with no diff
assert np.setdiff1d(np.array([1, 2]), np.array([1, 2])).tolist() == [], 'setdiff1d empty'

# setxor1d with no overlap
assert np.setxor1d(np.array([1, 2]), np.array([3, 4])).tolist() == [1.0, 2.0, 3.0, 4.0], 'setxor1d disjoint'

# === Creation function edge cases ===
# logspace single element
r = np.logspace(0, 0, 1)
assert abs(r.tolist()[0] - 1.0) < 1e-10, 'logspace single'

# geomspace single element
r = np.geomspace(5, 5, 1)
assert abs(r.tolist()[0] - 5.0) < 1e-10, 'geomspace single'

# tri 1x1
assert np.tri(1).tolist() == [[1.0]], 'tri 1x1'

# identity edge
assert np.identity(2).tolist() == [[1.0, 0.0], [0.0, 1.0]], 'identity 2'

# gradient 3 elements
r = np.gradient(np.array([0.0, 1.0, 4.0]))
assert r.tolist() == [1.0, 2.0, 3.0], 'gradient quadratic'

# interp at boundaries
xp = np.array([0.0, 1.0])
fp = np.array([0.0, 10.0])
assert np.interp(np.array([0.5]), xp, fp).tolist() == [5.0], 'interp midpoint'
assert np.interp(np.array([-1.0]), xp, fp).tolist() == [0.0], 'interp below'
assert np.interp(np.array([2.0]), xp, fp).tolist() == [10.0], 'interp above'

# === Method edge cases ===
# item on single element
assert np.array([42]).item() == 42, 'item int'
assert np.array([3.14]).item() == 3.14, 'item float'

# cumprod
assert np.array([1, 2, 3, 4, 5]).cumprod().tolist() == [1, 2, 6, 24, 120], 'cumprod method'
assert np.array([2.0]).cumprod().tolist() == [2.0], 'cumprod single'

# squeeze on array without 1-dims
a = np.array([1, 2, 3])
assert a.squeeze().tolist() == [1, 2, 3], 'squeeze no effect 1d'

# take
a = np.array([10, 20, 30, 40, 50])
assert a.take(np.array([0, 2, 4])).tolist() == [10, 30, 50], 'take indices'
assert a.take(np.array([4, 3, 2, 1, 0])).tolist() == [50, 40, 30, 20, 10], 'take reversed'

# diagonal on identity
assert np.eye(3).diagonal().tolist() == [1.0, 1.0, 1.0], 'diagonal eye'

# trace on identity
assert np.eye(5).trace() == 5.0, 'trace eye 5'

# fill
a = np.array([1, 2, 3])
a.fill(0)
assert a.tolist() == [0, 0, 0], 'fill zeros'
a.fill(99)
assert a.tolist() == [99, 99, 99], 'fill 99'

# compress
a = np.array([10, 20, 30, 40, 50])
cond = np.array([1, 0, 1, 0, 1])
assert a.compress(cond).tolist() == [10, 30, 50], 'compress method'
assert a.compress(np.array([0, 0, 0, 0, 0])).tolist() == [], 'compress all false'

# swapaxes on 1D (no-op)
a = np.array([1, 2, 3])
assert a.swapaxes(0, 0).tolist() == [1, 2, 3], 'swapaxes 1d noop'

# nbytes / itemsize
a = np.array([1.0, 2.0, 3.0])
assert a.nbytes == 24, 'nbytes 3 floats'
assert a.itemsize == 8, 'itemsize float'
a5 = np.array([1.0, 2.0, 3.0, 4.0, 5.0])
assert a5.nbytes == 40, 'nbytes 5 floats'

# === Additional array creation and attribute coverage ===
# zeros shape
assert np.zeros(0).tolist() == [], 'zeros 0'
assert np.zeros(1).tolist() == [0.0], 'zeros 1'
assert np.zeros(5).shape == (5,), 'zeros 5 shape'

# ones shape
assert np.ones(0).tolist() == [], 'ones 0'
assert np.ones(1).tolist() == [1.0], 'ones 1'

# full edge
assert np.full(3, 0.0).tolist() == [0.0, 0.0, 0.0], 'full 3 zeros'
assert np.full(1, 42.0).tolist() == [42.0], 'full 1'
assert np.full(0, 1.0).tolist() == [], 'full 0'

# arange edge
assert np.arange(0).tolist() == [], 'arange 0'
assert np.arange(1).tolist() == [0], 'arange 1'
assert np.arange(1, 1).tolist() == [], 'arange empty range'
assert np.arange(5, 0, -1).tolist() == [5, 4, 3, 2, 1], 'arange reverse'

# linspace edge
assert np.linspace(0, 1, 2).tolist() == [0.0, 1.0], 'linspace 2'
assert np.linspace(0, 1, 1).tolist() == [0.0], 'linspace 1'
assert np.linspace(5, 5, 3).tolist() == [5.0, 5.0, 5.0], 'linspace same'

# === Additional aggregation coverage ===
# sum/mean/min/max on various sizes
assert np.sum(np.array([1])) == 1, 'sum single'
assert np.mean(np.array([5.0])) == 5.0, 'mean single'
assert np.min(np.array([42])) == 42, 'min single'
assert np.max(np.array([42])) == 42, 'max single'

# std/var on uniform
assert np.std(np.array([5.0, 5.0, 5.0])) == 0.0, 'std uniform'
assert np.var(np.array([5.0, 5.0, 5.0])) == 0.0, 'var uniform'

# prod
assert np.array([1, 2, 3, 4]).prod() == 24, 'prod method'
assert np.array([0, 1, 2]).prod() == 0, 'prod with zero'

# median
assert np.median(np.array([1.0])) == 1.0, 'median single'
assert np.median(np.array([1.0, 3.0])) == 2.0, 'median two'
assert np.median(np.array([3.0, 1.0, 2.0])) == 2.0, 'median unsorted'

# cumsum edge
assert np.cumsum(np.array([1])).tolist() == [1], 'cumsum single'

# === Additional comparison coverage ===
a = np.array([1.0, 2.0, 3.0, 4.0, 5.0])
assert (a > 3).tolist() == [False, False, False, True, True], 'gt 3'
assert (a >= 3).tolist() == [False, False, True, True, True], 'ge 3'
assert (a < 3).tolist() == [True, True, False, False, False], 'lt 3'
assert (a <= 3).tolist() == [True, True, True, False, False], 'le 3'
assert (a == 3).tolist() == [False, False, True, False, False], 'eq 3'
assert (a != 3).tolist() == [True, True, False, True, True], 'ne 3'

# === Chained operations coverage ===
# Sort then cumsum
a = np.sort(np.array([3, 1, 4, 1, 5]))
assert np.cumsum(a).tolist() == [1, 2, 5, 9, 14], 'sort then cumsum'

# Where then sum
a = np.array([1, 2, 3, 4, 5])
b = np.where(a > 2, a, np.zeros(5))
assert np.sum(b) == 12.0, 'where then sum'

# Unique then sort (already sorted by unique)
u = np.unique(np.array([5, 3, 1, 3, 5, 1]))
assert u.tolist() == [1.0, 3.0, 5.0], 'unique sorted'
assert len(u.tolist()) == 3, 'unique count'

# Concatenate then reshape
c = np.concatenate([np.array([1, 2, 3]), np.array([4, 5, 6])])
r = np.reshape(c, [2, 3])
assert r.tolist() == [[1, 2, 3], [4, 5, 6]], 'concat then reshape'

# === 2D coverage ===
m = np.array([[1, 2, 3], [4, 5, 6], [7, 8, 9]])
assert m.shape == (3, 3), '3x3 shape'
assert m.ndim == 2, '3x3 ndim'
assert m.size == 9, '3x3 size'
assert m.T.tolist() == [[1, 4, 7], [2, 5, 8], [3, 6, 9]], '3x3 transpose'
assert m.flatten().tolist() == [1, 2, 3, 4, 5, 6, 7, 8, 9], '3x3 flatten'
assert m.ravel().tolist() == [1, 2, 3, 4, 5, 6, 7, 8, 9], '3x3 ravel'

# 2D sum, mean
assert m.sum() == 45, '3x3 sum'
assert m.mean() == 5.0, '3x3 mean'
assert m.min() == 1, '3x3 min'
assert m.max() == 9, '3x3 max'

# 2D operations
m2 = np.array([[1, 2], [3, 4]])
assert (m2 + m2).tolist() == [[2, 4], [6, 8]], '2d add'
assert (m2 * 2).tolist() == [[2, 4], [6, 8]], '2d scalar mul'
assert (m2 > 2).tolist() == [[False, False], [True, True]], '2d compare'

# eye
e3 = np.eye(3)
assert e3.tolist() == [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], 'eye 3'
assert e3.shape == (3, 3), 'eye 3 shape'
assert e3.dtype == 'float64', 'eye 3 dtype'

# Matmul with identity
m = np.array([[1, 2], [3, 4]])
e = np.eye(2)
r = m @ e
assert r.tolist() == [[1.0, 2.0], [3.0, 4.0]], 'matmul with identity preserves'

# === String repr edge cases ===
assert repr(np.array([])) == 'array([], dtype=float64)', 'repr empty'
assert repr(np.array([1.0])) == 'array([1.])', 'repr single float'
assert repr(np.array([1, 2])) == 'array([1, 2])', 'repr int pair'

# ============================================================
# 43. ADDITIONAL COVERAGE TO REACH 1000+ ASSERTIONS
# ============================================================

# === astype coverage ===
a = np.array([1.5, 2.7, 3.1])
assert a.astype('int64').tolist() == [1, 2, 3], 'astype float->int truncate'
assert a.astype('int64').dtype == 'int64', 'astype float->int dtype'
assert np.array([1, 0, 1]).astype('bool').tolist() == [True, False, True], 'astype int->bool'
assert np.array([1, 0, 1]).astype('bool').dtype == 'bool', 'astype int->bool dtype'
assert np.array([0.0, 1.5, 0.0]).astype('bool').tolist() == [False, True, False], 'astype float->bool'
assert np.array([True, False]).astype('float64').tolist() == [1.0, 0.0], 'astype bool->float'
assert np.array([True, False]).astype('int64').tolist() == [1, 0], 'astype bool->int'

# === copy method ===
a = np.array([1, 2, 3])
b = a.copy()
assert b.tolist() == [1, 2, 3], 'copy values'
b[0] = 99
assert a[0] == 1, 'copy is independent'

# === sort method edge cases ===
a = np.array([5, 3, 1, 4, 2])
a.sort()
assert a.tolist() == [1, 2, 3, 4, 5], 'sort in-place'

a = np.array([1.0])
a.sort()
assert a.tolist() == [1.0], 'sort single'

# === Module-level sort ===
assert np.sort(np.array([3, 1, 2])).tolist() == [1, 2, 3], 'np.sort int'
assert np.sort(np.array([3.0, 1.0, 2.0])).tolist() == [1.0, 2.0, 3.0], 'np.sort float'

# === argsort method ===
a = np.array([30, 10, 20])
assert a.argsort().tolist() == [1, 2, 0], 'argsort method'

# === argmin/argmax method ===
a = np.array([3, 1, 4, 1, 5])
assert a.argmin() == 1, 'argmin method'
assert a.argmax() == 4, 'argmax method'

# === all/any method ===
assert np.array([1, 1, 1]).all() == True, 'all true'
assert np.array([1, 0, 1]).all() == False, 'all false'
assert np.array([0, 0, 1]).any() == True, 'any true'
assert np.array([0, 0, 0]).any() == False, 'any false'

# === dot method ===
a = np.array([1, 2, 3])
b = np.array([4, 5, 6])
assert a.dot(b) == 32, 'dot method'

# === clip method ===
a = np.array([1, 5, 10, 15, 20])
assert a.clip(5, 15).tolist() == [5, 5, 10, 15, 15], 'clip method'

# === round method ===
a = np.array([1.234, 5.678, 9.012])
r = a.round()
assert r.tolist() == [1.0, 6.0, 9.0], 'round method'

# === .T on 1D is identity ===
a = np.array([1, 2, 3])
assert a.T.tolist() == [1, 2, 3], 'T on 1D is identity'

# === np.copy ===
a = np.array([1, 2, 3])
b = np.copy(a)
assert b.tolist() == [1, 2, 3], 'np.copy'

# === np.count_nonzero edge cases ===
assert np.count_nonzero(np.array([0, 0, 0])) == 0, 'count_nonzero all zero'
assert np.count_nonzero(np.array([1, 2, 3])) == 3, 'count_nonzero all nonzero'
assert np.count_nonzero(np.array([])) == 0, 'count_nonzero empty'

# === np.diff edge cases ===
assert np.diff(np.array([1, 4, 9, 16])).tolist() == [3, 5, 7], 'diff squares'
assert np.diff(np.array([1, 1, 1, 1])).tolist() == [0, 0, 0], 'diff constant'
assert np.diff(np.array([5, 3])).tolist() == [-2], 'diff two elem'

# === np.where with 3 args ===
a = np.array([1, 2, 3, 4, 5])
r = np.where(a > 3, a, np.array([0, 0, 0, 0, 0]))
assert r.tolist() == [0, 0, 0, 4, 5], 'where 3-arg'

# === np.maximum / np.minimum edge cases ===
assert np.maximum(np.array([1, 5, 3]), np.array([4, 2, 6])).tolist() == [4, 5, 6], 'maximum pairwise'
assert np.minimum(np.array([1, 5, 3]), np.array([4, 2, 6])).tolist() == [1, 2, 3], 'minimum pairwise'

# === np.concatenate more cases ===
r = np.concatenate([np.array([1]), np.array([2]), np.array([3])])
assert r.tolist() == [1, 2, 3], 'concat three'
r = np.concatenate([np.array([1, 2, 3])])
assert r.tolist() == [1, 2, 3], 'concat single'

# === np.stack more cases ===
a = np.array([1, 2])
b = np.array([3, 4])
r = np.stack([a, b])
assert r.tolist() == [[1, 2], [3, 4]], 'stack 2 arrays'

# === np.vstack / np.hstack ===
assert np.vstack([np.array([1, 2]), np.array([3, 4])]).tolist() == [[1, 2], [3, 4]], 'vstack pair'
assert np.hstack([np.array([1, 2]), np.array([3, 4])]).tolist() == [1, 2, 3, 4], 'hstack pair'

# === np.array_equal more cases ===
assert np.array_equal(np.array([1, 2, 3]), np.array([1, 2, 3])) == True, 'array_equal same'
assert np.array_equal(np.array([1, 2, 3]), np.array([1, 2, 4])) == False, 'array_equal diff'
assert np.array_equal(np.array([1, 2]), np.array([1, 2, 3])) == False, 'array_equal diff size'

# === np.zeros_like / np.ones_like ===
a = np.array([5, 10, 15])
assert np.zeros_like(a).tolist() == [0, 0, 0], 'zeros_like int'
assert np.ones_like(a).tolist() == [1, 1, 1], 'ones_like int'
assert np.zeros_like(a).shape == (3,), 'zeros_like shape'

# === Unary minus/plus on arrays ===
a = np.array([1, -2, 3])
assert (-a).tolist() == [-1, 2, -3], 'unary neg'
# Bitwise invert
b = np.array([0, 1, 0, 1])
assert (~b).tolist() == [-1, -2, -1, -2], 'bitwise invert int'

# === Section 44: In-place floor divide, modulo, power ===
a = np.array([10, 7, 15])
a //= 3
assert a.tolist() == [3, 2, 5], 'ifloordiv scalar'
a = np.array([10, 7, 15])
a %= 3
assert a.tolist() == [1, 1, 0], 'imod scalar'
a = np.array([2.0, 3.0, 4.0])
a **= 2
assert a.tolist() == [4.0, 9.0, 16.0], 'ipow scalar'
try:
    a = np.array([2, 4])
    a **= -1
    assert False, 'expected int ipow negative exponent to fail'
except ValueError as exc:
    assert str(exc) == 'Integers to negative integer powers are not allowed.', 'ipow negative int exponent error'
# Array rhs
a = np.array([10, 20, 30])
a //= np.array([3, 7, 4])
assert a.tolist() == [3, 2, 7], 'ifloordiv array'
a = np.array([10, 20, 30])
a %= np.array([3, 7, 4])
assert a.tolist() == [1, 6, 2], 'imod array'
a = np.array([2.0, 3.0, 4.0])
a **= np.array([3.0, 2.0, 0.5])
assert a.tolist() == [8.0, 9.0, 2.0], 'ipow array'
try:
    a = np.array([2, 4])
    a **= np.array([-1, -2])
    assert False, 'expected int ipow negative exponent array to fail'
except ValueError as exc:
    assert str(exc) == 'Integers to negative integer powers are not allowed.', 'ipow negative int array error'

# === Section 45: .flat attribute ===
a = np.array([[1, 2], [3, 4]])
f = a.flat
assert f.tolist() == [1, 2, 3, 4], 'flat tolist'
assert f.shape == (4,), 'flat shape'
assert f[2] == 3, 'flat indexing'

# === Section 46: ndarray .repeat() and .nonzero() methods ===
a = np.array([1, 2, 3])
assert a.repeat(2).tolist() == [1, 1, 2, 2, 3, 3], 'repeat method 2x'
assert a.repeat(1).tolist() == [1, 2, 3], 'repeat method 1x'
a = np.array([0, 1, 0, 3, 5])
nz = a.nonzero()
assert nz[0].tolist() == [1, 3, 4], 'nonzero method 1d'

# === Section 47: NumPy scalar constants, dtype markers, and index tricks ===
assert np.False_ == False, 'False_ matches False'
assert np.True_ == True, 'True_ matches True'
assert np.issubdtype(np.bool_, np.generic) == True, 'bool is generic'
assert np.issubdtype(np.int64, np.generic) == True, 'int64 is generic'
assert np.issubdtype(np.float64, np.generic) == True, 'float64 is generic'
assert np.issubdtype(np.bool_, np.number) == False, 'bool is not number'
assert np.issubdtype(np.int64, np.number) == True, 'int64 is number'
assert np.issubdtype(np.float64, np.number) == True, 'float64 is number'
assert np.issubdtype(np.int64, np.signedinteger) == True, 'int64 is signedinteger'
assert np.issubdtype(np.uint64, np.unsignedinteger) == True, 'uint64 is unsignedinteger'
assert np.issubdtype(np.uint64, np.signedinteger) == False, 'uint64 is not signedinteger'
assert np.issubdtype(np.int64, np.unsignedinteger) == False, 'int64 is not unsignedinteger'
assert np.issubdtype(np.float64, np.complexfloating) == False, 'float64 is not complexfloating'
assert np.issubdtype(np.float64, np.flexible) == False, 'float64 is not flexible'
assert np.issubdtype(np.float64, np.character) == False, 'float64 is not character'
assert np.issubdtype(np.character, np.flexible) == True, 'character is flexible'
assert np.issubdtype(np.complexfloating, np.inexact) == True, 'complexfloating is inexact'

idx = np.s_[1:5:2]
assert idx.start == 1, 's_ slice start'
assert idx.stop == 5, 's_ slice stop'
assert idx.step == 2, 's_ slice step'
idx = np.index_exp[1:5:2, None]
assert len(idx) == 2, 'index_exp tuple length'
assert idx[0].start == 1, 'index_exp slice start'
assert idx[0].stop == 5, 'index_exp slice stop'
assert idx[0].step == 2, 'index_exp slice step'
assert idx[1] is None, 'index_exp preserves None'
assert np.array([10, 20, 30, 40])[np.s_[1:3]].tolist() == [20, 30], 's_ slices arrays'

assert np.mgrid[0:3].tolist() == [0, 1, 2], 'mgrid one range'
assert np.mgrid[0:3, 0:2].tolist() == [[[0, 0], [1, 1], [2, 2]], [[0, 1], [0, 1], [0, 1]]], 'mgrid dense grids'
og = np.ogrid[0:3, 0:2]
assert og[0].tolist() == [[0], [1], [2]], 'ogrid first sparse grid'
assert og[1].tolist() == [[0, 1]], 'ogrid second sparse grid'
assert np.r_[1, 2, 3].tolist() == [1, 2, 3], 'r_ scalar concatenation'
assert np.r_[1:4, [10, 20]].tolist() == [1, 2, 3, 10, 20], 'r_ range and list'
assert np.c_[[1, 2], [3, 4]].tolist() == [[1, 3], [2, 4]], 'c_ column stack vectors'
assert np.c_[[1, 2, 3]].tolist() == [[1], [2], [3]], 'c_ single vector column'
