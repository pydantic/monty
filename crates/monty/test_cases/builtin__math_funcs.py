# === abs() ===
# Basic abs operations
assert abs(5) == 5
assert abs(-5) == 5
assert abs(0) == 0
assert abs(3.14) == 3.14
assert abs(-3.14) == 3.14
assert abs(True) == 1
assert abs(False) == 0

# === round() ===
# Basic round operations
assert round(2.5) == 2
assert round(3.5) == 4
assert round(0.5) == 0
assert round(-0.5) == 0
assert round(2.4) == 2
assert round(2.6) == 3
assert round(-2.5) == -2
assert round(-1.5) == -2
assert round(5) == 5

# round with ndigits
assert round(3.14159, 2) == 3.14
assert round(3.14159, 0) == 3.0
assert repr(round(-0.4, 0)) == '-0.0'
assert repr(round(-0.5, 0)) == '-0.0'
assert round(1234, -2) == 1200
assert round(1250, -2) == 1200
assert round(1350, -2) == 1400
assert round(15, -1) == 20
assert round(25, -1) == 20

# round with None
assert round(2.5, None) == 2
assert round(True, -1) == 0
assert round(True, 2) == 1
assert round(False, -3) == 0

# round type errors
threw = False
try:
    round(1.2, 1.5)
except TypeError:
    threw = True
assert threw

# round edge cases with extreme values
assert isinstance(round(1e15), int)
assert isinstance(round(-1e15), int)
assert round(0.0) == 0
assert round(-0.0) == 0

# round special float values (infinity / NaN)
inf = float('inf')
neg_inf = float('-inf')
nan = float('nan')

threw = False
try:
    round(inf)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    round(neg_inf)
except OverflowError:
    threw = True
assert threw

threw = False
try:
    round(nan)
except ValueError:
    threw = True
assert threw

r = round(inf, 0)
assert r == inf

r = round(neg_inf, 0)
assert r == neg_inf

r = round(nan, 0)
assert r != r

# round with extreme ndigits values
assert round(1.23, 10**6) == 1.23
assert round(1.23, -(10**6)) == 0.0
assert repr(round(-1.23, -(10**6))) == '-0.0'

# round with float result (ndigits specified)
assert isinstance(round(1.5, 1), float)
assert round(1.25, 1) == 1.2
assert round(1.35, 1) == 1.4

# === divmod() ===
# Basic divmod operations
assert divmod(17, 5) == (3, 2)
assert divmod(10, 3) == (3, 1)
assert divmod(9, 3) == (3, 0)
assert divmod(-10, 3) == (-4, 2)
assert divmod(10, -3) == (-4, -2)
assert divmod(-10, -3) == (3, -1)

# divmod with floats
r = divmod(7.5, 2.5)
assert r[0] == 3.0 and r[1] == 0.0, 'divmod floats'
assert divmod(True, 2) == (0, 1)
assert divmod(5, True) == (5, 0)

# === pow() ===
# Basic pow operations
assert pow(2, 3) == 8
assert pow(2, 0) == 1
assert pow(5, 1) == 5
assert pow(2, 10) == 1024

# pow with negative exponent
assert pow(2, -1) == 0.5
assert pow(4, -2) == 0.0625

# pow with floats
assert pow(2.0, 3.0) == 8.0
assert pow(4.0, 0.5) == 2.0

# Three-argument pow (modular exponentiation)
assert pow(2, 10, 1000) == 24
assert pow(3, 4, 5) == 1
assert pow(7, 256, 13) == 9

# Modular exponentiation edge cases
assert pow(2, 0, 5) == 1
assert pow(0, 5, 3) == 0

# |modulo| == 1 always returns 0, including the exp == 0 corner case
assert pow(5, 3, 1) == 0
assert pow(5, 3, -1) == 0
assert pow(5, 0, 1) == 0
assert pow(5, 0, -1) == 0

# i64::MIN base with modulo == -1 used to panic via rem_euclid overflow
assert pow(-9223372036854775808, 1, -1) == 0
assert pow(-9223372036854775808, 7, -1) == 0
assert pow(True, 2) == 1
assert pow(2, True) == 2
assert pow(True, True) == 1
assert pow(True, -1) == 1.0

threw = False
try:
    pow(0, -1)
except ZeroDivisionError:
    threw = True
assert threw

threw = False
try:
    pow(0.0, -1)
except ZeroDivisionError:
    threw = True
assert threw
