# === float modulo: result takes the divisor's sign ===
assert 7.5 % 2 == 1.5
assert -7.0 % 3.0 == 2.0
assert 7.0 % -3.0 == -2.0
assert -7.0 % -3.0 == -1.0
assert -7 % 3.0 == 2.0
assert -7.0 % 3 == 2.0
assert str(-6.0 % 3.0) == '0.0'
assert str(6.0 % -3.0) == '-0.0'

# === infinite divisor ===
assert 5.0 % float('inf') == 5.0
assert -5.0 % float('inf') == float('inf')
assert 5.0 % -float('inf') == -float('inf')

# === `%` agrees with divmod's remainder ===
assert divmod(-7.0, 3.0)[1] == -7.0 % 3.0
assert divmod(7.0, -3.0)[1] == 7.0 % -3.0

# === `%` inside a comparison ===
assert (-7.0 % 3.0 == 2) is True
assert (7.0 % -3.0 == -2) is True
assert (5.0 % 3.0 == 2) is True

# === unsupported operands ===
try:
    [1] % 2
    assert False, 'expected TypeError from %'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for %: 'list' and 'int'"

# === zero divisor raises for every numeric combination ===
for a, b in [(5.0, 0.0), (5.0, 0), (5, 0.0), (5, 0)]:
    try:
        a % b
        assert False, 'expected ZeroDivisionError from %'
    except ZeroDivisionError as e:
        assert str(e) == 'division by zero'
