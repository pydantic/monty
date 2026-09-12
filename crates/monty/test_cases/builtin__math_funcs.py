import sys

is_monty = sys.platform == 'monty'

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
# both round() arguments are keyword-capable in CPython
assert round(3.14159, ndigits=2) == 3.14
assert round(number=3.14159, ndigits=2) == 3.14
assert round(ndigits=2, number=3.14159) == 3.14
assert repr(round(-0.4, 0)) == '-0.0'
assert repr(round(-0.5, 0)) == '-0.0'

# round() on a long int rounds exactly, half to even, like `int.__round__`
big = 2**70
assert round(big) == big
assert round(big, 2) == big
assert round(big, None) == big
assert round(big, big) == big
assert round(big, -2) == 1180591620717411303400
assert round(big, -5) == 1180591620717411300000
assert round(-big, -5) == -1180591620717411300000
assert round(big, -21) == 10**21
assert round(big, -22) == 0
assert round(10**30 + 5 * 10**10, -11) == 10**30
assert round(10**30 + 15 * 10**10, -11) == 10**30 + 2 * 10**11
assert round(-(10**30) - 5 * 10**10, -11) == -(10**30)
assert round(10**400, -399) == 10**400
assert round(10**400, -400) == 10**400
assert round(10**400, -401) == 0
assert round(5 * 10**399, -400) == 0
assert round(15 * 10**399, -400) == 2 * 10**400
assert round(-big) == -big
assert round(big, True) == big
assert round(big, -1) == 1180591620717411303420
assert round(big + 5, -1) == 1180591620717411303430
assert round(big + 15, -1) == 1180591620717411303440
assert round(-big - 5, -1) == -1180591620717411303430
assert round(-(10**30) - 15 * 10**10, -11) == -(10**30) - 2 * 10**11
assert round(10**30, -30) == 10**30
assert round(5 * 10**29, -30) == 0
assert round(-5 * 10**29, -30) == 0
assert round(15 * 10**29, -30) == 2 * 10**30
try:
    round(big, 2.0)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'float' object cannot be interpreted as an integer"
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

# ndigits wider than i64 is clamped by sign: huge positive returns the number
# unchanged; huge negative rounds to 0 / ±0.0
assert round(5, 10**30) == 5
assert round(-5, 10**30) == -5
assert round(2.675, 10**30) == 2.675
assert round(5, 2**63 - 1) == 5
assert round(1.5, -(10**30)) == 0.0
assert repr(round(-1.5, -(10**30))) == '-0.0'
assert round(1.5, -(2**63)) == 0.0
assert round(12345, -(10**5)) == 0

# negative ndigits round exactly in integers (no float corruption), promoting
# past i64 when rounding up crosses it
assert round(2**63 - 1, -1) == 9223372036854775810
assert round(2**63 - 1, -19) == 10**19
assert round(-(2**63 - 1), -19) == -(10**19)
assert round(5 * 10**18, -19) == 0
assert round(2**63 - 1, -25) == 0
assert round(1234567890123456789, -5) == 1234567890123500000
assert round(-1250, -2) == -1200
if is_monty:
    # CPython tries to materialise 10**(10**30) here and dies with
    # MemoryError; Monty's clamp returns 0 immediately (limitations/builtins.md)
    assert round(5, -(10**30)) == 0
    assert round(5, -(2**63)) == 0

# round edge cases with extreme values
assert isinstance(round(1e15), int)
assert isinstance(round(-1e15), int)
assert round(9.223372036854776e18) == 9223372036854775808
assert round(1e20) == 100000000000000000000
assert round(-1e20) == -100000000000000000000
assert round(1.7976931348623157e308) == (2**53 - 1) * 2**971
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

# Modular exponentiation with heap-backed integers
big = 2**63
assert pow(big, 3, 7) == 1
assert pow(2, big, 7) == 4
assert pow(2, 3, big + 1) == 8
assert pow(big, 3, -(big + 1)) == -1

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

# pow() is the ** operator: every numeric pairing, including long ints and bools
big = 2**70
assert pow(2, 100) == 1267650600228229401496703205376
assert pow(2, 63) == 9223372036854775808
assert pow(-2, 63) == -9223372036854775808
assert pow(big, 2) == 1393796574908163946345982392040522594123776
assert pow(1, big) == 1
assert pow(-1, big) == 1
assert pow(-1, big + 1) == -1
assert pow(0, big) == 0
assert pow(big, 0) == 1
assert pow(big, -2) == 7.174648137343064e-43
assert pow(False, 0) == 1
assert pow(True, 2.5) == 1.0
assert pow(2.5, True) == 2.5
assert pow(2, 3, None) == 8

# modular pow with every integer representation
assert pow(2, 3, 5) == 3
assert pow(2, 3, -5) == -2
assert pow(-2, 3, 5) == 2
assert pow(big, 2, 7) == 4
assert pow(2, big, 7) == 2
assert pow(2, 3, big) == 8
assert pow(big, big, big - 1) == 1
assert pow(True, 3, 2) == 1
try:
    pow(2, 3, 0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'pow() 3rd argument cannot be 0'

# a float anywhere rejects the third argument; other types are an unsupported operand
for compute in [lambda: pow(2.0, 3, 5), lambda: pow(2, 3.0, 5), lambda: pow(2, 3, 5.0), lambda: pow(2.0, 'a', 3)]:
    try:
        compute()
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == 'pow() 3rd argument not allowed unless all arguments are integers'
for compute, message in [
    (lambda: pow('a', 2), "unsupported operand type(s) for ** or pow(): 'str' and 'int'"),
    (lambda: pow(2, 'a'), "unsupported operand type(s) for ** or pow(): 'int' and 'str'"),
    (lambda: pow('a', 2, 3), "unsupported operand type(s) for ** or pow(): 'str', 'int', 'int'"),
    (lambda: pow(2, 3, 'a'), "unsupported operand type(s) for ** or pow(): 'int', 'int', 'str'"),
    (lambda: pow(True, 2, 'a'), "unsupported operand type(s) for ** or pow(): 'bool', 'int', 'str'"),
    (lambda: pow(2), "pow() missing required argument 'exp' (pos 2)"),
    (lambda: pow(), "pow() missing required argument 'base' (pos 1)"),
    (lambda: pow(2, 3, 4, 5), 'pow() takes at most 3 arguments (4 given)'),
]:
    try:
        compute()
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == message
