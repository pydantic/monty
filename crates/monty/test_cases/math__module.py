import math

# === Constants ===
assert math.pi == 3.141592653589793, 'math.pi value'
assert math.e == 2.718281828459045, 'math.e value'
assert math.tau == 6.283185307179586, 'math.tau value'
assert math.inf == float('inf'), 'math.inf is infinity'
assert math.nan != math.nan, 'math.nan is NaN (not equal to itself)'
assert math.isinf(math.inf), 'math.inf is recognized by isinf'
assert math.isnan(math.nan), 'math.nan is recognized by isnan'

# === math.floor() ===
assert math.floor(2.3) == 2, 'floor(2.3)'
assert math.floor(-2.3) == -3, 'floor(-2.3)'
assert math.floor(2.0) == 2, 'floor(2.0)'
assert math.floor(5) == 5, 'floor(int)'
assert math.floor(True) == 1, 'floor(True)'
assert math.floor(False) == 0, 'floor(False)'
assert math.floor(-0.5) == -1, 'floor(-0.5)'
assert math.floor(0.9) == 0, 'floor(0.9)'
assert math.floor(1e18) == 1000000000000000000, 'floor(1e18)'

threw = False
try:
    math.floor(float('inf'))
except OverflowError:
    threw = True
assert threw, 'floor(inf) raises OverflowError'

threw = False
try:
    math.floor(float('nan'))
except ValueError:
    threw = True
assert threw, 'floor(nan) raises ValueError'

threw = False
try:
    math.floor('x')
except TypeError:
    threw = True
assert threw, 'floor(str) raises TypeError'

# === math.ceil() ===
assert math.ceil(2.3) == 3, 'ceil(2.3)'
assert math.ceil(-2.3) == -2, 'ceil(-2.3)'
assert math.ceil(2.0) == 2, 'ceil(2.0)'
assert math.ceil(5) == 5, 'ceil(int)'
assert math.ceil(True) == 1, 'ceil(True)'
assert math.ceil(False) == 0, 'ceil(False)'
assert math.ceil(0.1) == 1, 'ceil(0.1)'
assert math.ceil(-0.1) == 0, 'ceil(-0.1)'

threw = False
try:
    math.ceil(float('inf'))
except OverflowError:
    threw = True
assert threw, 'ceil(inf) raises OverflowError'

threw = False
try:
    math.ceil(float('nan'))
except ValueError:
    threw = True
assert threw, 'ceil(nan) raises ValueError'

threw = False
try:
    math.ceil('x')
except TypeError:
    threw = True
assert threw, 'ceil(str) raises TypeError'

# === math.trunc() ===
assert math.trunc(2.7) == 2, 'trunc(2.7)'
assert math.trunc(-2.7) == -2, 'trunc(-2.7)'
assert math.trunc(2.0) == 2, 'trunc(2.0)'
assert math.trunc(5) == 5, 'trunc(int)'
assert math.trunc(True) == 1, 'trunc(True)'
assert math.trunc(False) == 0, 'trunc(False)'

threw = False
try:
    math.trunc(float('inf'))
except OverflowError:
    threw = True
assert threw, 'trunc(inf) raises OverflowError'

threw = False
try:
    math.trunc(float('nan'))
except ValueError:
    threw = True
assert threw, 'trunc(nan) raises ValueError'

threw = False
try:
    math.trunc('x')
except TypeError:
    threw = True
assert threw, 'trunc(str) raises TypeError'

# === math.sqrt() ===
assert math.sqrt(4) == 2.0, 'sqrt(4)'
assert math.sqrt(2) == 1.4142135623730951, 'sqrt(2)'
assert math.sqrt(0) == 0.0, 'sqrt(0)'
assert math.sqrt(1) == 1.0, 'sqrt(1)'
assert math.sqrt(0.25) == 0.5, 'sqrt(0.25)'
assert isinstance(math.sqrt(4), float), 'sqrt returns float'
assert math.sqrt(True) == 1.0, 'sqrt(True)'
assert math.sqrt(False) == 0.0, 'sqrt(False)'
assert math.sqrt(float('inf')) == float('inf'), 'sqrt(inf) returns inf'
assert math.isnan(math.sqrt(float('nan'))), 'sqrt(nan) returns nan'

threw = False
try:
    math.sqrt(-1)
except ValueError:
    threw = True
assert threw, 'sqrt(-1) raises ValueError'

threw = False
try:
    math.sqrt('x')
except TypeError:
    threw = True
assert threw, 'sqrt(str) raises TypeError'

# === math.isqrt() ===
assert math.isqrt(0) == 0, 'isqrt(0)'
assert math.isqrt(1) == 1, 'isqrt(1)'
assert math.isqrt(4) == 2, 'isqrt(4)'
assert math.isqrt(10) == 3, 'isqrt(10)'
assert math.isqrt(99) == 9, 'isqrt(99)'
assert math.isqrt(100) == 10, 'isqrt(100)'
assert math.isqrt(True) == 1, 'isqrt(True)'

threw = False
try:
    math.isqrt(-1)
except ValueError:
    threw = True
assert threw, 'isqrt(-1) raises ValueError'

threw = False
try:
    math.isqrt(4.0)
except TypeError:
    threw = True
assert threw, 'isqrt(float) raises TypeError'

# === math.cbrt() ===
assert math.cbrt(0) == 0.0, 'cbrt(0)'
assert math.cbrt(8) == 2.0, 'cbrt(8)'
assert math.cbrt(-8) == -2.0, 'cbrt(-8)'
assert math.cbrt(1) == 1.0, 'cbrt(1)'
assert math.cbrt(64) == 4.0, 'cbrt(64)'
assert math.cbrt(float('inf')) == float('inf'), 'cbrt(inf)'
assert math.cbrt(float('-inf')) == float('-inf'), 'cbrt(-inf)'
assert math.isnan(math.cbrt(float('nan'))), 'cbrt(nan) is nan'

threw = False
try:
    math.cbrt('x')
except TypeError:
    threw = True
assert threw, 'cbrt(str) raises TypeError'

# === math.pow() ===
assert math.pow(2, 3) == 8.0, 'pow(2, 3)'
assert math.pow(2.0, 0.5) == math.sqrt(2), 'pow(2, 0.5)'
assert math.pow(0, 0) == 1.0, 'pow(0, 0)'
assert isinstance(math.pow(2, 3), float), 'pow returns float'
assert math.pow(2, -1) == 0.5, 'pow(2, -1)'
assert math.pow(float('inf'), 0) == 1.0, 'pow(inf, 0)'
assert math.pow(float('nan'), 0) == 1.0, 'pow(nan, 0)'
assert math.pow(1, float('inf')) == 1.0, 'pow(1, inf)'
assert math.pow(1, float('nan')) == 1.0, 'pow(1, nan)'

threw = False
try:
    math.pow(0, -1)
except ValueError:
    threw = True
assert threw, 'pow(0, -1) raises ValueError'

threw = False
try:
    math.pow(-1, 0.5)
except ValueError:
    threw = True
assert threw, 'pow(-1, 0.5) raises ValueError'

threw = False
try:
    math.pow(2, 1024)
except OverflowError:
    threw = True
assert threw, 'pow(2, 1024) raises OverflowError'

threw = False
try:
    math.pow('x', 2)
except TypeError:
    threw = True
assert threw, 'pow(str, int) raises TypeError'

# === math.exp() ===
assert math.exp(0) == 1.0, 'exp(0)'
assert math.exp(1) == math.e, 'exp(1)'
assert math.exp(float('-inf')) == 0.0, 'exp(-inf)'
assert math.exp(float('inf')) == float('inf'), 'exp(inf)'
assert math.isnan(math.exp(float('nan'))), 'exp(nan) is nan'

threw = False
try:
    math.exp(1000)
except OverflowError:
    threw = True
assert threw, 'exp(1000) raises OverflowError'

threw = False
try:
    math.exp('x')
except TypeError:
    threw = True
assert threw, 'exp(str) raises TypeError'

# === math.exp2() ===
assert math.exp2(0) == 1.0, 'exp2(0)'
assert math.exp2(3) == 8.0, 'exp2(3)'
assert math.exp2(10) == 1024.0, 'exp2(10)'
assert math.exp2(float('-inf')) == 0.0, 'exp2(-inf)'
assert math.exp2(float('inf')) == float('inf'), 'exp2(inf)'
assert math.isnan(math.exp2(float('nan'))), 'exp2(nan) is nan'

threw = False
try:
    math.exp2(1024)
except OverflowError:
    threw = True
assert threw, 'exp2(1024) raises OverflowError'

threw = False
try:
    math.exp2('x')
except TypeError:
    threw = True
assert threw, 'exp2(str) raises TypeError'

# === math.expm1() ===
assert math.expm1(0) == 0.0, 'expm1(0)'
assert math.isclose(math.expm1(1), math.e - 1), 'expm1(1)'
assert math.expm1(1e-15) != 0.0, 'expm1(1e-15) is precise'
assert math.expm1(float('-inf')) == -1.0, 'expm1(-inf)'
assert math.expm1(float('inf')) == float('inf'), 'expm1(inf)'
assert math.isnan(math.expm1(float('nan'))), 'expm1(nan) is nan'

threw = False
try:
    math.expm1(1000)
except OverflowError:
    threw = True
assert threw, 'expm1(1000) raises OverflowError'

threw = False
try:
    math.expm1('x')
except TypeError:
    threw = True
assert threw, 'expm1(str) raises TypeError'

# === math.fabs() ===
assert math.fabs(-5) == 5.0, 'fabs(-5)'
assert math.fabs(5) == 5.0, 'fabs(5)'
assert math.fabs(-3.14) == 3.14, 'fabs(-3.14)'
assert math.fabs(0) == 0.0, 'fabs(0)'
assert isinstance(math.fabs(-5), float), 'fabs returns float'
assert isinstance(math.fabs(0), float), 'fabs(0) returns float'
assert math.fabs(True) == 1.0, 'fabs(True)'
assert math.fabs(False) == 0.0, 'fabs(False)'
assert math.fabs(float('inf')) == float('inf'), 'fabs(inf)'
assert math.fabs(float('-inf')) == float('inf'), 'fabs(-inf)'
assert math.isnan(math.fabs(float('nan'))), 'fabs(nan) returns nan'

threw = False
try:
    math.fabs('x')
except TypeError:
    threw = True
assert threw, 'fabs(str) raises TypeError'

# === math.isnan() ===
assert math.isnan(float('nan')) == True, 'isnan(nan)'
assert math.isnan(1.0) == False, 'isnan(1.0)'
assert math.isnan(0.0) == False, 'isnan(0.0)'
assert math.isnan(float('inf')) == False, 'isnan(inf)'
assert math.isnan(0) == False, 'isnan(int)'
assert math.isnan(True) == False, 'isnan(True)'
assert math.isnan(False) == False, 'isnan(False)'

threw = False
try:
    math.isnan('x')
except TypeError:
    threw = True
assert threw, 'isnan(str) raises TypeError'

# === math.isinf() ===
assert math.isinf(float('inf')) == True, 'isinf(inf)'
assert math.isinf(float('-inf')) == True, 'isinf(-inf)'
assert math.isinf(1.0) == False, 'isinf(1.0)'
assert math.isinf(float('nan')) == False, 'isinf(nan)'
assert math.isinf(0) == False, 'isinf(int)'
assert math.isinf(True) == False, 'isinf(True)'
assert math.isinf(False) == False, 'isinf(False)'

threw = False
try:
    math.isinf('x')
except TypeError:
    threw = True
assert threw, 'isinf(str) raises TypeError'

# === math.isfinite() ===
assert math.isfinite(1.0) == True, 'isfinite(1.0)'
assert math.isfinite(0) == True, 'isfinite(0)'
assert math.isfinite(float('inf')) == False, 'isfinite(inf)'
assert math.isfinite(float('-inf')) == False, 'isfinite(-inf)'
assert math.isfinite(float('nan')) == False, 'isfinite(nan)'
assert math.isfinite(True) == True, 'isfinite(True)'
assert math.isfinite(False) == True, 'isfinite(False)'

threw = False
try:
    math.isfinite('x')
except TypeError:
    threw = True
assert threw, 'isfinite(str) raises TypeError'

# === math.copysign() ===
assert math.copysign(1.0, -0.0) == -1.0, 'copysign(1.0, -0.0)'
assert math.copysign(-1.0, 1.0) == 1.0, 'copysign(-1.0, 1.0)'
assert math.copysign(5, -3) == -5.0, 'copysign(5, -3)'
assert isinstance(math.copysign(5, -3), float), 'copysign returns float'
assert math.copysign(float('inf'), -1.0) == float('-inf'), 'copysign(inf, -1.0)'
assert math.copysign(0.0, -1.0) == -0.0, 'copysign(0.0, -1.0)'
assert math.isnan(math.copysign(float('nan'), -1.0)), 'copysign(nan, -1.0) is nan'
assert math.copysign(True, -1) == -1.0, 'copysign(True, -1)'

threw = False
try:
    math.copysign('x', 1)
except TypeError:
    threw = True
assert threw, 'copysign(str, int) raises TypeError'

# === math.isclose() ===
assert math.isclose(1.0, 1.0) == True, 'isclose equal'
assert math.isclose(1.0, 1.0000000001) == True, 'isclose very close'
assert math.isclose(1.0, 1.1) == False, 'isclose not close'
assert math.isclose(0.0, 0.0) == True, 'isclose zeros'
assert math.isclose(-0.0, 0.0) == True, 'isclose neg zero and zero'
assert math.isclose(float('inf'), float('inf')) == True, 'isclose(inf, inf)'
assert math.isclose(float('inf'), 1e308) == False, 'isclose(inf, large) is False'
assert math.isclose(float('nan'), float('nan')) == False, 'isclose(nan, nan) is False'
assert math.isclose(1e-15, 0.0) == False, 'isclose(1e-15, 0.0) is False with default abs_tol'
assert math.isclose(0.0, 1e-15) == False, 'isclose(0.0, 1e-15) is False with default abs_tol'

threw = False
try:
    math.isclose('x', 1)
except TypeError:
    threw = True
assert threw, 'isclose(str, int) raises TypeError'

# === math.log() ===
assert math.log(1) == 0.0, 'log(1)'
assert math.log(math.e) == 1.0, 'log(e)'
assert math.log(100, 10) == 2.0, 'log(100, 10)'
assert math.log(1, 10) == 0.0, 'log(1, 10)'
assert math.log(True) == 0.0, 'log(True)'
assert math.log(float('inf')) == float('inf'), 'log(inf) returns inf'
assert math.isnan(math.log(float('nan'))), 'log(nan) returns nan'
assert math.isnan(math.log(float('nan'), 2)), 'log(nan, 2) returns nan'
assert math.log(float('inf'), 2) == float('inf'), 'log(inf, 2) returns inf'

threw = False
try:
    math.log(0)
except ValueError:
    threw = True
assert threw, 'log(0) raises ValueError'

threw = False
try:
    math.log(-1)
except ValueError:
    threw = True
assert threw, 'log(-1) raises ValueError'

threw = False
try:
    math.log(10, 1)
except ZeroDivisionError:
    threw = True
assert threw, 'log(10, 1) raises ZeroDivisionError'

threw = False
try:
    math.log(10, 0)
except ValueError:
    threw = True
assert threw, 'log(10, 0) raises ValueError'

threw = False
try:
    math.log(10, -1)
except ValueError:
    threw = True
assert threw, 'log(10, -1) raises ValueError'

threw = False
try:
    math.log('x')
except TypeError:
    threw = True
assert threw, 'log(str) raises TypeError'

# === math.log2() ===
assert math.log2(1) == 0.0, 'log2(1)'
assert math.log2(8) == 3.0, 'log2(8)'
assert math.log2(1024) == 10.0, 'log2(1024)'
assert math.log2(True) == 0.0, 'log2(True)'
assert math.log2(float('inf')) == float('inf'), 'log2(inf) returns inf'
assert math.isnan(math.log2(float('nan'))), 'log2(nan) returns nan'

threw = False
try:
    math.log2(0)
except ValueError:
    threw = True
assert threw, 'log2(0) raises ValueError'

threw = False
try:
    math.log2(-1)
except ValueError:
    threw = True
assert threw, 'log2(-1) raises ValueError'

threw = False
try:
    math.log2('x')
except TypeError:
    threw = True
assert threw, 'log2(str) raises TypeError'

# === math.log10() ===
assert math.log10(1) == 0.0, 'log10(1)'
assert math.log10(1000) == 3.0, 'log10(1000)'
assert math.log10(100) == 2.0, 'log10(100)'
assert math.log10(True) == 0.0, 'log10(True)'
assert math.log10(float('inf')) == float('inf'), 'log10(inf) returns inf'
assert math.isnan(math.log10(float('nan'))), 'log10(nan) returns nan'

threw = False
try:
    math.log10(0)
except ValueError:
    threw = True
assert threw, 'log10(0) raises ValueError'

threw = False
try:
    math.log10(-1)
except ValueError:
    threw = True
assert threw, 'log10(-1) raises ValueError'

threw = False
try:
    math.log10('x')
except TypeError:
    threw = True
assert threw, 'log10(str) raises TypeError'

# === math.log1p() ===
assert math.log1p(0) == 0.0, 'log1p(0)'
assert math.isclose(math.log1p(math.e - 1), 1.0), 'log1p(e-1)'
assert math.log1p(float('inf')) == float('inf'), 'log1p(inf)'
assert math.isnan(math.log1p(float('nan'))), 'log1p(nan) is nan'

threw = False
try:
    math.log1p(-1)
except ValueError:
    threw = True
assert threw, 'log1p(-1) raises ValueError'

threw = False
try:
    math.log1p(-2)
except ValueError:
    threw = True
assert threw, 'log1p(-2) raises ValueError'

threw = False
try:
    math.log1p('x')
except TypeError:
    threw = True
assert threw, 'log1p(str) raises TypeError'

# === math.factorial() ===
assert math.factorial(0) == 1, 'factorial(0)'
assert math.factorial(1) == 1, 'factorial(1)'
assert math.factorial(5) == 120, 'factorial(5)'
assert math.factorial(10) == 3628800, 'factorial(10)'
assert math.factorial(20) == 2432902008176640000, 'factorial(20)'
assert math.factorial(True) == 1, 'factorial(True)'
assert math.factorial(False) == 1, 'factorial(False)'

threw = False
try:
    math.factorial(-1)
except ValueError:
    threw = True
assert threw, 'factorial(-1) raises ValueError'

threw = False
try:
    math.factorial(1.5)
except TypeError:
    threw = True
assert threw, 'factorial(1.5) raises TypeError'

threw = False
try:
    math.factorial('x')
except TypeError:
    threw = True
assert threw, 'factorial(str) raises TypeError'

# === math.gcd() ===
assert math.gcd(12, 8) == 4, 'gcd(12, 8)'
assert math.gcd(0, 5) == 5, 'gcd(0, 5)'
assert math.gcd(5, 0) == 5, 'gcd(5, 0)'
assert math.gcd(0, 0) == 0, 'gcd(0, 0)'
assert math.gcd(-12, 8) == 4, 'gcd(-12, 8)'
assert math.gcd(12, -8) == 4, 'gcd(12, -8)'
assert math.gcd(-12, -8) == 4, 'gcd(-12, -8)'
assert math.gcd(7, 13) == 1, 'gcd(7, 13) coprime'
assert math.gcd(True, 2) == 1, 'gcd(True, 2)'
assert math.gcd(False, 5) == 5, 'gcd(False, 5)'

threw = False
try:
    math.gcd(1.5, 2)
except TypeError:
    threw = True
assert threw, 'gcd(float, int) raises TypeError'

threw = False
try:
    math.gcd(2, 1.5)
except TypeError:
    threw = True
assert threw, 'gcd(int, float) raises TypeError'

# === math.lcm() ===
assert math.lcm(4, 6) == 12, 'lcm(4, 6)'
assert math.lcm(0, 5) == 0, 'lcm(0, 5)'
assert math.lcm(5, 0) == 0, 'lcm(5, 0)'
assert math.lcm(0, 0) == 0, 'lcm(0, 0)'
assert math.lcm(3, 7) == 21, 'lcm(3, 7) coprime'
assert math.lcm(6, 6) == 6, 'lcm(6, 6) equal'
assert math.lcm(-4, 6) == 12, 'lcm(-4, 6) negative'
assert math.lcm(-4, -6) == 12, 'lcm(-4, -6) both negative'
assert math.lcm(True, 2) == 2, 'lcm(True, 2)'
assert math.lcm(False, 5) == 0, 'lcm(False, 5)'

threw = False
try:
    math.lcm(1.5, 2)
except TypeError:
    threw = True
assert threw, 'lcm(float, int) raises TypeError'

threw = False
try:
    math.lcm(2, 1.5)
except TypeError:
    threw = True
assert threw, 'lcm(int, float) raises TypeError'

# === math.comb() ===
assert math.comb(5, 2) == 10, 'comb(5, 2)'
assert math.comb(10, 0) == 1, 'comb(10, 0)'
assert math.comb(10, 10) == 1, 'comb(10, 10)'
assert math.comb(0, 0) == 1, 'comb(0, 0)'
assert math.comb(5, 6) == 0, 'comb(5, 6) k > n'

threw = False
try:
    math.comb(5, -1)
except ValueError:
    threw = True
assert threw, 'comb(5, -1) raises ValueError'

threw = False
try:
    math.comb(-1, 2)
except ValueError:
    threw = True
assert threw, 'comb(-1, 2) raises ValueError'

threw = False
try:
    math.comb(5.0, 2)
except TypeError:
    threw = True
assert threw, 'comb(float, int) raises TypeError'

# === math.perm() ===
assert math.perm(5, 2) == 20, 'perm(5, 2)'
assert math.perm(5, 0) == 1, 'perm(5, 0)'
assert math.perm(5, 5) == 120, 'perm(5, 5)'
assert math.perm(5, 6) == 0, 'perm(5, 6) k > n'

threw = False
try:
    math.perm(5, -1)
except ValueError:
    threw = True
assert threw, 'perm(5, -1) raises ValueError'

threw = False
try:
    math.perm(-1, 2)
except ValueError:
    threw = True
assert threw, 'perm(-1, 2) raises ValueError'

threw = False
try:
    math.perm(5.0, 2)
except TypeError:
    threw = True
assert threw, 'perm(float, int) raises TypeError'

# === math.copysign() (already above) ===

# === math.isclose() (already above) ===

# === math.degrees() ===
assert math.degrees(0) == 0.0, 'degrees(0)'
assert math.degrees(math.pi) == 180.0, 'degrees(pi)'
assert math.degrees(math.tau) == 360.0, 'degrees(tau)'
assert math.degrees(True) == math.degrees(1), 'degrees(True)'
assert math.degrees(float('inf')) == float('inf'), 'degrees(inf)'
assert math.degrees(float('-inf')) == float('-inf'), 'degrees(-inf)'
assert math.isnan(math.degrees(float('nan'))), 'degrees(nan) is nan'

threw = False
try:
    math.degrees('x')
except TypeError:
    threw = True
assert threw, 'degrees(str) raises TypeError'

# === math.radians() ===
assert math.radians(0) == 0.0, 'radians(0)'
assert math.radians(180) == math.pi, 'radians(180)'
assert math.radians(360) == math.tau, 'radians(360)'
assert math.radians(True) == math.radians(1), 'radians(True)'
assert math.radians(float('inf')) == float('inf'), 'radians(inf)'
assert math.radians(float('-inf')) == float('-inf'), 'radians(-inf)'
assert math.isnan(math.radians(float('nan'))), 'radians(nan) is nan'

threw = False
try:
    math.radians('x')
except TypeError:
    threw = True
assert threw, 'radians(str) raises TypeError'

# === math.sin() ===
assert math.sin(0) == 0.0, 'sin(0)'
assert math.sin(math.pi / 2) == 1.0, 'sin(pi/2)'
assert math.sin(math.pi) < 1e-15, 'sin(pi) near zero'
assert math.isnan(math.sin(float('nan'))), 'sin(nan) is nan'

threw = False
try:
    math.sin(float('inf'))
except ValueError:
    threw = True
assert threw, 'sin(inf) raises ValueError'

threw = False
try:
    math.sin(float('-inf'))
except ValueError:
    threw = True
assert threw, 'sin(-inf) raises ValueError'

threw = False
try:
    math.sin('x')
except TypeError:
    threw = True
assert threw, 'sin(str) raises TypeError'

# === math.cos() ===
assert math.cos(0) == 1.0, 'cos(0)'
assert abs(math.cos(math.pi / 2)) < 1e-15, 'cos(pi/2) near zero'
assert math.cos(math.pi) == -1.0, 'cos(pi)'
assert math.isnan(math.cos(float('nan'))), 'cos(nan) is nan'

threw = False
try:
    math.cos(float('inf'))
except ValueError:
    threw = True
assert threw, 'cos(inf) raises ValueError'

threw = False
try:
    math.cos(float('-inf'))
except ValueError:
    threw = True
assert threw, 'cos(-inf) raises ValueError'

threw = False
try:
    math.cos('x')
except TypeError:
    threw = True
assert threw, 'cos(str) raises TypeError'

# === math.tan() ===
assert math.tan(0) == 0.0, 'tan(0)'
assert abs(math.tan(math.pi / 4) - 1.0) < 1e-15, 'tan(pi/4) near 1'
assert math.isnan(math.tan(float('nan'))), 'tan(nan) is nan'

threw = False
try:
    math.tan(float('inf'))
except ValueError:
    threw = True
assert threw, 'tan(inf) raises ValueError'

threw = False
try:
    math.tan(float('-inf'))
except ValueError:
    threw = True
assert threw, 'tan(-inf) raises ValueError'

threw = False
try:
    math.tan('x')
except TypeError:
    threw = True
assert threw, 'tan(str) raises TypeError'

# === math.asin() ===
assert math.asin(0) == 0.0, 'asin(0)'
assert math.asin(1) == math.pi / 2, 'asin(1)'
assert math.asin(-1) == -math.pi / 2, 'asin(-1)'
assert math.isnan(math.asin(float('nan'))), 'asin(nan) is nan'

threw = False
try:
    math.asin(2)
except ValueError:
    threw = True
assert threw, 'asin(2) raises ValueError'

threw = False
try:
    math.asin(-2)
except ValueError:
    threw = True
assert threw, 'asin(-2) raises ValueError'

threw = False
try:
    math.asin('x')
except TypeError:
    threw = True
assert threw, 'asin(str) raises TypeError'

# === math.acos() ===
assert math.acos(1) == 0.0, 'acos(1)'
assert math.acos(0) == math.pi / 2, 'acos(0)'
assert math.acos(-1) == math.pi, 'acos(-1)'
assert math.isnan(math.acos(float('nan'))), 'acos(nan) is nan'

threw = False
try:
    math.acos(2)
except ValueError:
    threw = True
assert threw, 'acos(2) raises ValueError'

threw = False
try:
    math.acos(-2)
except ValueError:
    threw = True
assert threw, 'acos(-2) raises ValueError'

threw = False
try:
    math.acos('x')
except TypeError:
    threw = True
assert threw, 'acos(str) raises TypeError'

# === math.atan() ===
assert math.atan(0) == 0.0, 'atan(0)'
assert math.atan(1) == math.pi / 4, 'atan(1)'
assert math.atan(float('inf')) == math.pi / 2, 'atan(inf)'
assert math.atan(float('-inf')) == -math.pi / 2, 'atan(-inf)'
assert math.isnan(math.atan(float('nan'))), 'atan(nan) is nan'

threw = False
try:
    math.atan('x')
except TypeError:
    threw = True
assert threw, 'atan(str) raises TypeError'

# === math.atan2() ===
assert math.atan2(0, 1) == 0.0, 'atan2(0, 1)'
assert math.atan2(1, 0) == math.pi / 2, 'atan2(1, 0)'
assert math.atan2(0, -1) == math.pi, 'atan2(0, -1)'
assert math.atan2(0, 0) == 0.0, 'atan2(0, 0)'
assert math.atan2(-1, 0) == -math.pi / 2, 'atan2(-1, 0)'
assert math.isclose(math.atan2(float('inf'), float('inf')), math.pi / 4), 'atan2(inf, inf)'
assert math.isnan(math.atan2(float('nan'), 1)), 'atan2(nan, 1) is nan'
assert math.isnan(math.atan2(1, float('nan'))), 'atan2(1, nan) is nan'

threw = False
try:
    math.atan2('x', 1)
except TypeError:
    threw = True
assert threw, 'atan2(str, int) raises TypeError'

# === math.sinh() ===
assert math.sinh(0) == 0.0, 'sinh(0)'
assert math.isclose(math.sinh(1), 1.1752011936438014), 'sinh(1)'
assert math.sinh(float('inf')) == float('inf'), 'sinh(inf)'
assert math.sinh(float('-inf')) == float('-inf'), 'sinh(-inf)'
assert math.isnan(math.sinh(float('nan'))), 'sinh(nan) is nan'

threw = False
try:
    math.sinh(1000)
except OverflowError:
    threw = True
assert threw, 'sinh(1000) raises OverflowError'

threw = False
try:
    math.sinh('x')
except TypeError:
    threw = True
assert threw, 'sinh(str) raises TypeError'

# === math.cosh() ===
assert math.cosh(0) == 1.0, 'cosh(0)'
assert math.isclose(math.cosh(1), 1.5430806348152437), 'cosh(1)'
assert math.cosh(float('inf')) == float('inf'), 'cosh(inf)'
assert math.cosh(float('-inf')) == float('inf'), 'cosh(-inf)'
assert math.isnan(math.cosh(float('nan'))), 'cosh(nan) is nan'

threw = False
try:
    math.cosh(1000)
except OverflowError:
    threw = True
assert threw, 'cosh(1000) raises OverflowError'

threw = False
try:
    math.cosh('x')
except TypeError:
    threw = True
assert threw, 'cosh(str) raises TypeError'

# === math.tanh() ===
assert math.tanh(0) == 0.0, 'tanh(0)'
assert math.tanh(float('inf')) == 1.0, 'tanh(inf)'
assert math.tanh(float('-inf')) == -1.0, 'tanh(-inf)'
assert math.tanh(1) == 0.7615941559557649, 'tanh(1)'
assert math.isnan(math.tanh(float('nan'))), 'tanh(nan) is nan'

threw = False
try:
    math.tanh('x')
except TypeError:
    threw = True
assert threw, 'tanh(str) raises TypeError'

# === math.asinh() ===
assert math.asinh(0) == 0.0, 'asinh(0)'
assert math.isclose(math.asinh(1), 0.881373587019543), 'asinh(1)'
assert math.asinh(float('inf')) == float('inf'), 'asinh(inf)'
assert math.asinh(float('-inf')) == float('-inf'), 'asinh(-inf)'
assert math.isnan(math.asinh(float('nan'))), 'asinh(nan) is nan'

threw = False
try:
    math.asinh('x')
except TypeError:
    threw = True
assert threw, 'asinh(str) raises TypeError'

# === math.acosh() ===
assert math.acosh(1) == 0.0, 'acosh(1)'
assert math.isclose(math.acosh(2), 1.3169578969248166), 'acosh(2)'
assert math.acosh(float('inf')) == float('inf'), 'acosh(inf)'
assert math.isnan(math.acosh(float('nan'))), 'acosh(nan) is nan'

threw = False
try:
    math.acosh(0.5)
except ValueError:
    threw = True
assert threw, 'acosh(0.5) raises ValueError'

threw = False
try:
    math.acosh('x')
except TypeError:
    threw = True
assert threw, 'acosh(str) raises TypeError'

# === math.atanh() ===
assert math.atanh(0) == 0.0, 'atanh(0)'
assert math.isclose(math.atanh(0.5), 0.5493061443340549), 'atanh(0.5)'
assert math.isnan(math.atanh(float('nan'))), 'atanh(nan) is nan'

threw = False
try:
    math.atanh(1)
except ValueError:
    threw = True
assert threw, 'atanh(1) raises ValueError'

threw = False
try:
    math.atanh(-1)
except ValueError:
    threw = True
assert threw, 'atanh(-1) raises ValueError'

threw = False
try:
    math.atanh('x')
except TypeError:
    threw = True
assert threw, 'atanh(str) raises TypeError'

# === math.fmod() ===
assert math.fmod(10, 3) == 1.0, 'fmod(10, 3)'
assert math.fmod(-10, 3) == -1.0, 'fmod(-10, 3)'
assert math.fmod(10.5, 3) == 1.5, 'fmod(10.5, 3)'
assert math.fmod(3, float('inf')) == 3.0, 'fmod(3, inf)'
assert math.isnan(math.fmod(float('nan'), 3)), 'fmod(nan, 3) is nan'
assert math.isnan(math.fmod(3, float('nan'))), 'fmod(3, nan) is nan'
assert math.isnan(math.fmod(float('nan'), float('nan'))), 'fmod(nan, nan) is nan'

threw = False
try:
    math.fmod(10, 0)
except ValueError:
    threw = True
assert threw, 'fmod(10, 0) raises ValueError'

threw = False
try:
    math.fmod(float('inf'), 3)
except ValueError:
    threw = True
assert threw, 'fmod(inf, 3) raises ValueError'

threw = False
try:
    math.fmod('x', 3)
except TypeError:
    threw = True
assert threw, 'fmod(str, int) raises TypeError'

# === math.remainder() ===
assert math.remainder(10, 3) == 1.0, 'remainder(10, 3)'
assert math.remainder(10, 4) == 2.0, 'remainder(10, 4)'
assert math.remainder(-10, 3) == -1.0, 'remainder(-10, 3)'
assert math.remainder(10.5, 3) == -1.5, 'remainder(10.5, 3)'
assert math.remainder(3, float('inf')) == 3.0, 'remainder(3, inf)'
assert math.isnan(math.remainder(float('nan'), 3)), 'remainder(nan, 3) is nan'
assert math.isnan(math.remainder(3, float('nan'))), 'remainder(3, nan) is nan'

threw = False
try:
    math.remainder(10, 0)
except ValueError:
    threw = True
assert threw, 'remainder(10, 0) raises ValueError'

threw = False
try:
    math.remainder(float('inf'), 3)
except ValueError:
    threw = True
assert threw, 'remainder(inf, 3) raises ValueError'

threw = False
try:
    math.remainder('x', 3)
except TypeError:
    threw = True
assert threw, 'remainder(str, int) raises TypeError'

# === math.modf() ===
r = math.modf(3.5)
assert r == (0.5, 3.0), 'modf(3.5)'
r = math.modf(-3.5)
assert r == (-0.5, -3.0), 'modf(-3.5)'
r = math.modf(0.0)
assert r == (0.0, 0.0), 'modf(0.0)'
r = math.modf(float('inf'))
assert r == (0.0, float('inf')), 'modf(inf)'
r = math.modf(float('-inf'))
# modf(-inf) returns (-0.0, -inf), verify both parts including sign of fractional
assert str(r[0]) == '-0.0', 'modf(-inf) fractional part is -0.0'
assert r[1] == float('-inf'), 'modf(-inf) integer part is -inf'
r_nan = math.modf(float('nan'))
assert math.isnan(r_nan[0]) and math.isnan(r_nan[1]), 'modf(nan) both parts are nan'

threw = False
try:
    math.modf('x')
except TypeError:
    threw = True
assert threw, 'modf(str) raises TypeError'

# === math.frexp() ===
r = math.frexp(0.0)
assert r == (0.0, 0), 'frexp(0.0)'
r = math.frexp(3.5)
assert r == (0.875, 2), 'frexp(3.5)'
r = math.frexp(1.0)
assert r == (0.5, 1), 'frexp(1.0)'
r = math.frexp(-1.0)
assert r == (-0.5, 1), 'frexp(-1.0)'
r = math.frexp(float('inf'))
assert r == (float('inf'), 0), 'frexp(inf)'
r = math.frexp(float('-inf'))
assert r == (float('-inf'), 0), 'frexp(-inf)'
r_nan = math.frexp(float('nan'))
assert math.isnan(r_nan[0]) and r_nan[1] == 0, 'frexp(nan)'

threw = False
try:
    math.frexp('x')
except TypeError:
    threw = True
assert threw, 'frexp(str) raises TypeError'

# === math.ldexp() ===
assert math.ldexp(0.875, 2) == 3.5, 'ldexp(0.875, 2)'
assert math.ldexp(1.0, 0) == 1.0, 'ldexp(1.0, 0)'
assert math.ldexp(0.5, 1) == 1.0, 'ldexp(0.5, 1)'
assert math.ldexp(1.0, -1075) == 0.0, 'ldexp(1.0, -1075) underflows to 0'
assert math.ldexp(float('inf'), 1) == float('inf'), 'ldexp(inf, 1)'
assert math.isnan(math.ldexp(float('nan'), 1)), 'ldexp(nan, 1) is nan'
assert math.ldexp(0.0, 1000) == 0.0, 'ldexp(0.0, 1000)'

threw = False
try:
    math.ldexp(1.0, 1075)
except OverflowError:
    threw = True
assert threw, 'ldexp(1.0, 1075) raises OverflowError'

threw = False
try:
    math.ldexp(0.5, 1025)
except OverflowError:
    threw = True
assert threw, 'ldexp(0.5, 1025) raises OverflowError'

threw = False
try:
    math.ldexp('x', 1)
except TypeError:
    threw = True
assert threw, 'ldexp(str, int) raises TypeError'

# === math.gamma() ===
assert math.gamma(1) == 1.0, 'gamma(1)'
assert math.gamma(5) == 24.0, 'gamma(5)'
assert math.isclose(math.gamma(0.5), math.sqrt(math.pi)), 'gamma(0.5)'
assert math.gamma(float('inf')) == float('inf'), 'gamma(inf)'
assert math.isnan(math.gamma(float('nan'))), 'gamma(nan) is nan'

threw = False
try:
    math.gamma(0)
except ValueError:
    threw = True
assert threw, 'gamma(0) raises ValueError'

threw = False
try:
    math.gamma(-1)
except ValueError:
    threw = True
assert threw, 'gamma(-1) raises ValueError'

threw = False
try:
    math.gamma(float('-inf'))
except ValueError:
    threw = True
assert threw, 'gamma(-inf) raises ValueError'

threw = False
try:
    math.gamma(172)
except OverflowError:
    threw = True
assert threw, 'gamma(172) raises OverflowError'

threw = False
try:
    math.gamma('x')
except TypeError:
    threw = True
assert threw, 'gamma(str) raises TypeError'

# === math.lgamma() ===
assert math.lgamma(1) == 0.0, 'lgamma(1)'
assert math.isclose(math.lgamma(5), math.log(24)), 'lgamma(5)'
assert math.lgamma(float('inf')) == float('inf'), 'lgamma(inf)'
assert math.isnan(math.lgamma(float('nan'))), 'lgamma(nan) is nan'
assert math.isclose(math.lgamma(-0.5), 1.265512123484645), 'lgamma(-0.5)'

threw = False
try:
    math.lgamma(0)
except ValueError:
    threw = True
assert threw, 'lgamma(0) raises ValueError'

threw = False
try:
    math.lgamma(-2)
except ValueError:
    threw = True
assert threw, 'lgamma(-2) raises ValueError'

threw = False
try:
    math.lgamma('x')
except TypeError:
    threw = True
assert threw, 'lgamma(str) raises TypeError'

# === math.erf() ===
assert math.erf(0) == 0.0, 'erf(0)'
assert math.isclose(math.erf(1), 0.8427007929497148, rel_tol=1e-15), 'erf(1)'
assert math.isclose(math.erf(-1), -0.8427007929497148, rel_tol=1e-15), 'erf(-1)'
assert math.erf(float('inf')) == 1.0, 'erf(inf)'
assert math.erf(float('-inf')) == -1.0, 'erf(-inf)'
assert math.isnan(math.erf(float('nan'))), 'erf(nan) is nan'

threw = False
try:
    math.erf('x')
except TypeError:
    threw = True
assert threw, 'erf(str) raises TypeError'

# === math.erfc() ===
assert math.erfc(0) == 1.0, 'erfc(0)'
assert math.isclose(math.erfc(1), 1.0 - math.erf(1)), 'erfc(1)'
assert math.erfc(float('inf')) == 0.0, 'erfc(inf)'
assert math.erfc(float('-inf')) == 2.0, 'erfc(-inf)'
assert math.isnan(math.erfc(float('nan'))), 'erfc(nan) is nan'

threw = False
try:
    math.erfc('x')
except TypeError:
    threw = True
assert threw, 'erfc(str) raises TypeError'

# === math.nextafter() ===
r = math.nextafter(1.0, 2.0)
assert r > 1.0, 'nextafter(1.0, 2.0) > 1.0'
assert r == 1.0000000000000002, 'nextafter(1.0, 2.0) value'
r = math.nextafter(1.0, 0.0)
assert r < 1.0, 'nextafter(1.0, 0.0) < 1.0'
assert math.nextafter(0.0, 1.0) == 5e-324, 'nextafter(0.0, 1.0) smallest positive'
assert math.nextafter(0.0, -1.0) == -5e-324, 'nextafter(0.0, -1.0) smallest negative'
assert math.isnan(math.nextafter(float('nan'), 1.0)), 'nextafter(nan, 1.0) is nan'
assert math.isnan(math.nextafter(1.0, float('nan'))), 'nextafter(1.0, nan) is nan'
assert math.nextafter(float('inf'), float('inf')) == float('inf'), 'nextafter(inf, inf)'
assert math.nextafter(1.0, 1.0) == 1.0, 'nextafter(1.0, 1.0) equal inputs'

threw = False
try:
    math.nextafter('x', 1.0)
except TypeError:
    threw = True
assert threw, 'nextafter(str, float) raises TypeError'

# === math.ulp() ===
assert math.ulp(1.0) == 2.220446049250313e-16, 'ulp(1.0)'
assert math.ulp(-1.0) == 2.220446049250313e-16, 'ulp(-1.0) same as ulp(1.0)'
assert math.ulp(0.0) == 5e-324, 'ulp(0.0) is smallest subnormal'
assert math.isinf(math.ulp(float('inf'))), 'ulp(inf) is inf'
assert math.isnan(math.ulp(float('nan'))), 'ulp(nan) is nan'
assert math.ulp(5e-324) == 5e-324, 'ulp(smallest subnormal)'

threw = False
try:
    math.ulp('x')
except TypeError:
    threw = True
assert threw, 'ulp(str) raises TypeError'

# === Additional edge cases for coverage ===

# --- frexp subnormal numbers ---
r = math.frexp(5e-324)
assert r == (0.5, -1073), 'frexp(5e-324) subnormal'

# --- ldexp large negative exponent (underflow to zero) ---
assert math.ldexp(1.0, -2000) == 0.0, 'ldexp(1.0, -2000) underflows to 0'

# --- fmod NaN propagation edge cases ---
assert math.isnan(math.fmod(float('inf'), float('nan'))), 'fmod(inf, nan) propagates nan'
assert math.isnan(math.fmod(float('nan'), 0)), 'fmod(nan, 0) propagates nan'

# --- gamma negative non-integer (reflection formula) ---
assert math.isclose(math.gamma(-0.5), -3.544907701811032), 'gamma(-0.5)'
assert math.isclose(math.gamma(-1.5), 2.3632718012073544), 'gamma(-1.5)'

# --- lgamma(-inf) returns inf ---
assert math.lgamma(float('-inf')) == float('inf'), 'lgamma(-inf) returns inf'

# --- lgamma overflow for extremely large input ---
threw = False
try:
    math.lgamma(1e308)
except OverflowError:
    threw = True
assert threw, 'lgamma(1e308) raises OverflowError'

# --- lgamma negative non-integer (reflection formula) ---
assert math.isclose(math.lgamma(-0.5), 1.265512123484645), 'lgamma(-0.5) reflection'

# ==========================================================
# Tests for bug fixes and CPython behavior alignment
# ==========================================================

# === floor/ceil/trunc with large floats (LongInt promotion) ===
large_floor = math.floor(1e300)
assert large_floor > 0, 'floor(1e300) should be positive'
assert (
    large_floor
    == 1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160
), 'floor(1e300) matches CPython'

large_ceil = math.ceil(-1e300)
assert large_ceil < 0, 'ceil(-1e300) should be negative'
assert (
    large_ceil
    == -1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160
), 'ceil(-1e300) matches CPython'

large_trunc = math.trunc(1e300)
assert large_trunc == math.floor(1e300), 'trunc(1e300) matches floor(1e300) for positive'
large_trunc_neg = math.trunc(-1e300)
assert large_trunc_neg == math.ceil(-1e300), 'trunc(-1e300) matches ceil(-1e300) for negative'

# floor/ceil should still work normally for values within i64 range
assert math.floor(1e18) == 1000000000000000000, 'floor(1e18) within i64 range'
assert math.floor(2.7) == 2, 'floor(2.7) basic case'
assert math.ceil(-2.7) == -2, 'ceil(-2.7) basic case'

# === ldexp with large exponent but small x ===
assert math.ldexp(5e-324, 1075) == 2.0, 'ldexp(5e-324, 1075) should be 2.0'
assert math.ldexp(0.5, 1024) == 8.98846567431158e307, 'ldexp(0.5, 1024) large but finite'

# === modf(-0.0) sign preservation ===
frac, integer = math.modf(-0.0)
# Both parts should be -0.0
assert str(frac) == '-0.0', 'modf(-0.0) fractional part is -0.0'
assert str(integer) == '-0.0', 'modf(-0.0) integer part is -0.0'

# === erfc accuracy for large x ===
erfc_6 = math.erfc(6)
assert erfc_6 > 0, 'erfc(6) should be positive, not zero'
assert math.isclose(erfc_6, 2.1519736712498913e-17, rel_tol=1e-12), 'erfc(6) matches CPython'
erfc_neg6 = math.erfc(-6)
assert erfc_neg6 == 2.0, 'erfc(-6) is exactly 2.0'
assert math.erfc(0) == 1.0, 'erfc(0) is 1.0'

# === variadic gcd ===
assert math.gcd() == 0, 'gcd() with no args returns 0'
assert math.gcd(12) == 12, 'gcd(12) single arg returns abs(12)'
assert math.gcd(-12) == 12, 'gcd(-12) single arg returns abs(-12)'
assert math.gcd(12, 8) == 4, 'gcd(12, 8) two args'
assert math.gcd(12, 8, 6) == 2, 'gcd(12, 8, 6) three args'

# === variadic lcm ===
assert math.lcm() == 1, 'lcm() with no args returns 1'
assert math.lcm(12) == 12, 'lcm(12) single arg returns abs(12)'
assert math.lcm(-12) == 12, 'lcm(-12) single negative arg returns abs(-12)'
assert math.lcm(4, 6) == 12, 'lcm(4, 6) two args'
assert math.lcm(4, 6, 10) == 60, 'lcm(4, 6, 10) three args'
assert math.lcm(0, 5) == 0, 'lcm(0, 5) returns 0 if any arg is 0'

# === perm with optional k ===
assert math.perm(5) == 120, 'perm(5) defaults k to n (= 5!)'
assert math.perm(5, 2) == 20, 'perm(5, 2) with explicit k'
assert math.perm(0) == 1, 'perm(0) is 1'

# === isclose with rel_tol/abs_tol kwargs ===
assert math.isclose(1.0, 1.1, rel_tol=0.2) == True, 'isclose with rel_tol=0.2'
assert math.isclose(1.0, 1.1, abs_tol=0.2) == True, 'isclose with abs_tol=0.2'
assert math.isclose(1.0, 1.1) == False, 'isclose with defaults (not close)'
assert math.isclose(1.0, 1.0 + 1e-10) == True, 'isclose with defaults (close)'

# isclose negative tolerance raises ValueError
threw = False
try:
    math.isclose(1.0, 1.0, rel_tol=-0.1)
except ValueError:
    threw = True
assert threw, 'isclose with negative rel_tol raises ValueError'

threw = False
try:
    math.isclose(1.0, 1.0, abs_tol=-0.1)
except ValueError:
    threw = True
assert threw, 'isclose with negative abs_tol raises ValueError'

# isclose unknown kwarg raises TypeError
threw = False
try:
    math.isclose(1.0, 1.0, foo=0.1)
except TypeError:
    threw = True
assert threw, 'isclose with unknown kwarg raises TypeError'

# === ldexp sign preservation ===
assert str(math.ldexp(-0.0, 1000)) == '-0.0', 'ldexp(-0.0, n) preserves sign'
assert math.ldexp(float('-inf'), 1) == float('-inf'), 'ldexp(-inf, 1) returns -inf'

# === frexp(-0.0) sign preservation ===
m, e = math.frexp(-0.0)
assert str(m) == '-0.0', 'frexp(-0.0) mantissa preserves sign'
assert e == 0, 'frexp(-0.0) exponent is 0'

# === comb with GCD reduction (values that would overflow intermediate without it) ===
assert math.comb(62, 31) == 465428353255261088, 'comb(62, 31) with GCD reduction'
assert math.comb(61, 30) == 232714176627630544, 'comb(61, 30) with GCD reduction'

# === isclose arg count errors ===
threw = False
try:
    math.isclose()
except TypeError:
    threw = True
assert threw, 'isclose with 0 args raises TypeError'

threw = False
try:
    math.isclose(1.0)
except TypeError:
    threw = True
assert threw, 'isclose with 1 arg raises TypeError'

threw = False
try:
    math.isclose(1.0, 2.0, 3.0)
except TypeError:
    threw = True
assert threw, 'isclose with 3 positional args raises TypeError'

# === perm(-1) single-arg error message ===
threw = False
try:
    math.perm(-1)
except ValueError:
    threw = True
assert threw, 'perm(-1) single-arg raises ValueError'

# === gcd/lcm with i64::MIN-like values (u64 promotion) ===
# gcd(-9223372036854775808, 0) should return 9223372036854775808 (exceeds i64::MAX)
big_gcd = math.gcd(-9223372036854775808, 0)
assert big_gcd == 9223372036854775808, 'gcd(i64::MIN, 0) promotes to LongInt'

# === isqrt large values (Newton's method refinement) ===
# Values near i64::MAX where f64 sqrt loses precision
assert math.isqrt(9223372036854775807) == 3037000499, 'isqrt(i64::MAX)'
assert math.isqrt(9223372030926249001) == 3037000499, 'isqrt(3037000499^2)'
assert math.isqrt(9223372030926249000) == 3037000498, 'isqrt(3037000499^2 - 1)'

# === erf/erfc range coverage ===
# Small x (|x| < 0.84375): exercises PP/QQ polynomial
assert math.erf(0.1) == 0.1124629160182849, 'erf(0.1) small-x range'
assert math.erf(0.5) == 0.5204998778130465, 'erf(0.5) small-x range'

# Medium x (1.25 ≤ |x| < 28): exercises erfc_inner path
assert math.erf(2.0) == 0.9953222650189527, 'erf(2.0) medium-x range'
assert math.erf(5.0) == 0.9999999999984626, 'erf(5.0) large-x range'

# erfc in range 3 (1.25 ≤ |x| < 2.857): exercises RA/SA coefficients
erfc_2 = math.erfc(2.0)
assert math.isclose(erfc_2, 0.004677734981047266, rel_tol=1e-12), 'erfc(2.0) range 3'
