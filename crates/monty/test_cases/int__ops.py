# === Integer addition ===
assert 1 + 2 == 3
assert 5 + 0 == 5
assert 0 + 5 == 5

# === Integer subtraction ===
assert 5 - 3 == 2
assert 5 - 0 == 5

# === Mixed int/float addition ===
assert 3 + 4.0 == 7.0
assert 4.0 + 3 == 7.0
assert -2 + 3.5 == 1.5
assert 0 + 2.5 == 2.5
assert 2.5 + 0 == 2.5

# === Mixed int/float subtraction ===
assert 5 - 2.5 == 2.5
assert 5.5 - 2 == 3.5
assert -3 - 1.5 == -4.5
assert 1.5 - (-2) == 3.5

# === Float subtraction ===
assert 5.5 - 2.5 == 3.0
assert 0.0 - 1.5 == -1.5

# === Integer modulo ===
assert 10 % 3 == 1
assert 3 % 10 == 3
assert 9 % 3 == 0

# === Augmented assignment (+=) ===
x = 5
x += 3
assert x == 8

# === Integer repr/str ===
assert repr(42) == '42'
assert str(42) == '42'

# === Float repr/str ===
assert repr(2.5) == '2.5'
assert str(2.5) == '2.5'

# === Integer multiplication ===
assert 3 * 4 == 12
assert 5 * 0 == 0
assert 0 * 5 == 0
assert -3 * 4 == -12
assert 3 * -4 == -12
assert -3 * -4 == 12

# === Float multiplication ===
assert 3.0 * 4.0 == 12.0
assert 2.5 * 2.0 == 5.0

# === Mixed int/float multiplication ===
assert 3 * 4.0 == 12.0
assert 4.0 * 3 == 12.0

# === True division (always returns float) ===
assert 6 / 2 == 3.0
assert 7 / 2 == 3.5
assert 1 / 4 == 0.25
assert 6.0 / 2.0 == 3.0
assert 7 / 2.0 == 3.5
assert 7.0 / 2 == 3.5
assert -7 / 2 == -3.5

# === Floor division ===
assert 7 // 2 == 3
assert 6 // 2 == 3
assert -7 // 2 == -4
assert 7 // -2 == -4
assert -7 // -2 == 3
assert 7.0 // 2.0 == 3.0
assert 7 // 2.0 == 3.0
assert 7.0 // 2 == 3.0
assert -7.0 // 2.0 == -4.0

# === Power (exponentiation) ===
assert 2**3 == 8
assert 2**10 == 1024
assert 2**0 == 1
assert (-2) ** 3 == -8
assert (-2) ** 2 == 4
assert 2**-1 == 0.5
assert 2**-2 == 0.25
assert 4.0**2.0 == 16.0
assert 4**0.5 == 2.0
assert 8 ** (1 / 3) == 2.0
assert 2.0**3 == 8.0

# === Augmented assignment operators ===
# *=
x = 5
x *= 3
assert x == 15

# /=
x = 10
x /= 4
assert x == 2.5

# //=
x = 10
x //= 3
assert x == 3

# **=
x = 2
x **= 4
assert x == 16

# -=
x = 10
x -= 3
assert x == 7

# %=
x = 10
x %= 3
assert x == 1

# === Bool arithmetic (True=1, False=0) ===
# Bool multiplication
assert True * 3 == 3
assert False * 5 == 0
assert 3 * True == 3
assert 3 * False == 0
assert True * True == 1
assert True * False == 0
assert True * 2.5 == 2.5
assert 2.5 * True == 2.5

# Bool division
assert True / 2 == 0.5
assert False / 2 == 0.0
assert 4 / True == 4.0
assert True / True == 1.0
assert True / 2.0 == 0.5
assert 4.0 / True == 4.0

# Bool floor division
assert True // 2 == 0
assert False // 2 == 0
assert 5 // True == 5
assert True // True == 1
assert True // 2.0 == 0.0
assert 5.5 // True == 5.0

# Bool power
assert True**3 == 1
assert False**3 == 0
assert 2**True == 2
assert 2**False == 1
assert True**True == 1
assert False**False == 1
assert True**2.0 == 1.0
assert 2.0**True == 2.0
assert 2.0**False == 1.0

# === Unary positive (no-op for numbers, converts bools to int) ===
assert +5 == 5
assert +(-3) == -3
assert +0 == 0
assert +3.14 == 3.14
assert +(-2.5) == -2.5
assert +0.0 == 0.0
assert +True == 1
assert +False == 0
# Verify +bool returns int type, not bool
assert type(+True) == int
assert type(+False) == int

# === Unary negative ===
assert -5 == -5
assert -(-3) == 3
assert -0 == 0
assert -3.14 == -3.14
assert -(-2.5) == 2.5
assert -True == -1
assert repr(-True) == '-1'
assert -False == 0
assert repr(-False) == '0'

# === Unary invert (bitwise NOT) ===
assert ~0 == -1
assert ~1 == -2
assert ~(-1) == 0
assert ~True == -2
assert repr(~True) == '-2'
assert ~False == -1
assert repr(~False) == '-1'

assert int('123') == 123
assert int('  123  ') == 123
assert int('1_234 ') == 1234

try:
    int('abc')
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 10: 'abc'", f'got err: {e}'
else:
    raise AssertionError('int conversion from string should fail')

try:
    int(' ')
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 10: ' '", f'got err: {e}'
else:
    raise AssertionError('int conversion from string should fail')

try:
    int('a\tbc')
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 10: 'a\\tbc'", f'got err: {e}'
else:
    raise AssertionError('int conversion from string should fail')
