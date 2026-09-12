import math

# === Constants ===
assert math.pi == 3.141592653589793
assert math.e == 2.718281828459045
assert math.tau == 6.283185307179586
assert math.inf == float('inf')
assert math.nan != math.nan
assert math.isinf(math.inf)
assert math.isnan(math.nan)

# === math.floor() ===
assert math.floor(2.3) == 2
assert math.floor(-2.3) == -3
assert math.floor(2.0) == 2
assert math.floor(5) == 5
assert math.floor(True) == 1
assert math.floor(False) == 0
assert math.floor(-0.5) == -1
assert math.floor(0.9) == 0
assert math.floor(1e18) == 1000000000000000000

threw = False
try:
    math.floor(float('inf'))
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.floor(float('nan'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.floor('x')
except TypeError:
    threw = True
assert threw

# === math.ceil() ===
assert math.ceil(2.3) == 3
assert math.ceil(-2.3) == -2
assert math.ceil(2.0) == 2
assert math.ceil(5) == 5
assert math.ceil(True) == 1
assert math.ceil(False) == 0
assert math.ceil(0.1) == 1
assert math.ceil(-0.1) == 0

threw = False
try:
    math.ceil(float('inf'))
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.ceil(float('nan'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.ceil('x')
except TypeError:
    threw = True
assert threw

# === math.trunc() ===
assert math.trunc(2.7) == 2
assert math.trunc(-2.7) == -2
assert math.trunc(2.0) == 2
assert math.trunc(5) == 5
assert math.trunc(True) == 1
assert math.trunc(False) == 0

threw = False
try:
    math.trunc(float('inf'))
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.trunc(float('nan'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.trunc('x')
except TypeError:
    threw = True
assert threw

# === math.sqrt() ===
assert math.sqrt(4) == 2.0
assert math.sqrt(2) == 1.4142135623730951
assert math.sqrt(0) == 0.0
assert math.sqrt(1) == 1.0
assert math.sqrt(0.25) == 0.5
assert isinstance(math.sqrt(4), float)
assert math.sqrt(True) == 1.0
assert math.sqrt(False) == 0.0
assert math.sqrt(float('inf')) == float('inf')
assert math.isnan(math.sqrt(float('nan')))

threw = False
try:
    math.sqrt(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.sqrt('x')
except TypeError:
    threw = True
assert threw

# === math.isqrt() ===
assert math.isqrt(0) == 0
assert math.isqrt(1) == 1
assert math.isqrt(4) == 2
assert math.isqrt(10) == 3
assert math.isqrt(99) == 9
assert math.isqrt(100) == 10
assert math.isqrt(True) == 1

threw = False
try:
    math.isqrt(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.isqrt(4.0)
except TypeError:
    threw = True
assert threw

# === math.cbrt() ===
assert math.cbrt(0) == 0.0
assert math.cbrt(8) == 2.0
assert math.cbrt(-8) == -2.0
assert math.cbrt(1) == 1.0
assert math.cbrt(64) == 4.0
assert math.cbrt(float('inf')) == float('inf')
assert math.cbrt(float('-inf')) == float('-inf')
assert math.isnan(math.cbrt(float('nan')))

threw = False
try:
    math.cbrt('x')
except TypeError:
    threw = True
assert threw

# === math.pow() ===
assert math.pow(2, 3) == 8.0
assert math.pow(2.0, 0.5) == math.sqrt(2)
assert math.pow(0, 0) == 1.0
assert isinstance(math.pow(2, 3), float)
assert math.pow(2, -1) == 0.5
assert math.pow(float('inf'), 0) == 1.0
assert math.pow(float('nan'), 0) == 1.0
assert math.pow(1, float('inf')) == 1.0
assert math.pow(1, float('nan')) == 1.0

threw = False
try:
    math.pow(0, -1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.pow(-1, 0.5)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.pow(2, 1024)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.pow('x', 2)
except TypeError:
    threw = True
assert threw

# === math.exp() ===
assert math.exp(0) == 1.0
assert math.exp(1) == math.e
assert math.exp(float('-inf')) == 0.0
assert math.exp(float('inf')) == float('inf')
assert math.isnan(math.exp(float('nan')))

threw = False
try:
    math.exp(1000)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.exp('x')
except TypeError:
    threw = True
assert threw

# === math.exp2() ===
assert math.exp2(0) == 1.0
assert math.exp2(3) == 8.0
assert math.exp2(10) == 1024.0
assert math.exp2(float('-inf')) == 0.0
assert math.exp2(float('inf')) == float('inf')
assert math.isnan(math.exp2(float('nan')))

threw = False
try:
    math.exp2(1024)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.exp2('x')
except TypeError:
    threw = True
assert threw

# === math.expm1() ===
assert math.expm1(0) == 0.0
assert math.isclose(math.expm1(1), math.e - 1)
assert math.expm1(1e-15) != 0.0
assert math.expm1(float('-inf')) == -1.0
assert math.expm1(float('inf')) == float('inf')
assert math.isnan(math.expm1(float('nan')))

threw = False
try:
    math.expm1(1000)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.expm1('x')
except TypeError:
    threw = True
assert threw

# === math.fabs() ===
assert math.fabs(-5) == 5.0
assert math.fabs(5) == 5.0
assert math.fabs(-3.14) == 3.14
assert math.fabs(0) == 0.0
assert isinstance(math.fabs(-5), float)
assert isinstance(math.fabs(0), float)
assert math.fabs(True) == 1.0
assert math.fabs(False) == 0.0
assert math.fabs(float('inf')) == float('inf')
assert math.fabs(float('-inf')) == float('inf')
assert math.isnan(math.fabs(float('nan')))

threw = False
try:
    math.fabs('x')
except TypeError:
    threw = True
assert threw

# === math.isnan() ===
assert math.isnan(float('nan')) == True
assert math.isnan(1.0) == False
assert math.isnan(0.0) == False
assert math.isnan(float('inf')) == False
assert math.isnan(0) == False
assert math.isnan(True) == False
assert math.isnan(False) == False

threw = False
try:
    math.isnan('x')
except TypeError:
    threw = True
assert threw

# === math.isinf() ===
assert math.isinf(float('inf')) == True
assert math.isinf(float('-inf')) == True
assert math.isinf(1.0) == False
assert math.isinf(float('nan')) == False
assert math.isinf(0) == False
assert math.isinf(True) == False
assert math.isinf(False) == False

threw = False
try:
    math.isinf('x')
except TypeError:
    threw = True
assert threw

# === math.isfinite() ===
assert math.isfinite(1.0) == True
assert math.isfinite(0) == True
assert math.isfinite(float('inf')) == False
assert math.isfinite(float('-inf')) == False
assert math.isfinite(float('nan')) == False
assert math.isfinite(True) == True
assert math.isfinite(False) == True

threw = False
try:
    math.isfinite('x')
except TypeError:
    threw = True
assert threw

# === math.copysign() ===
assert math.copysign(1.0, -0.0) == -1.0
assert math.copysign(-1.0, 1.0) == 1.0
assert math.copysign(5, -3) == -5.0
assert isinstance(math.copysign(5, -3), float)
assert math.copysign(float('inf'), -1.0) == float('-inf')
assert math.copysign(0.0, -1.0) == -0.0
assert math.isnan(math.copysign(float('nan'), -1.0))
assert math.copysign(True, -1) == -1.0

threw = False
try:
    math.copysign('x', 1)
except TypeError:
    threw = True
assert threw

# === math.isclose() ===
assert math.isclose(1.0, 1.0) == True
assert math.isclose(1.0, 1.0000000001) == True
assert math.isclose(1.0, 1.1) == False
assert math.isclose(0.0, 0.0) == True
assert math.isclose(-0.0, 0.0) == True
assert math.isclose(float('inf'), float('inf')) == True
assert math.isclose(float('inf'), 1e308) == False
assert math.isclose(float('nan'), float('nan')) == False
assert math.isclose(1e-15, 0.0) == False
assert math.isclose(0.0, 1e-15) == False

threw = False
try:
    math.isclose('x', 1)
except TypeError:
    threw = True
assert threw

# === math.log() ===
assert math.log(1) == 0.0
assert math.log(math.e) == 1.0
assert math.log(100, 10) == 2.0
assert math.log(1, 10) == 0.0
assert math.log(True) == 0.0
assert math.log(float('inf')) == float('inf')
assert math.isnan(math.log(float('nan')))
assert math.isnan(math.log(float('nan'), 2))
assert math.log(float('inf'), 2) == float('inf')

threw = False
try:
    math.log(0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log(10, 1)
except ZeroDivisionError:
    threw = True
assert threw

threw = False
try:
    math.log(10, 0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log(10, -1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log('x')
except TypeError:
    threw = True
assert threw

# === math.log2() ===
assert math.log2(1) == 0.0
assert math.log2(8) == 3.0
assert math.log2(1024) == 10.0
assert math.log2(True) == 0.0
assert math.log2(float('inf')) == float('inf')
assert math.isnan(math.log2(float('nan')))

threw = False
try:
    math.log2(0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log2(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log2('x')
except TypeError:
    threw = True
assert threw

# === math.log10() ===
assert math.log10(1) == 0.0
assert math.log10(1000) == 3.0
assert math.log10(100) == 2.0
assert math.log10(True) == 0.0
assert math.log10(float('inf')) == float('inf')
assert math.isnan(math.log10(float('nan')))

threw = False
try:
    math.log10(0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log10(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log10('x')
except TypeError:
    threw = True
assert threw

# === math.log1p() ===
assert math.log1p(0) == 0.0
assert math.isclose(math.log1p(math.e - 1), 1.0)
assert math.log1p(float('inf')) == float('inf')
assert math.isnan(math.log1p(float('nan')))

threw = False
try:
    math.log1p(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log1p(-2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.log1p('x')
except TypeError:
    threw = True
assert threw

# === math.factorial() ===
assert math.factorial(0) == 1
assert math.factorial(1) == 1
assert math.factorial(5) == 120
assert math.factorial(10) == 3628800
assert math.factorial(20) == 2432902008176640000
assert math.factorial(True) == 1
assert math.factorial(False) == 1

threw = False
try:
    math.factorial(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.factorial(1.5)
except TypeError:
    threw = True
assert threw

threw = False
try:
    math.factorial('x')
except TypeError:
    threw = True
assert threw

# === math.gcd() ===
assert math.gcd(12, 8) == 4
assert math.gcd(0, 5) == 5
assert math.gcd(5, 0) == 5
assert math.gcd(0, 0) == 0
assert math.gcd(-12, 8) == 4
assert math.gcd(12, -8) == 4
assert math.gcd(-12, -8) == 4
assert math.gcd(7, 13) == 1
assert math.gcd(True, 2) == 1
assert math.gcd(False, 5) == 5

threw = False
try:
    math.gcd(1.5, 2)
except TypeError:
    threw = True
assert threw

threw = False
try:
    math.gcd(2, 1.5)
except TypeError:
    threw = True
assert threw

# === math.lcm() ===
assert math.lcm(4, 6) == 12
assert math.lcm(0, 5) == 0
assert math.lcm(5, 0) == 0
assert math.lcm(0, 0) == 0
assert math.lcm(3, 7) == 21
assert math.lcm(6, 6) == 6
assert math.lcm(-4, 6) == 12
assert math.lcm(-4, -6) == 12
assert math.lcm(True, 2) == 2
assert math.lcm(False, 5) == 0

threw = False
try:
    math.lcm(1.5, 2)
except TypeError:
    threw = True
assert threw

threw = False
try:
    math.lcm(2, 1.5)
except TypeError:
    threw = True
assert threw

# === math.comb() ===
assert math.comb(5, 2) == 10
assert math.comb(10, 0) == 1
assert math.comb(10, 10) == 1
assert math.comb(0, 0) == 1
assert math.comb(5, 6) == 0

threw = False
try:
    math.comb(5, -1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.comb(-1, 2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.comb(5.0, 2)
except TypeError:
    threw = True
assert threw

# === math.perm() ===
assert math.perm(5, 2) == 20
assert math.perm(5, 0) == 1
assert math.perm(5, 5) == 120
assert math.perm(5, 6) == 0

threw = False
try:
    math.perm(5, -1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.perm(-1, 2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.perm(5.0, 2)
except TypeError:
    threw = True
assert threw

# === math.copysign() (already above) ===

# === math.isclose() (already above) ===

# === math.degrees() ===
assert math.degrees(0) == 0.0
assert math.degrees(math.pi) == 180.0
assert math.degrees(math.tau) == 360.0
assert math.degrees(True) == math.degrees(1)
assert math.degrees(float('inf')) == float('inf')
assert math.degrees(float('-inf')) == float('-inf')
assert math.isnan(math.degrees(float('nan')))

threw = False
try:
    math.degrees('x')
except TypeError:
    threw = True
assert threw

# === math.radians() ===
assert math.radians(0) == 0.0
assert math.radians(180) == math.pi
assert math.radians(360) == math.tau
assert math.radians(True) == math.radians(1)
assert math.radians(float('inf')) == float('inf')
assert math.radians(float('-inf')) == float('-inf')
assert math.isnan(math.radians(float('nan')))

threw = False
try:
    math.radians('x')
except TypeError:
    threw = True
assert threw

# === math.sin() ===
assert math.sin(0) == 0.0
assert math.sin(math.pi / 2) == 1.0
assert math.sin(math.pi) < 1e-15
assert math.isnan(math.sin(float('nan')))

threw = False
try:
    math.sin(float('inf'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.sin(float('-inf'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.sin('x')
except TypeError:
    threw = True
assert threw

# === math.cos() ===
assert math.cos(0) == 1.0
assert abs(math.cos(math.pi / 2)) < 1e-15
assert math.cos(math.pi) == -1.0
assert math.isnan(math.cos(float('nan')))

threw = False
try:
    math.cos(float('inf'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.cos(float('-inf'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.cos('x')
except TypeError:
    threw = True
assert threw

# === math.tan() ===
assert math.tan(0) == 0.0
assert abs(math.tan(math.pi / 4) - 1.0) < 1e-15
assert math.isnan(math.tan(float('nan')))

threw = False
try:
    math.tan(float('inf'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.tan(float('-inf'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.tan('x')
except TypeError:
    threw = True
assert threw

# === math.asin() ===
assert math.asin(0) == 0.0
assert math.asin(1) == math.pi / 2
assert math.asin(-1) == -math.pi / 2
assert math.isnan(math.asin(float('nan')))

threw = False
try:
    math.asin(2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.asin(-2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.asin('x')
except TypeError:
    threw = True
assert threw

# === math.acos() ===
assert math.acos(1) == 0.0
assert math.acos(0) == math.pi / 2
assert math.acos(-1) == math.pi
assert math.isnan(math.acos(float('nan')))

threw = False
try:
    math.acos(2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.acos(-2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.acos('x')
except TypeError:
    threw = True
assert threw

# === math.atan() ===
assert math.atan(0) == 0.0
assert math.atan(1) == math.pi / 4
assert math.atan(float('inf')) == math.pi / 2
assert math.atan(float('-inf')) == -math.pi / 2
assert math.isnan(math.atan(float('nan')))

threw = False
try:
    math.atan('x')
except TypeError:
    threw = True
assert threw

# === math.atan2() ===
assert math.atan2(0, 1) == 0.0
assert math.atan2(1, 0) == math.pi / 2
assert math.atan2(0, -1) == math.pi
assert math.atan2(0, 0) == 0.0
assert math.atan2(-1, 0) == -math.pi / 2
assert math.isclose(math.atan2(float('inf'), float('inf')), math.pi / 4)
assert math.isnan(math.atan2(float('nan'), 1))
assert math.isnan(math.atan2(1, float('nan')))

threw = False
try:
    math.atan2('x', 1)
except TypeError:
    threw = True
assert threw

# === math.sinh() ===
assert math.sinh(0) == 0.0
assert math.isclose(math.sinh(1), 1.1752011936438014)
assert math.sinh(float('inf')) == float('inf')
assert math.sinh(float('-inf')) == float('-inf')
assert math.isnan(math.sinh(float('nan')))

threw = False
try:
    math.sinh(1000)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.sinh('x')
except TypeError:
    threw = True
assert threw

# === math.cosh() ===
assert math.cosh(0) == 1.0
assert math.isclose(math.cosh(1), 1.5430806348152437)
assert math.cosh(float('inf')) == float('inf')
assert math.cosh(float('-inf')) == float('inf')
assert math.isnan(math.cosh(float('nan')))

threw = False
try:
    math.cosh(1000)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.cosh('x')
except TypeError:
    threw = True
assert threw

# === math.tanh() ===
assert math.tanh(0) == 0.0
assert math.tanh(float('inf')) == 1.0
assert math.tanh(float('-inf')) == -1.0
assert math.tanh(1) == 0.7615941559557649
assert math.isnan(math.tanh(float('nan')))

threw = False
try:
    math.tanh('x')
except TypeError:
    threw = True
assert threw

# === math.asinh() ===
assert math.asinh(0) == 0.0
assert math.isclose(math.asinh(1), 0.881373587019543)
assert math.asinh(float('inf')) == float('inf')
assert math.asinh(float('-inf')) == float('-inf')
assert math.isnan(math.asinh(float('nan')))

threw = False
try:
    math.asinh('x')
except TypeError:
    threw = True
assert threw

# === math.acosh() ===
assert math.acosh(1) == 0.0
assert math.isclose(math.acosh(2), 1.3169578969248166)
assert math.acosh(float('inf')) == float('inf')
assert math.isnan(math.acosh(float('nan')))

threw = False
try:
    math.acosh(0.5)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.acosh('x')
except TypeError:
    threw = True
assert threw

# === math.atanh() ===
assert math.atanh(0) == 0.0
assert math.isclose(math.atanh(0.5), 0.5493061443340549)
assert math.isnan(math.atanh(float('nan')))

threw = False
try:
    math.atanh(1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.atanh(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.atanh('x')
except TypeError:
    threw = True
assert threw

# === math.fmod() ===
assert math.fmod(10, 3) == 1.0
assert math.fmod(-10, 3) == -1.0
assert math.fmod(10.5, 3) == 1.5
assert math.fmod(3, float('inf')) == 3.0
assert math.isnan(math.fmod(float('nan'), 3))
assert math.isnan(math.fmod(3, float('nan')))
assert math.isnan(math.fmod(float('nan'), float('nan')))

threw = False
try:
    math.fmod(10, 0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.fmod(float('inf'), 3)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.fmod('x', 3)
except TypeError:
    threw = True
assert threw

# === math.remainder() ===
assert math.remainder(10, 3) == 1.0
assert math.remainder(10, 4) == 2.0
assert math.remainder(-10, 3) == -1.0
assert math.remainder(10.5, 3) == -1.5
assert math.remainder(3, float('inf')) == 3.0
assert math.isnan(math.remainder(float('nan'), 3))
assert math.isnan(math.remainder(3, float('nan')))

threw = False
try:
    math.remainder(10, 0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.remainder(float('inf'), 3)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.remainder('x', 3)
except TypeError:
    threw = True
assert threw

# === math.modf() ===
r = math.modf(3.5)
assert r == (0.5, 3.0)
r = math.modf(-3.5)
assert r == (-0.5, -3.0)
r = math.modf(0.0)
assert r == (0.0, 0.0)
r = math.modf(float('inf'))
assert r == (0.0, float('inf'))
r = math.modf(float('-inf'))
# modf(-inf) returns (-0.0, -inf), verify both parts including sign of fractional
assert str(r[0]) == '-0.0'
assert r[1] == float('-inf')
r_nan = math.modf(float('nan'))
assert math.isnan(r_nan[0]) and math.isnan(r_nan[1]), 'modf(nan) both parts are nan'

threw = False
try:
    math.modf('x')
except TypeError:
    threw = True
assert threw

# === math.frexp() ===
r = math.frexp(0.0)
assert r == (0.0, 0)
r = math.frexp(3.5)
assert r == (0.875, 2)
r = math.frexp(1.0)
assert r == (0.5, 1)
r = math.frexp(-1.0)
assert r == (-0.5, 1)
r = math.frexp(float('inf'))
assert r == (float('inf'), 0)
r = math.frexp(float('-inf'))
assert r == (float('-inf'), 0)
r_nan = math.frexp(float('nan'))
assert math.isnan(r_nan[0]) and r_nan[1] == 0, 'frexp(nan)'

threw = False
try:
    math.frexp('x')
except TypeError:
    threw = True
assert threw

# === math.ldexp() ===
assert math.ldexp(0.875, 2) == 3.5
assert math.ldexp(1.0, 0) == 1.0
assert math.ldexp(0.5, 1) == 1.0
assert math.ldexp(1.0, -1075) == 0.0
assert math.ldexp(float('inf'), 1) == float('inf')
assert math.isnan(math.ldexp(float('nan'), 1))
assert math.ldexp(0.0, 1000) == 0.0

threw = False
try:
    math.ldexp(1.0, 1075)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.ldexp(0.5, 1025)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.ldexp('x', 1)
except TypeError:
    threw = True
assert threw

# === math.gamma() ===
assert math.gamma(1) == 1.0
assert math.gamma(5) == 24.0
assert math.isclose(math.gamma(0.5), math.sqrt(math.pi))
assert math.gamma(float('inf')) == float('inf')
assert math.isnan(math.gamma(float('nan')))

threw = False
try:
    math.gamma(0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.gamma(-1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.gamma(float('-inf'))
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.gamma(172)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    math.gamma('x')
except TypeError:
    threw = True
assert threw

# === math.lgamma() ===
assert math.lgamma(1) == 0.0
assert math.isclose(math.lgamma(5), math.log(24))
assert math.lgamma(float('inf')) == float('inf')
assert math.isnan(math.lgamma(float('nan')))
assert math.isclose(math.lgamma(-0.5), 1.265512123484645)

threw = False
try:
    math.lgamma(0)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.lgamma(-2)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.lgamma('x')
except TypeError:
    threw = True
assert threw

# === math.erf() ===
assert math.erf(0) == 0.0
assert math.isclose(math.erf(1), 0.8427007929497148, rel_tol=1e-15)
assert math.isclose(math.erf(-1), -0.8427007929497148, rel_tol=1e-15)
assert math.erf(float('inf')) == 1.0
assert math.erf(float('-inf')) == -1.0
assert math.isnan(math.erf(float('nan')))

threw = False
try:
    math.erf('x')
except TypeError:
    threw = True
assert threw

# === math.erfc() ===
assert math.erfc(0) == 1.0
assert math.isclose(math.erfc(1), 1.0 - math.erf(1))
assert math.erfc(float('inf')) == 0.0
assert math.erfc(float('-inf')) == 2.0
assert math.isnan(math.erfc(float('nan')))

threw = False
try:
    math.erfc('x')
except TypeError:
    threw = True
assert threw

# === math.nextafter() ===
r = math.nextafter(1.0, 2.0)
assert r > 1.0
assert r == 1.0000000000000002
r = math.nextafter(1.0, 0.0)
assert r < 1.0
assert math.nextafter(0.0, 1.0) == 5e-324
assert math.nextafter(0.0, -1.0) == -5e-324
assert math.isnan(math.nextafter(float('nan'), 1.0))
assert math.isnan(math.nextafter(1.0, float('nan')))
assert math.nextafter(float('inf'), float('inf')) == float('inf')
assert math.nextafter(1.0, 1.0) == 1.0

threw = False
try:
    math.nextafter('x', 1.0)
except TypeError:
    threw = True
assert threw

# === math.ulp() ===
assert math.ulp(1.0) == 2.220446049250313e-16
assert math.ulp(-1.0) == 2.220446049250313e-16
assert math.ulp(0.0) == 5e-324
assert math.isinf(math.ulp(float('inf')))
assert math.isnan(math.ulp(float('nan')))
assert math.ulp(5e-324) == 5e-324

threw = False
try:
    math.ulp('x')
except TypeError:
    threw = True
assert threw

# === Additional edge cases for coverage ===

# --- frexp subnormal numbers ---
r = math.frexp(5e-324)
assert r == (0.5, -1073)

# --- ldexp large negative exponent (underflow to zero) ---
assert math.ldexp(1.0, -2000) == 0.0

# --- fmod NaN propagation edge cases ---
assert math.isnan(math.fmod(float('inf'), float('nan')))
assert math.isnan(math.fmod(float('nan'), 0))

# --- gamma negative non-integer (reflection formula) ---
assert math.isclose(math.gamma(-0.5), -3.544907701811032)
assert math.isclose(math.gamma(-1.5), 2.3632718012073544)

# --- lgamma(-inf) returns inf ---
assert math.lgamma(float('-inf')) == float('inf')

# --- lgamma overflow for extremely large input ---
threw = False
try:
    math.lgamma(1e308)
except OverflowError:
    threw = True
assert threw

# --- lgamma negative non-integer (reflection formula) ---
assert math.isclose(math.lgamma(-0.5), 1.265512123484645)

# ==========================================================
# Tests for bug fixes and CPython behavior alignment
# ==========================================================

# === floor/ceil/trunc with large floats (LongInt promotion) ===
large_floor = math.floor(1e300)
assert large_floor > 0
assert (
    large_floor
    == 1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160
)

large_ceil = math.ceil(-1e300)
assert large_ceil < 0
assert (
    large_ceil
    == -1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160
)

large_trunc = math.trunc(1e300)
assert large_trunc == math.floor(1e300)
large_trunc_neg = math.trunc(-1e300)
assert large_trunc_neg == math.ceil(-1e300)

# floor/ceil should still work normally for values within i64 range
assert math.floor(1e18) == 1000000000000000000
assert math.floor(2.7) == 2
assert math.ceil(-2.7) == -2

# === ldexp with large exponent but small x ===
assert math.ldexp(5e-324, 1075) == 2.0
assert math.ldexp(0.5, 1024) == 8.98846567431158e307

# === modf(-0.0) sign preservation ===
frac, integer = math.modf(-0.0)
# Both parts should be -0.0
assert str(frac) == '-0.0'
assert str(integer) == '-0.0'

# === erfc accuracy for large x ===
erfc_6 = math.erfc(6)
assert erfc_6 > 0
assert math.isclose(erfc_6, 2.1519736712498913e-17, rel_tol=1e-12)
erfc_neg6 = math.erfc(-6)
assert erfc_neg6 == 2.0
assert math.erfc(0) == 1.0

# === variadic gcd ===
assert math.gcd() == 0
assert math.gcd(12) == 12
assert math.gcd(-12) == 12
assert math.gcd(12, 8) == 4
assert math.gcd(12, 8, 6) == 2

# === variadic lcm ===
assert math.lcm() == 1
assert math.lcm(12) == 12
assert math.lcm(-12) == 12
assert math.lcm(4, 6) == 12
assert math.lcm(4, 6, 10) == 60
assert math.lcm(0, 5) == 0

# === perm with optional k ===
assert math.perm(5) == 120
assert math.perm(5, 2) == 20
assert math.perm(0) == 1

# === isclose with rel_tol/abs_tol kwargs ===
assert math.isclose(1.0, 1.1, rel_tol=0.2) == True
assert math.isclose(1.0, 1.1, abs_tol=0.2) == True
assert math.isclose(1.0, 1.1) == False
assert math.isclose(1.0, 1.0 + 1e-10) == True

# === isclose with a/b as keyword arguments (CPython accepts these) ===
assert math.isclose(a=1.0, b=1.0) == True
assert math.isclose(1.0, b=1.0) == True
assert math.isclose(a=1.0, b=1.1, abs_tol=0.2) == True
# Passing `a` twice (positional + keyword) is a duplicate
threw = False
try:
    math.isclose(1.0, 2.0, a=3.0)
except TypeError:
    threw = True
assert threw

# isclose negative tolerance raises ValueError
threw = False
try:
    math.isclose(1.0, 1.0, rel_tol=-0.1)
except ValueError:
    threw = True
assert threw

threw = False
try:
    math.isclose(1.0, 1.0, abs_tol=-0.1)
except ValueError:
    threw = True
assert threw

# isclose unknown kwarg raises TypeError
threw = False
try:
    math.isclose(1.0, 1.0, foo=0.1)
except TypeError:
    threw = True
assert threw

# === ldexp sign preservation ===
assert str(math.ldexp(-0.0, 1000)) == '-0.0'
assert math.ldexp(float('-inf'), 1) == float('-inf')

# === frexp(-0.0) sign preservation ===
m, e = math.frexp(-0.0)
assert str(m) == '-0.0'
assert e == 0

# === comb with GCD reduction (values that would overflow intermediate without it) ===
assert math.comb(62, 31) == 465428353255261088
assert math.comb(61, 30) == 232714176627630544

# === isclose arg count errors ===
threw = False
try:
    math.isclose()
except TypeError:
    threw = True
assert threw

threw = False
try:
    math.isclose(1.0)
except TypeError:
    threw = True
assert threw

threw = False
try:
    math.isclose(1.0, 2.0, 3.0)
except TypeError:
    threw = True
assert threw

# === perm(-1) single-arg error message ===
threw = False
try:
    math.perm(-1)
except ValueError:
    threw = True
assert threw

# === gcd/lcm with i64::MIN-like values (u64 promotion) ===
# gcd(-9223372036854775808, 0) should return 9223372036854775808 (exceeds i64::MAX)
big_gcd = math.gcd(-9223372036854775808, 0)
assert big_gcd == 9223372036854775808

# === isqrt large values (Newton's method refinement) ===
# Values near i64::MAX where f64 sqrt loses precision
assert math.isqrt(9223372036854775807) == 3037000499
assert math.isqrt(9223372030926249001) == 3037000499
assert math.isqrt(9223372030926249000) == 3037000498

# === erf/erfc range coverage ===
# Small x (|x| < 0.84375): exercises PP/QQ polynomial
assert math.erf(0.1) == 0.1124629160182849
assert math.erf(0.5) == 0.5204998778130465

# Medium x (1.25 ≤ |x| < 28): exercises erfc_inner path
assert math.erf(2.0) == 0.9953222650189527
assert math.erf(5.0) == 0.9999999999984626

# erfc in range 3 (1.25 ≤ |x| < 2.857): exercises RA/SA coefficients
erfc_2 = math.erfc(2.0)
assert math.isclose(erfc_2, 0.004677734981047266, rel_tol=1e-12)

# ==========================================================
# Long int arguments
# ==========================================================

# === Real-number functions convert a long int the way float() does ===
big = 2**70
huge = 10**400
assert math.sqrt(big) == 34359738368.0
assert math.fabs(-big) == 1.1805916207174113e21
assert math.copysign(1, -big) == -1.0
assert math.isclose(big, big + 1)
assert math.fmod(big, 3.0) == 1.0
assert math.pow(big, 2) == 1.393796574908164e42
assert math.degrees(big) == 6.764291719561731e22
assert math.ldexp(big, 1) == 2.3611832414348226e21
assert math.floor(big) == big
assert math.floor(-big) == -big
assert math.ceil(-big) == -big
assert math.trunc(big) == big
# `cbrt` differs across libms in the last bit, so this one is checked with a tolerance.
assert math.isclose(math.cbrt(big), 10568983.798516532, rel_tol=1e-15)
assert math.exp(-big) == 0.0
assert math.atan(big) == 1.5707963267948966
assert math.hypot(big, 1) == 1.1805916207174113e21
assert math.modf(big) == (0.0, 1.1805916207174113e21)
assert math.frexp(big) == (0.5, 71)
assert math.remainder(big, 3) == 1.0
assert not math.isnan(big)
for compute in [
    lambda: math.sqrt(huge),
    lambda: math.log1p(huge),
    lambda: math.pow(huge, 0),
    lambda: math.gamma(huge),
    lambda: math.sin(huge),
    lambda: math.isfinite(huge),
]:
    try:
        compute()
        assert False, 'expected OverflowError'
    except OverflowError as e:
        assert str(e) == 'int too large to convert to float'

# === Logarithms of ints beyond the float range ===
assert math.log(big) == 48.520302639196174
assert math.log2(big) == 70.0
assert math.log10(big) == 21.072099696478684
assert math.log(huge) == 921.0340371976182
assert math.log(huge, 10) == 399.99999999999994
assert math.log(huge, 2) == 1328.7712379549448
assert math.log(huge, huge) == 1.0
assert math.log(2, huge) == 0.000752574989159953
assert math.log2(huge) == 1328.771237954945
assert math.log10(huge) == 400.0
assert math.log2(2**1023) == 1023.0
assert math.log2(2**1024) == 1024.0
assert math.log2(2**1024 + 1) == 1024.0
assert math.log2(2**1100 - 1) == 1100.0
assert math.log(2**1100 - 1) == 762.4618986159398
assert math.log10(2**1100 - 1) == 331.1329952303793
# Values chosen so fused and unfused `log(m) + log(2) * e` agree (see limitations/math.md).
assert math.log(7**500) == 972.9550745276566
assert math.log2(3**700) == 1109.4737505048092
assert math.log10(3**700) == 333.9848783037637
for compute, message in [
    (lambda: math.log(), 'log expected at least 1 argument, got 0'),
    (lambda: math.log(2, 10, 3), 'log expected at most 2 arguments, got 3'),
    (lambda: math.perm(), 'perm expected at least 1 argument, got 0'),
    (lambda: math.perm(1, 2, 3), 'perm expected at most 2 arguments, got 3'),
]:
    try:
        compute()
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == message
for compute in [lambda: math.log(huge, 1), lambda: math.log(huge, True)]:
    try:
        compute()
        assert False, 'expected ZeroDivisionError'
    except ZeroDivisionError as e:
        assert str(e) == 'division by zero'
for compute in [
    lambda: math.log(-huge),
    lambda: math.log(10, -huge),
    lambda: math.log2(-5),
    lambda: math.log(0),
    lambda: math.log(False),
]:
    try:
        compute()
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == 'expected a positive input'
# A float argument names the offending value.
for compute, message in [
    (lambda: math.log(0.0), 'expected a positive input, got 0.0'),
    (lambda: math.log2(0.0), 'expected a positive input, got 0.0'),
    (lambda: math.log10(-1.0), 'expected a positive input, got -1.0'),
    (lambda: math.log(float('-inf')), 'expected a positive input, got -inf'),
    (lambda: math.log(10, 0.0), 'expected a positive input, got 0.0'),
]:
    try:
        compute()
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == message

# === Integer functions accept and return long ints ===
assert math.isqrt(big) == 34359738368
assert math.isqrt(huge) == 10**200
assert math.isqrt(huge + 1) == 10**200
assert math.isqrt(huge - 1) == 10**200 - 1
# The machine-int fast path ends at 2**63; the big path starts there.
assert math.isqrt(2**63 - 1) == 3037000499
assert math.isqrt(2**63) == 3037000499
assert math.isqrt(2**64) == 4294967296
assert math.isqrt(2**128 - 1) == 18446744073709551615
assert math.isqrt(2**200) == 2**100
try:
    math.isqrt(-big)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'isqrt() argument must be nonnegative'

assert math.gcd(big, 2**40) == 2**40
assert math.gcd(big * 3, big * 5) == big
assert math.gcd(big) == big
assert math.gcd(-big, -big) == big
assert math.gcd(big, 0) == big
assert math.gcd(0, big) == big
try:
    math.gcd(big, 2.0)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'float' object cannot be interpreted as an integer"
assert math.lcm(big) == big
assert math.lcm(-big) == big
assert math.lcm(big, 3) == 3541774862152233910272
assert math.lcm(2**40, 3**30) == 226379693794030958489370624
assert math.lcm(2**62, 3) == 3 * 2**62
assert math.lcm(-big, 3) == 3541774862152233910272
assert math.lcm(big, 0) == 0
assert math.lcm(big, 0, big) == 0
assert math.lcm(huge, 0) == 0
for compute in [lambda: math.lcm(0, 2.0), lambda: math.lcm(big, 0, 2.0)]:
    try:
        compute()
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == "'float' object cannot be interpreted as an integer"

assert math.factorial(21) == 51090942171709440000
assert math.factorial(30) == 265252859812191058636308480000000
assert len(str(math.factorial(1000))) == 2568
# CPython's limit is C `long`, which is 32 bits on Windows; Monty always uses the 64-bit limit.
FACTORIAL_LIMIT_MESSAGES = {
    'factorial() argument should not exceed 9223372036854775807',
    'factorial() argument should not exceed 2147483647',
}
for compute in [lambda: math.factorial(big), lambda: math.factorial(2**63), lambda: math.perm(big)]:
    try:
        compute()
        assert False, 'expected OverflowError'
    except OverflowError as e:
        assert str(e) in FACTORIAL_LIMIT_MESSAGES
try:
    math.factorial(-big)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'factorial() not defined for negative values'

assert math.comb(100, 50) == 100891344545564193334812497256
assert math.comb(big, 2) == 696898287454081973172400900209902591410176
assert math.comb(big, big) == 1
assert math.comb(big, big - 1) == big
assert math.comb(5, big) == 0
assert len(str(math.comb(1000, 500))) == 300
for compute, exc_type, message in [
    (lambda: math.comb(big, big // 2), OverflowError, 'min(n - k, k) must not exceed 9223372036854775807'),
    (lambda: math.comb(-big, 1), ValueError, 'n must be a non-negative integer'),
    (lambda: math.comb(5, -big), ValueError, 'k must be a non-negative integer'),
    (lambda: math.perm(big, big // 2), OverflowError, 'k must not exceed 9223372036854775807'),
    (lambda: math.perm(-big), ValueError, 'factorial() not defined for negative values'),
    (lambda: math.comb(3.0, 1), TypeError, "'float' object cannot be interpreted as an integer"),
    (lambda: math.perm(3, 1.0), TypeError, "'float' object cannot be interpreted as an integer"),
]:
    try:
        compute()
        assert False, 'expected an exception'
    except exc_type as e:
        assert str(e) == message
assert math.perm(30) == 265252859812191058636308480000000
assert math.perm(25) == 15511210043330985984000000
assert math.perm(30, 20) == 73096577329197271449600000
assert math.perm(big, 2) == 1393796574908163946344801800419805182820352
assert math.perm(5, big) == 0
assert len(str(math.perm(1000, 500))) == 1434
# Small and boundary arguments on the big-int paths.
assert math.comb(50, 25) == 126410606437752
assert math.comb(big, 0) == 1
assert math.perm(big, 0) == 1
assert math.perm(big, 1) == big
assert math.comb(0, 0) == 1
assert math.comb(True, True) == 1
assert math.comb(2**63, 1) == 2**63
assert math.perm(2**63, 1) == 2**63
assert math.comb(2**63, 2) == 42535295865117307928310139910543638528
assert math.perm(2**63, 2) == 85070591730234615856620279821087277056

# === ldexp saturates a long int exponent ===
assert math.ldexp(1.0, -big) == 0.0
assert math.ldexp(1.0, -(2**31)) == 0.0
assert math.ldexp(0.0, big) == 0.0
assert math.ldexp(float('inf'), big) == float('inf')
assert math.ldexp(big, -70) == 1.0
for compute in [lambda: math.ldexp(1.0, big), lambda: math.ldexp(1.0, 2**31), lambda: math.ldexp(True, big)]:
    try:
        compute()
        assert False, 'expected OverflowError'
    except OverflowError as e:
        assert str(e) == 'math range error'
