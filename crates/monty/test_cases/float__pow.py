# === Overflow raises like CPython's float_pow ===
# CPython's message is strerror(ERANGE), which differs by libc; Monty always uses glibc's.
ERANGE_MESSAGES = {"(34, 'Numerical result out of range')", "(34, 'Result too large')"}
for compute in [
    lambda: 1.5**10000,
    lambda: 10.0**400,
    lambda: 10**400.0,
    lambda: (-10.0) ** 401,
    lambda: (-1.5) ** 10000,
    lambda: (2**70) ** 15.0,
    lambda: 1.5 ** (2**70),
    lambda: 2.0**1024,
    lambda: pow(1.5, 10000),
    lambda: pow(10, 400.0),
    lambda: pow(2**70, 15.0),
    lambda: pow(1.5, 2**70),
]:
    try:
        compute()
        assert False, 'expected OverflowError'
    except OverflowError as e:
        assert str(e) in ERANGE_MESSAGES

x = 10.0
try:
    x **= 400
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) in ERANGE_MESSAGES

# === Infinite and NaN operands never raise ===
inf = float('inf')
nan = float('nan')
assert inf**2 == inf
assert 2.0**inf == inf
assert 0.5**inf == 0.0
assert (-1.5) ** inf == inf
assert (-1.0) ** inf == 1.0
assert 2.0**-inf == 0.0
assert inf**-1 == 0.0
assert (-inf) ** 3 == -inf
assert (-inf) ** 2 == inf
assert 1.0**nan == 1.0
assert nan**0 == 1.0
assert str(nan**2) == 'nan'
assert str(2.0**nan) == 'nan'
assert pow(inf, 2) == inf
assert pow(2**70, inf) == inf
# A zero base with an infinite exponent is not "zero to a negative power".
assert 0.0**-inf == inf
assert (-0.0) ** -inf == inf
assert 0**-inf == inf
assert False**-inf == inf
assert pow(0.0, -inf) == inf
assert 0.0**inf == 0.0
assert str(0.0**nan) == 'nan'

# === Underflow is silent ===
assert 2.0**-10000 == 0.0
assert 0.5**10000 == 0.0
assert 10**-400.0 == 0.0
assert pow(1.5, -10000) == 0.0
assert 2 ** -(2**70) == 0.0

# === Zero to a negative power ===
for compute in [
    lambda: 0.0**-1,
    lambda: 0**-1.0,
    lambda: 0.0**-1.5,
    lambda: pow(0.0, -2),
    lambda: False**-1.0,
    lambda: 0.0 ** -(2**70),
]:
    try:
        compute()
        assert False, 'expected ZeroDivisionError'
    except ZeroDivisionError as e:
        assert str(e) == 'zero to a negative power'

# === Ordinary results ===
assert 2.0**10 == 1024.0
assert 2**0.5 == 1.4142135623730951
assert 1.1**10 == 2.5937424601000023
assert (-2.0) ** 3 == -8.0
assert 9.0**0.5 == 3.0
assert pow(2.0, -2) == 0.25

# === int ** negative int goes through float pow ===
# Every spelling calls the same C `pow`, so they agree to the last bit.
assert 5**-23 == pow(5, -23) == 5.0**-23 == 8.388608e-17
assert 7**-13 == pow(7, -13) == 7.0**-13
assert 3**-33 == 1.79886509245143e-16
assert True**-3 == 1.0
assert (-3) ** -3 == -0.037037037037037035
assert (-10) ** -401 == 0.0
assert str((-10) ** -401) == '-0.0'
assert (-(2**63)) ** -1 == -1.0842021724855044e-19
