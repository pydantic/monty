# === abs() ===
# Basic abs operations
assert abs(5) == 5, 'abs of positive int'
assert abs(-5) == 5, 'abs of negative int'
assert abs(0) == 0, 'abs of zero'
assert abs(3.14) == 3.14, 'abs of positive float'
assert abs(-3.14) == 3.14, 'abs of negative float'
assert abs(True) == 1, 'abs of True'
assert abs(False) == 0, 'abs of False'

# === round() ===
# Basic round operations
assert round(2.5) == 2, 'round 2.5 (bankers rounding)'
assert round(3.5) == 4, 'round 3.5 (bankers rounding)'
assert round(0.5) == 0, 'round 0.5 (bankers rounding)'
assert round(-0.5) == 0, 'round -0.5 (bankers rounding)'
assert round(2.4) == 2, 'round 2.4'
assert round(2.6) == 3, 'round 2.6'
assert round(-2.5) == -2, 'round -2.5'
assert round(-1.5) == -2, 'round -1.5 (bankers rounding)'
assert round(5) == 5, 'round integer'

# round with ndigits
assert round(3.14159, 2) == 3.14, 'round to 2 digits'
assert round(3.14159, 0) == 3.0, 'round to 0 digits returns float'
assert repr(round(-0.4, 0)) == '-0.0', 'round(-0.4, 0) preserves negative zero sign'
assert repr(round(-0.5, 0)) == '-0.0', 'round(-0.5, 0) preserves negative zero sign'
assert round(1234, -2) == 1200, 'round int to nearest 100'
assert round(1250, -2) == 1200, 'round 1250 to nearest 100 (bankers)'
assert round(1350, -2) == 1400, 'round 1350 to nearest 100'
assert round(15, -1) == 20, 'round 15 to nearest 10 (bankers)'
assert round(25, -1) == 20, 'round 25 to nearest 10 (bankers)'

# round with None
assert round(2.5, None) == 2, 'round with None ndigits'
assert round(True, -1) == 0, 'round True with negative digits behaves like int'
assert round(True, 2) == 1, 'round True with positive digits returns int'
assert round(False, -3) == 0, 'round False with negative digits stays zero'

# round type errors
threw = False
try:
    round(1.2, 1.5)
except TypeError:
    threw = True
assert threw, 'round with non-int ndigits raises TypeError'

# round edge cases with extreme values
assert isinstance(round(1e15), int), 'round large float returns int'
assert isinstance(round(-1e15), int), 'round large negative float returns int'
assert round(0.0) == 0, 'round(0.0) is zero'
assert round(-0.0) == 0, 'round(-0.0) is zero'

# round special float values (infinity / NaN)
inf = float('inf')
neg_inf = float('-inf')
nan = float('nan')

threw = False
try:
    round(inf)
except OverflowError:
    threw = True
assert threw, 'round(inf) raises OverflowError'

threw = False
try:
    round(neg_inf)
except OverflowError:
    threw = True
assert threw, 'round(-inf) raises OverflowError'

threw = False
try:
    round(nan)
except ValueError:
    threw = True
assert threw, 'round(nan) raises ValueError'

r = round(inf, 0)
assert r == inf, 'round(inf, 0) returns inf'

r = round(neg_inf, 0)
assert r == neg_inf, 'round(-inf, 0) returns -inf'

r = round(nan, 0)
assert r != r, 'round(nan, 0) returns NaN'

# round with extreme ndigits values
assert round(1.23, 10**6) == 1.23, 'round with huge positive ndigits returns original float'
assert round(1.23, -(10**6)) == 0.0, 'round with huge negative ndigits returns zero'
assert repr(round(-1.23, -(10**6))) == '-0.0', 'round with huge negative ndigits preserves signed zero'

# round with float result (ndigits specified)
assert isinstance(round(1.5, 1), float), 'round with ndigits returns float'
assert round(1.25, 1) == 1.2, 'round 1.25 to 1 decimal (bankers rounding)'
assert round(1.35, 1) == 1.4, 'round 1.35 to 1 decimal'

# === divmod() ===
# Basic divmod operations
assert divmod(17, 5) == (3, 2), 'divmod 17, 5'
assert divmod(10, 3) == (3, 1), 'divmod 10, 3'
assert divmod(9, 3) == (3, 0), 'divmod 9, 3'
assert divmod(-10, 3) == (-4, 2), 'divmod -10, 3 (floor division)'
assert divmod(10, -3) == (-4, -2), 'divmod 10, -3'
assert divmod(-10, -3) == (3, -1), 'divmod -10, -3'

# divmod with floats
r = divmod(7.5, 2.5)
assert r[0] == 3.0 and r[1] == 0.0, 'divmod floats'
assert divmod(True, 2) == (0, 1), 'divmod accepts bool numerator'
assert divmod(5, True) == (5, 0), 'divmod accepts bool denominator'

# === pow() ===
# Basic pow operations
assert pow(2, 3) == 8, 'pow 2^3'
assert pow(2, 0) == 1, 'pow x^0'
assert pow(5, 1) == 5, 'pow x^1'
assert pow(2, 10) == 1024, 'pow 2^10'

# pow with negative exponent
assert pow(2, -1) == 0.5, 'pow with negative exp'
assert pow(4, -2) == 0.0625, 'pow 4^-2'

# pow with floats
assert pow(2.0, 3.0) == 8.0, 'pow with floats'
assert pow(4.0, 0.5) == 2.0, 'pow float sqrt'

# Three-argument pow (modular exponentiation)
assert pow(2, 10, 1000) == 24, 'pow modular 2^10 % 1000'
assert pow(3, 4, 5) == 1, 'pow modular 3^4 % 5'
assert pow(7, 256, 13) == 9, 'pow modular large exp'

# Modular exponentiation edge cases
assert pow(2, 0, 5) == 1, 'pow x^0 mod n'
assert pow(0, 5, 3) == 0, 'pow 0^n mod m'
assert pow(True, 2) == 1, 'pow handles bool base'
assert pow(2, True) == 2, 'pow handles bool exponent'
assert pow(True, True) == 1, 'pow handles bool base and exponent'
assert pow(True, -1) == 1.0, 'pow bool negative exponent works like int'

threw = False
try:
    pow(0, -1)
except ZeroDivisionError:
    threw = True
assert threw, 'pow(0, negative) raises ZeroDivisionError'

threw = False
try:
    pow(0.0, -1)
except ZeroDivisionError:
    threw = True
assert threw, 'pow(0.0, negative) raises ZeroDivisionError'
