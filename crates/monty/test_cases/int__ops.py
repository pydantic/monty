# === Integer addition ===
assert 1 + 2 == 3, 'basic add'
assert 5 + 0 == 5, 'add zero'
assert 0 + 5 == 5, 'zero add'

# === Integer subtraction ===
assert 5 - 3 == 2, 'basic sub'
assert 5 - 0 == 5, 'sub zero'

# === Integer modulo ===
assert 10 % 3 == 1, 'basic mod'
assert 3 % 10 == 3, 'mod larger divisor'
assert 9 % 3 == 0, 'mod zero result'

# === Augmented assignment (+=) ===
x = 5
x += 3
assert x == 8, 'basic iadd'

# === Integer repr/str ===
assert repr(42) == '42', 'int repr'
assert str(42) == '42', 'int str'

# === Float repr/str ===
assert repr(2.5) == '2.5', 'float repr'
assert str(2.5) == '2.5', 'float str'

# === Integer multiplication ===
assert 3 * 4 == 12, 'basic int mult'
assert 5 * 0 == 0, 'mult by zero'
assert 0 * 5 == 0, 'zero mult'
assert -3 * 4 == -12, 'negative mult'
assert 3 * -4 == -12, 'mult negative'
assert -3 * -4 == 12, 'neg mult neg'

# === Float multiplication ===
assert 3.0 * 4.0 == 12.0, 'float mult'
assert 2.5 * 2.0 == 5.0, 'float mult 2'

# === Mixed int/float multiplication ===
assert 3 * 4.0 == 12.0, 'int mult float'
assert 4.0 * 3 == 12.0, 'float mult int'

# === True division (always returns float) ===
assert 6 / 2 == 3.0, 'int div exact'
assert 7 / 2 == 3.5, 'int div remainder'
assert 1 / 4 == 0.25, 'int div fraction'
assert 6.0 / 2.0 == 3.0, 'float div'
assert 7 / 2.0 == 3.5, 'int div float'
assert 7.0 / 2 == 3.5, 'float div int'
assert -7 / 2 == -3.5, 'neg div'

# === Floor division ===
assert 7 // 2 == 3, 'int floor div'
assert 6 // 2 == 3, 'int floor div exact'
assert -7 // 2 == -4, 'neg floor div rounds down'
assert 7 // -2 == -4, 'floor div neg rounds down'
assert -7 // -2 == 3, 'neg floor div neg'
assert 7.0 // 2.0 == 3.0, 'float floor div'
assert 7 // 2.0 == 3.0, 'int floor div float'
assert 7.0 // 2 == 3.0, 'float floor div int'
assert -7.0 // 2.0 == -4.0, 'neg float floor div'

# === Power (exponentiation) ===
assert 2**3 == 8, 'int pow'
assert 2**10 == 1024, 'int pow large'
assert 2**0 == 1, 'pow zero'
assert (-2) ** 3 == -8, 'neg base pow'
assert (-2) ** 2 == 4, 'neg base even pow'
assert 2**-1 == 0.5, 'pow neg returns float'
assert 2**-2 == 0.25, 'pow neg 2'
assert 4.0**2.0 == 16.0, 'float pow'
assert 4**0.5 == 2.0, 'sqrt via pow'
assert 8 ** (1 / 3) == 2.0, 'cube root via pow'
assert 2.0**3 == 8.0, 'float pow int'

# === Augmented assignment operators ===
# *=
x = 5
x *= 3
assert x == 15, 'imult'

# /=
x = 10
x /= 4
assert x == 2.5, 'idiv'

# //=
x = 10
x //= 3
assert x == 3, 'ifloordiv'

# **=
x = 2
x **= 4
assert x == 16, 'ipow'

# -=
x = 10
x -= 3
assert x == 7, 'isub'

# %=
x = 10
x %= 3
assert x == 1, 'imod'

# === Bool arithmetic (True=1, False=0) ===
# Bool multiplication
assert True * 3 == 3, 'bool mult int'
assert False * 5 == 0, 'false mult int'
assert 3 * True == 3, 'int mult bool'
assert 3 * False == 0, 'int mult false'
assert True * True == 1, 'bool mult bool'
assert True * False == 0, 'bool mult false'
assert True * 2.5 == 2.5, 'bool mult float'
assert 2.5 * True == 2.5, 'float mult bool'

# Bool division
assert True / 2 == 0.5, 'bool div int'
assert False / 2 == 0.0, 'false div int'
assert 4 / True == 4.0, 'int div bool'
assert True / True == 1.0, 'bool div bool'
assert True / 2.0 == 0.5, 'bool div float'
assert 4.0 / True == 4.0, 'float div bool'

# Bool floor division
assert True // 2 == 0, 'bool floordiv int'
assert False // 2 == 0, 'false floordiv int'
assert 5 // True == 5, 'int floordiv bool'
assert True // True == 1, 'bool floordiv bool'
assert True // 2.0 == 0.0, 'bool floordiv float'
assert 5.5 // True == 5.0, 'float floordiv bool'

# Bool power
assert True**3 == 1, 'bool pow int'
assert False**3 == 0, 'false pow int'
assert 2**True == 2, 'int pow bool true'
assert 2**False == 1, 'int pow bool false'
assert True**True == 1, 'bool pow bool'
assert False**False == 1, 'false pow false'
assert True**2.0 == 1.0, 'bool pow float'
assert 2.0**True == 2.0, 'float pow bool true'
assert 2.0**False == 1.0, 'float pow bool false'

# === Unary positive (no-op for numbers, converts bools to int) ===
assert +5 == 5, 'unary pos int'
assert +(-3) == -3, 'unary pos neg int'
assert +0 == 0, 'unary pos zero'
assert +3.14 == 3.14, 'unary pos float'
assert +(-2.5) == -2.5, 'unary pos neg float'
assert +0.0 == 0.0, 'unary pos zero float'
assert +True == 1, 'unary pos true'
assert +False == 0, 'unary pos false'
# Verify +bool returns int type, not bool
assert type(+True) == int, 'unary pos true returns int type'
assert type(+False) == int, 'unary pos false returns int type'

# === Unary negative ===
assert -5 == -5, 'unary neg int'
assert -(-3) == 3, 'unary neg neg int'
assert -0 == 0, 'unary neg zero'
assert -3.14 == -3.14, 'unary neg float'
assert -(-2.5) == 2.5, 'unary neg neg float'
assert -True == -1, 'unary neg true'
assert repr(-True) == '-1', 'unary neg true repr'
assert -False == 0, 'unary neg false'
assert repr(-False) == '0', 'unary neg false repr'

# === Unary invert (bitwise NOT) ===
assert ~0 == -1, 'unary invert zero'
assert ~1 == -2, 'unary invert one'
assert ~(-1) == 0, 'unary invert neg one'
assert ~True == -2, 'unary invert true'
assert repr(~True) == '-2', 'unary invert true repr'
assert ~False == -1, 'unary invert false'
assert repr(~False) == '-1', 'unary invert false repr'
