from decimal import Decimal as D
import decimal

# === Exact / short results ===
assert str(D('1.23') + D('4.56')) == '5.79', 'add'
assert str(D('1.1') + D('2.2')) == '3.3', 'add tenths'
assert str(D(10) - D(3)) == '7', 'sub'
assert str(D('1.5') * D('1.5')) == '2.25', 'mul'
assert str(D(10) / D(4)) == '2.5', 'div exact'
assert str(D(2) ** D(10)) == '1024', 'pow'
assert str(D(7) % D(3)) == '1', 'mod'
assert str(D(7) // D(3)) == '2', 'floordiv'
assert str(D(-7) % D(3)) == '-1', 'mod negative (sign of divisor)'
assert str(D(-7) // D(3)) == '-2', 'floordiv truncates toward zero (not floor)'

# === prec-28 rounding of inexact results ===
assert str(D(1) / D(3)) == '0.3333333333333333333333333333', '1/3 to 28 digits'
assert str(D(2) / D(3)) == '0.6666666666666666666666666667', '2/3 rounds half-even up'
assert str(D(100) / D(7)) == '14.28571428571428571428571429', '100/7 to 28 digits'

# === Mixed Decimal / int / bool (both orders) ===
assert str(D(5) + 2) == '7', 'decimal + int'
assert str(2 + D(5)) == '7', 'int + decimal'
assert str(10 - D(3)) == '7', 'int - decimal'
assert str(D(5) * 3) == '15', 'decimal * int'
assert str(D(10) / 2) == '5', 'decimal / int'
assert str(D(7) % 2) == '1', 'decimal % int'
assert str(D(7) // 2) == '3', 'decimal // int'
assert str(D(2) ** 3) == '8', 'decimal ** int'
assert str(D(5) + True) == '6', 'decimal + bool'
assert str(5**40 + D(0)) == '9094947017729282379150390625', 'bigint + decimal (fits D256)'

# === Unary ===
assert str(-D(5)) == '-5', 'unary minus'
assert str(+D(5)) == '5', 'unary plus'
assert str(-D('-1.5')) == '1.5', 'unary minus negative'
assert str(-D('2.5')) == '-2.5', 'unary minus on decimal'
# unary + / - round to the working precision
assert str(+D(D(0.1))) == '0.1000000000000000055511151231', 'unary plus rounds 55-digit float to 28'
assert str(-D(D(0.1))) == '-0.1000000000000000055511151231', 'unary minus rounds to 28'

# === Infinity arithmetic (no raise) ===
assert str(D('inf') + D('inf')) == 'Infinity', 'inf + inf'
assert str(D('inf') + D(1)) == 'Infinity', 'inf + finite'
assert str(D('inf') * D(2)) == 'Infinity', 'inf * 2'
assert str(D('-inf') * D(2)) == '-Infinity', '-inf * 2'
# 1/inf is zero (CPython keeps a huge negative exponent, `0E-1000026`; Monty
# clamps it — compare by value, which is zero in both).
assert D(1) / D('inf') == 0, '1 / inf is zero'

# === NaN propagation (no raise) ===
assert str(D('NaN') + D(1)) == 'NaN', 'NaN + 1'
assert str(D('NaN') * D(2)) == 'NaN', 'NaN * 2'
assert str(D(1) / D('NaN')) == 'NaN', '1 / NaN'
assert str(D('NaN') % D(2)) == 'NaN', 'NaN % 2'
assert str(-D('NaN')) == 'NaN', 'unary minus NaN'

# === Power special values (no raise) ===
assert str(D(-8) ** D(3)) == '-512', 'negative base, odd integer exponent'
assert str(D(-8) ** D(2)) == '64', 'negative base, even integer exponent'
assert str(D(2) ** D('3.0')) == '8', 'integer-valued decimal exponent is exact'
assert str(D(0) ** D(-1)) == 'Infinity', '0 ** -1 is Infinity (not an error)'
assert str(D(0) ** D(2)) == '0', '0 ** positive is 0'
assert str(D(2) ** D('inf')) == 'Infinity', '2 ** inf'
assert str(D(2) ** D('-inf')) == '0', '2 ** -inf'
assert str(D('inf') ** D(2)) == 'Infinity', 'inf ** 2'
assert str(D('inf') ** D(0)) == '1', 'inf ** 0 is 1'
assert str(D(2) ** D('NaN')) == 'NaN', '2 ** NaN propagates quietly (no fastnum panic)'
assert str(D('NaN') ** D(2)) == 'NaN', 'NaN ** 2'
assert str(D(2) ** D('0.5')) == '1.414213562373095048801688724', 'sqrt(2) via ** rounds to prec'
assert str(D('0.1') ** D('0.1')) == '0.7943282347242815020659182828', 'fractional base and exponent'
# A representable large power is computed; one beyond Monty's range raises Overflow
# (see decimal__errors.py and limitations/decimal.md for the smaller-range cutoff).
assert str(D(10) ** D('16400')) == '1.000000000000000000000000000E+16400', 'large in-range power'

# === Floor division / modulo with infinity (no raise) ===
assert str(D('inf') // D(2)) == 'Infinity', 'inf // finite is signed infinity'
assert str(D('-inf') // D(2)) == '-Infinity', '-inf // finite'
assert str(D('inf') // D(-2)) == '-Infinity', 'inf // negative finite flips sign'
assert str(D(2) // D('inf')) == '0', 'finite // inf is 0'
assert str(D(2) % D('inf')) == '2', 'finite % inf is the dividend'
assert str(D('inf') // D(0)) == 'Infinity', 'inf // 0 is Infinity (infinity precedes zero check)'

# === Integer-division remainder keeps the ideal exponent ===
assert str(D('7.0') % D(3)) == '1.0', 'remainder scale = min(operand exponents)'
assert str(D(7) % D('3.00')) == '1.00', 'remainder takes the divisor scale'
assert str(D(10) // D('0.003')) == '3333', 'floordiv across differing scales'
assert str(D('1e20') // D(3)) == '33333333333333333333', 'large in-precision quotient'
assert str(D('1e27') // D(1)) == '1000000000000000000000000000', '28-digit quotient still fits'

# === divmod ===
q, r = divmod(D(7), D(3))
assert str(q) == '2' and str(r) == '1', 'divmod decimals'
q, r = divmod(D(7), 2)
assert str(q) == '3' and str(r) == '1', 'divmod decimal/int'
q, r = divmod(D(-7), D(3))
assert str(q) == '-2' and str(r) == '-1', 'divmod negative'
q, r = divmod(D(10), D('0.003'))
assert str(q) == '3333' and str(r) == '0.001', 'divmod across differing scales'
q, r = divmod(D('NaN'), D(2))
assert str(q) == 'NaN' and str(r) == 'NaN', 'divmod with NaN dividend'
q, r = divmod(D(2), D('inf'))
assert str(q) == '0' and str(r) == '2', 'divmod by infinity'
# divmod always agrees with the separate // and % operators
assert divmod(D(17), D(5)) == (D(17) // D(5), D(17) % D(5)), 'divmod == (//, %)'

# === Float mixing raises TypeError (both orders, all ops) ===
for fn, msg in [
    (lambda: D('1.5') + 1.5, "unsupported operand type(s) for +: 'decimal.Decimal' and 'float'"),
    (lambda: 1.5 + D('1.5'), "unsupported operand type(s) for +: 'float' and 'decimal.Decimal'"),
    (lambda: D(1) * 2.0, "unsupported operand type(s) for *: 'decimal.Decimal' and 'float'"),
    (lambda: D(1) / 2.0, "unsupported operand type(s) for /: 'decimal.Decimal' and 'float'"),
    (lambda: D(1) - 2.0, "unsupported operand type(s) for -: 'decimal.Decimal' and 'float'"),
]:
    try:
        fn()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == msg, f'float-mix message: {exc}'

# === Signs of zero results (CPython minus/plus/_divide parity) ===
assert str(-D('0')) == '0', 'unary minus of +0 is +0'
assert str(-D('-0')) == '0', 'unary minus of -0 is +0'
assert str(+D('-0')) == '0', 'unary plus of -0 is +0'
assert not (-D('0')).is_signed(), 'negated zero is unsigned'
assert str(D(-1) // D(1000)) == '-0', 'zero floordiv quotient keeps the sign'
assert str(D(0) // D(-3)) == '-0', 'signed zero quotient'
assert str(D(-7) // D('inf')) == '-0', 'zero quotient by infinity keeps the sign'
q, r = divmod(D(-1), D(1000))
assert str(q) == '-0' and str(r) == '-1', 'divmod zero-quotient sign'

# === (-1) ** n parity for integral exponents in any representation ===
assert str(D(-1) ** D('3.0')) == '-1', 'odd integral exponent with trailing zero'
assert str(D(-1) ** D('30E-1')) == '-1', 'odd integral exponent with fractional zeros'
assert str(D(-1) ** D('4.0')) == '1', 'even integral exponent'

# === Huge int operands promote exactly ===
assert str(D(1) + 10**100) == '1.000000000000000000000000000E+100', 'huge int operand'
assert D(10**50) == 10**50, '51-digit int equality'

# === Emax / Etiny boundaries ===
assert str(D('9.999999999999999999999999999E+999999') + D('0')) == '9.999999999999999999999999999E+999999', (
    'Emax boundary value survives'
)
try:
    D('9E+999999') * 10
    assert False, 'expected Overflow past Emax'
except decimal.Overflow as exc:
    assert str(exc) == "[<class 'decimal.Overflow'>]", 'overflow message'
# A result below Etiny quietly underflows to a signed zero at Etiny.
assert str(D('1E-1000026') / D('1E+10')) == '0E-1000026', 'underflow to zero at Etiny'

# === pow() builtin ===
assert pow(D(2), 2) == 4, 'pow builtin decimal/int'
assert pow(2, D(2)) == 4, 'pow builtin int/decimal'
assert str(pow(D(2), D(3))) == '8', 'pow builtin decimal/decimal'
assert str(pow(D(2), D(3), D(5))) == '3', '3-arg pow all decimal'
assert str(pow(D(2), 3, 5)) == '3', '3-arg pow mixed int'
assert str(pow(D(-2), D(3), D(-5))) == '-3', '3-arg pow signs'
assert str(pow(True, D(3), 7)) == '1', '3-arg pow bool base promotes'
assert str(pow(D(2), 3, True)) == '0', '3-arg pow bool modulus promotes'
try:
    pow(D(2), D('3.5'), D(5))
    assert False, 'expected InvalidOperation for non-integral 3-arg pow'
except decimal.InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", '3-arg pow message'
# 2-arg pow with a non-promotable operand keeps the operator's message
try:
    pow(D(2), 1.5)
    assert False, 'expected TypeError from pow(Decimal, float)'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for ** or pow(): 'decimal.Decimal' and 'float'", 'pow d/f message'
try:
    pow(1.5, D(2))
    assert False, 'expected TypeError from pow(float, Decimal)'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for ** or pow(): 'float' and 'decimal.Decimal'", 'pow f/d message'
# 3-arg pow with a Decimal and a non-promotable operand raises the
# three-operand message — unless a float is present, whose `__pow__` wins
# with the integers-only message.
for fn, expected in [
    (lambda: pow(2, D(3), 'x'), "unsupported operand type(s) for ** or pow(): 'int', 'decimal.Decimal', 'str'"),
    (lambda: pow(True, D(3), 'x'), "unsupported operand type(s) for ** or pow(): 'bool', 'decimal.Decimal', 'str'"),
    (lambda: pow(D(2), 'x', True), "unsupported operand type(s) for ** or pow(): 'decimal.Decimal', 'str', 'bool'"),
    (lambda: pow('x', D(3), 5), "unsupported operand type(s) for ** or pow(): 'str', 'decimal.Decimal', 'int'"),
    (lambda: pow(D(2), [1], 5), "unsupported operand type(s) for ** or pow(): 'decimal.Decimal', 'list', 'int'"),
    (lambda: pow(D(2), 3, 1.5), 'pow() 3rd argument not allowed unless all arguments are integers'),
    (lambda: pow(D(2), 1.5, 5), 'pow() 3rd argument not allowed unless all arguments are integers'),
    (lambda: pow(1.5, D(2), 3), 'pow() 3rd argument not allowed unless all arguments are integers'),
]:
    try:
        fn()
        assert False, 'expected TypeError from 3-arg pow'
    except TypeError as exc:
        assert str(exc) == expected, f'3-arg pow TypeError message: {exc}'

# === Sequence repetition by a Decimal raises CPython's non-int message ===
for fn in [lambda: 'a' * D(2), lambda: D(2) * 'a', lambda: [1] * D(2), lambda: D(2) * (1, 2), lambda: b'x' * D(2)]:
    try:
        fn()
        assert False, 'expected TypeError from sequence * Decimal'
    except TypeError as exc:
        assert str(exc) == "can't multiply sequence by non-int of type 'decimal.Decimal'", f'seq-repeat message: {exc}'
# `range` has no repeat, so it keeps the generic message
try:
    D(2) * range(3)
    assert False, 'expected TypeError from range * Decimal'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for *: 'decimal.Decimal' and 'range'", 'range keeps generic message'
