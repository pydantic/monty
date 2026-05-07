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


# === low-risk real and integer math ufuncs ===
assert np.copysign(-2, 3) == 2.0, 'copysign scalar result'
assert np.copysign([-2, 3], [1, -1]).tolist() == [2.0, -3.0], 'copysign lists'

mantissa, exponent = np.frexp(np.array([0.0, 8.0, -6.0]))
assert mantissa.tolist() == [0.0, 0.5, -0.75], 'frexp mantissas'
assert exponent.tolist() == [0, 4, 3], 'frexp exponents'
scalar_mantissa, scalar_exponent = np.frexp(8.0)
assert scalar_mantissa == 0.5, 'frexp scalar mantissa'
assert scalar_exponent == 4, 'frexp scalar exponent'

fractional, integral = np.modf([-2.75, 3.25])
assert fractional.tolist() == [-0.75, 0.25], 'modf fractional parts'
assert integral.tolist() == [-2.0, 3.0], 'modf integral parts'
scalar_fractional, scalar_integral = np.modf(-2.75)
assert scalar_fractional == -0.75, 'modf scalar fractional'
assert scalar_integral == -2.0, 'modf scalar integral'

assert np.ldexp(0.5, 3) == 4.0, 'ldexp scalar'
assert np.ldexp([0.5, -1.5], [3, 2]).tolist() == [4.0, -6.0], 'ldexp lists'
assert np.gcd(-12, 18) == 6, 'gcd scalar'
assert np.gcd([12, -18], [8, 12]).tolist() == [4, 6], 'gcd lists'
assert np.gcd(True, 4) == 1, 'gcd bool scalar'
assert np.lcm(-4, 6) == 12, 'lcm scalar'
assert np.lcm([-4, 6], [6, 8]).tolist() == [12, 24], 'lcm lists'

logadd = np.logaddexp([0.0, 1.0], [0.0, 2.0])
assert abs(logadd[0] - 0.6931471805599453) < 1e-12, 'logaddexp equal inputs'
assert abs(logadd[1] - 2.313261687518223) < 1e-12, 'logaddexp offset inputs'
logadd2 = np.logaddexp2([0.0, 1.0], [0.0, 2.0])
assert logadd2[0] == 1.0, 'logaddexp2 equal inputs'
assert abs(logadd2[1] - 2.584962500721156) < 1e-12, 'logaddexp2 offset inputs'

assert np.nextafter(0.0, 1.0) == 5e-324, 'nextafter smallest subnormal'
assert np.nextafter([1.0], [2.0]).tolist() == [1.0000000000000002], 'nextafter lists'
assert np.spacing([0.0, 1.0, -1.0]).tolist() == [
    5e-324,
    2.220446049250313e-16,
    -2.220446049250313e-16,
], 'spacing signs'
assert np.signbit(np.array([0.0, -0.0, -2.0, 3.0])).tolist() == [
    False,
    True,
    True,
    False,
], 'signbit array'

sinc_result = np.sinc([0.0, 0.5, 1.0])
assert sinc_result[0] == 1.0, 'sinc zero'
assert abs(sinc_result[1] - 0.6366197723675814) < 1e-12, 'sinc half'
assert abs(sinc_result[2]) < 1e-12, 'sinc one'
assert np.heaviside([-2.0, 0.0, 3.0], 0.5).tolist() == [0.0, 0.5, 1.0], 'heaviside list'
assert np.trunc([-2.75, 3.25]).tolist() == [-2.0, 3.0], 'trunc list'
assert np.fix([-2.75, 3.25]).tolist() == [-2.0, 3.0], 'fix list'
assert np.float_power([2, 4], [-1, 0.5]).tolist() == [0.5, 2.0], 'float_power lists'

quotient, remainder = np.divmod(np.array([-3, 4]), np.array([2, 3]))
assert quotient.tolist() == [-2, 1], 'divmod quotient array'
assert remainder.tolist() == [1, 1], 'divmod remainder array'
scalar_quotient, scalar_remainder = np.divmod(7, 3)
assert scalar_quotient == 2, 'divmod scalar quotient'
assert scalar_remainder == 1, 'divmod scalar remainder'


# === window generators and Bessel i0 ===
assert np.bartlett(0).tolist() == [], 'bartlett zero length'
assert np.bartlett(-3).tolist() == [], 'bartlett negative length'
assert np.bartlett(1).tolist() == [1.0], 'bartlett singleton'
assert np.bartlett(5).tolist() == [0.0, 0.5, 1.0, 0.5, 0.0], 'bartlett values'

blackman = np.blackman(5)
assert abs(blackman[0]) < 1e-12, 'blackman first'
assert abs(blackman[1] - 0.34) < 1e-12, 'blackman second'
assert abs(blackman[2] - 1.0) < 1e-12, 'blackman center'

hamming = np.hamming(5)
assert abs(hamming[0] - 0.08) < 1e-12, 'hamming first'
assert abs(hamming[1] - 0.54) < 1e-12, 'hamming second'
assert hamming[2] == 1.0, 'hamming center'
hanning = np.hanning(5)
assert abs(hanning[0]) < 1e-12, 'hanning first'
assert abs(hanning[1] - 0.5) < 1e-12, 'hanning second'
assert hanning[2] == 1.0, 'hanning center'
assert abs(hanning[3] - 0.5) < 1e-12, 'hanning fourth'
assert abs(hanning[4]) < 1e-12, 'hanning last'

kaiser = np.kaiser(5, 2.0)
assert abs(kaiser[0] - 0.4386762798370488) < 1e-7, 'kaiser first'
assert abs(kaiser[1] - 0.8347614334011666) < 1e-7, 'kaiser second'
assert kaiser[2] == 1.0, 'kaiser center'

assert np.i0(0.0) == 1.0, 'i0 zero'
assert abs(np.i0(1.0) - 1.2660658777520082) < 1e-7, 'i0 scalar'
assert abs(np.i0([0.0, 2.0])[1] - 2.279585302336067) < 1e-7, 'i0 list'
