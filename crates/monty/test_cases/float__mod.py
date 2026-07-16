# === float modulo: result takes the divisor's sign ===
assert 7.5 % 2 == 1.5, 'positive float % positive int'
assert -7.0 % 3.0 == 2.0, 'negative dividend, positive divisor'
assert 7.0 % -3.0 == -2.0, 'positive dividend, negative divisor'
assert -7.0 % -3.0 == -1.0, 'negative dividend, negative divisor'
assert -7 % 3.0 == 2.0, 'int % float sign'
assert -7.0 % 3 == 2.0, 'float % int sign'
assert str(-6.0 % 3.0) == '0.0', 'zero result takes positive divisor sign'
assert str(6.0 % -3.0) == '-0.0', 'zero result takes negative divisor sign'

# === infinite divisor ===
assert 5.0 % float('inf') == 5.0, 'positive % inf is identity'
assert -5.0 % float('inf') == float('inf'), 'negative % inf is inf'
assert 5.0 % -float('inf') == -float('inf'), 'positive % -inf is -inf'

# === `%` agrees with divmod's remainder ===
assert divmod(-7.0, 3.0)[1] == -7.0 % 3.0, 'divmod remainder matches %'
assert divmod(7.0, -3.0)[1] == 7.0 % -3.0, 'divmod remainder matches % (negative divisor)'

# === `%` inside a comparison ===
assert (-7.0 % 3.0 == 2) is True, 'negative float mod compared to int'
assert (7.0 % -3.0 == -2) is True, 'negative divisor compared to int'
assert (5.0 % 3.0 == 2) is True, 'positive float mod compared to int'

# === unsupported operands ===
try:
    [1] % 2
    assert False, 'expected TypeError from %'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for %: 'list' and 'int'", '% TypeError message'

# === zero divisor raises for every numeric combination ===
for a, b in [(5.0, 0.0), (5.0, 0), (5, 0.0), (5, 0)]:
    try:
        a % b
        assert False, 'expected ZeroDivisionError from %'
    except ZeroDivisionError as e:
        assert str(e) == 'division by zero', '% zero divisor message'
