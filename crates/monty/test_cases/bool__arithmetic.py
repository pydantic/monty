from math import copysign, isnan

# Bool operands participate in numeric arithmetic as integers 0 and 1.
assert False + False == 0
assert type(False + False) is int
assert False + False == 0
assert type(False + False) is int
assert False - False == 0
assert type(False - False) is int
assert False - False == 0
assert type(False - False) is int

assert False + True == 1
assert type(False + True) is int
assert True + False == 1
assert type(True + False) is int
assert False - True == -1
assert type(False - True) is int
assert True - False == 1
assert type(True - False) is int
assert False % True == 0
assert type(False % True) is int

assert False + (-2) == -2
assert type(False + (-2)) is int
assert (-2) + False == -2
assert type((-2) + False) is int
assert False - (-2) == 2
assert type(False - (-2)) is int
assert (-2) - False == -2
assert type((-2) - False) is int
assert False % (-2) == 0
assert type(False % (-2)) is int

assert False + 0 == 0
assert type(False + 0) is int
assert 0 + False == 0
assert type(0 + False) is int
assert False - 0 == 0
assert type(False - 0) is int
assert 0 - False == 0
assert type(0 - False) is int

assert False + 3 == 3
assert type(False + 3) is int
assert 3 + False == 3
assert type(3 + False) is int
assert False - 3 == -3
assert type(False - 3) is int
assert 3 - False == 3
assert type(3 - False) is int
assert False % 3 == 0
assert type(False % 3) is int

assert False + (-(2**63)) == -9223372036854775808
assert type(False + (-(2**63))) is int
assert (-(2**63)) + False == -9223372036854775808
assert type((-(2**63)) + False) is int
assert False - (-(2**63)) == 9223372036854775808
assert type(False - (-(2**63))) is int
assert (-(2**63)) - False == -9223372036854775808
assert type((-(2**63)) - False) is int
assert False % (-(2**63)) == 0
assert type(False % (-(2**63))) is int

assert False + (2**63 - 1) == 9223372036854775807
assert type(False + (2**63 - 1)) is int
assert (2**63 - 1) + False == 9223372036854775807
assert type((2**63 - 1) + False) is int
assert False - (2**63 - 1) == -9223372036854775807
assert type(False - (2**63 - 1)) is int
assert (2**63 - 1) - False == 9223372036854775807
assert type((2**63 - 1) - False) is int
assert False % (2**63 - 1) == 0
assert type(False % (2**63 - 1)) is int

assert False + 2**80 == 1208925819614629174706176
assert type(False + 2**80) is int
assert 2**80 + False == 1208925819614629174706176
assert type(2**80 + False) is int
assert False - 2**80 == -1208925819614629174706176
assert type(False - 2**80) is int
assert 2**80 - False == 1208925819614629174706176
assert type(2**80 - False) is int
assert False % 2**80 == 0
assert type(False % 2**80) is int

assert False + (-(2**80)) == -1208925819614629174706176
assert type(False + (-(2**80))) is int
assert (-(2**80)) + False == -1208925819614629174706176
assert type((-(2**80)) + False) is int
assert False - (-(2**80)) == 1208925819614629174706176
assert type(False - (-(2**80))) is int
assert (-(2**80)) - False == -1208925819614629174706176
assert type((-(2**80)) - False) is int
assert False % (-(2**80)) == 0
assert type(False % (-(2**80))) is int

assert False + (-2.5) == -2.5
assert type(False + (-2.5)) is float
assert (-2.5) + False == -2.5
assert type((-2.5) + False) is float
assert False - (-2.5) == 2.5
assert type(False - (-2.5)) is float
assert (-2.5) - False == -2.5
assert type((-2.5) - False) is float
assert False % (-2.5) == -0.0
assert type(False % (-2.5)) is float

assert False + 0.0 == 0.0
assert type(False + 0.0) is float
assert 0.0 + False == 0.0
assert type(0.0 + False) is float
assert False - 0.0 == 0.0
assert type(False - 0.0) is float
assert 0.0 - False == 0.0
assert type(0.0 - False) is float

assert False + 3.5 == 3.5
assert type(False + 3.5) is float
assert 3.5 + False == 3.5
assert type(3.5 + False) is float
assert False - 3.5 == -3.5
assert type(False - 3.5) is float
assert 3.5 - False == 3.5
assert type(3.5 - False) is float
assert False % 3.5 == 0.0
assert type(False % 3.5) is float

assert True + False == 1
assert type(True + False) is int
assert False + True == 1
assert type(False + True) is int
assert True - False == 1
assert type(True - False) is int
assert False - True == -1
assert type(False - True) is int
assert False % True == 0
assert type(False % True) is int

assert True + True == 2
assert type(True + True) is int
assert True + True == 2
assert type(True + True) is int
assert True - True == 0
assert type(True - True) is int
assert True - True == 0
assert type(True - True) is int
assert True % True == 0
assert type(True % True) is int
assert True % True == 0
assert type(True % True) is int

assert True + (-2) == -1
assert type(True + (-2)) is int
assert (-2) + True == -1
assert type((-2) + True) is int
assert True - (-2) == 3
assert type(True - (-2)) is int
assert (-2) - True == -3
assert type((-2) - True) is int
assert True % (-2) == -1
assert type(True % (-2)) is int
assert (-2) % True == 0
assert type((-2) % True) is int

assert True + 0 == 1
assert type(True + 0) is int
assert 0 + True == 1
assert type(0 + True) is int
assert True - 0 == 1
assert type(True - 0) is int
assert 0 - True == -1
assert type(0 - True) is int
assert 0 % True == 0
assert type(0 % True) is int

assert True + 3 == 4
assert type(True + 3) is int
assert 3 + True == 4
assert type(3 + True) is int
assert True - 3 == -2
assert type(True - 3) is int
assert 3 - True == 2
assert type(3 - True) is int
assert True % 3 == 1
assert type(True % 3) is int
assert 3 % True == 0
assert type(3 % True) is int

assert True + (-(2**63)) == -9223372036854775807
assert type(True + (-(2**63))) is int
assert (-(2**63)) + True == -9223372036854775807
assert type((-(2**63)) + True) is int
assert True - (-(2**63)) == 9223372036854775809
assert type(True - (-(2**63))) is int
assert (-(2**63)) - True == -9223372036854775809
assert type((-(2**63)) - True) is int
assert True % (-(2**63)) == -9223372036854775807
assert type(True % (-(2**63))) is int
assert (-(2**63)) % True == 0
assert type((-(2**63)) % True) is int

assert True + (2**63 - 1) == 9223372036854775808
assert type(True + (2**63 - 1)) is int
assert (2**63 - 1) + True == 9223372036854775808
assert type((2**63 - 1) + True) is int
assert True - (2**63 - 1) == -9223372036854775806
assert type(True - (2**63 - 1)) is int
assert (2**63 - 1) - True == 9223372036854775806
assert type((2**63 - 1) - True) is int
assert True % (2**63 - 1) == 1
assert type(True % (2**63 - 1)) is int
assert (2**63 - 1) % True == 0
assert type((2**63 - 1) % True) is int

assert True + 2**80 == 1208925819614629174706177
assert type(True + 2**80) is int
assert 2**80 + True == 1208925819614629174706177
assert type(2**80 + True) is int
assert True - 2**80 == -1208925819614629174706175
assert type(True - 2**80) is int
assert 2**80 - True == 1208925819614629174706175
assert type(2**80 - True) is int
assert True % 2**80 == 1
assert type(True % 2**80) is int
assert 2**80 % True == 0
assert type(2**80 % True) is int

assert True + (-(2**80)) == -1208925819614629174706175
assert type(True + (-(2**80))) is int
assert (-(2**80)) + True == -1208925819614629174706175
assert type((-(2**80)) + True) is int
assert True - (-(2**80)) == 1208925819614629174706177
assert type(True - (-(2**80))) is int
assert (-(2**80)) - True == -1208925819614629174706177
assert type((-(2**80)) - True) is int
assert True % (-(2**80)) == -1208925819614629174706175
assert type(True % (-(2**80))) is int
assert (-(2**80)) % True == 0
assert type((-(2**80)) % True) is int

assert True + (-2.5) == -1.5
assert type(True + (-2.5)) is float
assert (-2.5) + True == -1.5
assert type((-2.5) + True) is float
assert True - (-2.5) == 3.5
assert type(True - (-2.5)) is float
assert (-2.5) - True == -3.5
assert type((-2.5) - True) is float
assert True % (-2.5) == -1.5
assert type(True % (-2.5)) is float
assert (-2.5) % True == 0.5
assert type((-2.5) % True) is float

assert True + 0.0 == 1.0
assert type(True + 0.0) is float
assert 0.0 + True == 1.0
assert type(0.0 + True) is float
assert True - 0.0 == 1.0
assert type(True - 0.0) is float
assert 0.0 - True == -1.0
assert type(0.0 - True) is float
assert 0.0 % True == 0.0
assert type(0.0 % True) is float

assert True + 3.5 == 4.5
assert type(True + 3.5) is float
assert 3.5 + True == 4.5
assert type(3.5 + True) is float
assert True - 3.5 == -2.5
assert type(True - 3.5) is float
assert 3.5 - True == 2.5
assert type(3.5 - True) is float
assert True % 3.5 == 1.0
assert type(True % 3.5) is float
assert 3.5 % True == 0.5
assert type(3.5 % True) is float

try:
    False % False
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass

try:
    True % False
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass

try:
    -2 % False
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass

try:
    2**80 % False
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass

try:
    -2.5 % False
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass

# Examples reported as failing in https://github.com/pydantic/monty/issues/963.
assert 1 + True == 2
assert True + 1 == 2
assert True + True == 2
assert 1.5 + True == 2.5
assert sum([True, False, True]) == 2
assert sum(x == 0 for x in [0, 1, 0]) == 2
assert sum([1, 2], True) == 4
assert len([1]) + (3 > 2) == 2
x = 0
x += True
assert x == 1
assert type(x) is int
assert 1 - False == 1
assert True - True == 0
assert 1.0 - True == 0.0
x = 5
x -= True
assert x == 4
assert type(x) is int
assert 7 % True == 0

# Examples reported as working in the same issue.
assert isinstance(True, int) is True
assert True == 1
assert hash(True) == hash(1)
assert 2 * True == 2
assert True * 3 == 3
assert 2.0 * True == 2.0
x = 2
x *= True
assert x == 2
assert 5 / True == 5.0
assert 7 // True == 7
assert 2**True == 2
assert True**2 == 1
assert -True == -1
assert ~True == -2
assert abs(True) == 1
assert round(True) == 1
assert divmod(True, 1) == (1, 0)
assert True & 1 == 1
assert True | 2 == 3
assert (True ^ True) is False
assert True < 2
assert max(True, 2) == 2
assert int(True) + 1 == 2
assert [10, 20][True] == 20
assert {1: 'a'}[True] == 'a'
assert sum(1 for x in [0, 1, 0] if x == 0) == 2

rows = [{'metrics': {'ctr': '0'}}, {'metrics': {'ctr': '0.5'}}, {'metrics': {'ctr': '0'}}]
assert sum(float(x['metrics']['ctr']) == 0 for x in rows) == 2

value = False
value += True
assert value == 1
assert type(value) is int
value -= True
assert value == 0
assert type(value) is int
value = 7
value %= True
assert value == 0
assert type(value) is int

# Formatting must retain the bool operand, including its spelling for %s.
assert '%s' % True == 'True'
assert b'%d' % True == b'1'

# Float floor division uses the same quotient as divmod, including special values.
assert True // 0.1 == 9.0
assert type(True // 0.1) is float
assert True // 0.1 == divmod(True, 0.1)[0]
assert True // -0.1 == -10.0
assert True // float('-inf') == -1.0
assert isnan(float('inf') // True) is True
assert isnan(float('-inf') // True) is True
assert isnan(True // float('nan')) is True
assert isnan(float('nan') // True) is True
assert copysign(1.0, False // -0.1) == -1.0
assert copysign(1.0, -0.0 // True) == -1.0
x = True
x //= 0.1
assert x == 9.0
assert type(x) is float

try:
    True // 0.0
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass
try:
    0.1 // False
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass

# Nonnegative integer exponents keep bool powers as ints across storage boundaries.
assert True ** (2**32 - 1) == 1
assert type(True ** (2**32 - 1)) is int
assert True ** (2**32) == 1
assert type(True ** (2**32)) is int
assert False ** (2**32) == 0
assert type(False ** (2**32)) is int
assert True ** (2**63 - 1) == 1
assert type(True ** (2**63 - 1)) is int
assert False ** (2**63 - 1) == 0
assert type(False ** (2**63 - 1)) is int
assert True ** (2**63) == 1
assert type(True ** (2**63)) is int
assert False ** (2**63) == 0
assert type(False ** (2**63)) is int
assert False**0 == 1
assert type(False**0) is int
assert True**0 == 1
assert type(True**0) is int
assert True**-1 == 1.0
assert type(True**-1) is float
assert True**2.0 == 1.0
assert type(True**2.0) is float
assert pow(True, 2**32) == 1
assert type(pow(True, 2**32)) is int
x = True
x **= 2**32
assert x == 1
assert type(x) is int
x = False
x **= 2**32
assert x == 0
assert type(x) is int

try:
    False**-1
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass
