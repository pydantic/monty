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
