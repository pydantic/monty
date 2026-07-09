from decimal import Decimal
import decimal

# === Construction from str: str() preserves coefficient and exponent ===
assert str(Decimal('0')) == '0', 'zero'
assert str(Decimal('1.23')) == '1.23', 'simple decimal'
assert str(Decimal('1.20')) == '1.20', 'trailing zero preserved'
assert str(Decimal('-1.5')) == '-1.5', 'negative'
assert str(Decimal('1E+5')) == '1E+5', 'positive exponent kept'
assert str(Decimal('123e1')) == '1.23E+3', 'exponent re-normalised like CPython'
assert str(Decimal('1e-7')) == '1E-7', 'scientific for small magnitudes'
assert str(Decimal('0.000001')) == '0.000001', 'plain form near zero'
assert str(Decimal('0E-10')) == '0E-10', 'zero with exponent'
assert str(Decimal('-0')) == '-0', 'negative zero'

# === Construction from str: special values ===
assert str(Decimal('inf')) == 'Infinity', 'lowercase inf'
assert str(Decimal('Infinity')) == 'Infinity', 'Infinity'
assert str(Decimal('-inf')) == '-Infinity', 'negative infinity'
assert str(Decimal('nan')) == 'NaN', 'lowercase nan'
assert str(Decimal('NaN')) == 'NaN', 'NaN'

# === Construction from str: whitespace and underscores (CPython-accepted) ===
assert str(Decimal('  1.5  ')) == '1.5', 'surrounding whitespace stripped'
assert str(Decimal('1_000')) == '1000', 'underscore separators'
assert str(Decimal('1_0.0_1')) == '10.01', 'underscores around the point'

# === Construction from int / bool / Decimal ===
assert str(Decimal(123)) == '123', 'from int'
assert str(Decimal(-45)) == '-45', 'from negative int'
assert str(Decimal(0)) == '0', 'from int zero'
assert str(Decimal(True)) == '1', 'from bool True'
assert str(Decimal(False)) == '0', 'from bool False'
assert str(Decimal()) == '0', 'no argument is zero'
assert str(Decimal(Decimal('7.50'))) == '7.50', 'copy preserves scale'
assert str(Decimal(10**18)) == '1000000000000000000', 'large int that fits'

# === Construction at the representable exponent boundary (matches CPython) ===
# Monty's fixed-width D256 caps the scale at ±16384; values exactly at the cap
# still round-trip and stay non-zero. Magnitudes past the cap raise
# decimal.Overflow (monty-specific, so tested in tests/decimal_range.rs).
assert str(Decimal('1E-16384')) == '1E-16384', 'smallest representable kept'
assert bool(Decimal('1E-16384')) is True, 'boundary value is non-zero'
assert str(Decimal('1E+16384')) == '1E+16384', 'largest representable kept'

# === Construction from float is the exact binary expansion ===
assert str(Decimal(0.5)) == '0.5', 'exact float 0.5'
assert str(Decimal(0.1)) == '0.1000000000000000055511151231257827021181583404541015625', 'exact float 0.1'
assert str(Decimal(2.0)) == '2', 'integral float'

# === repr() wraps str() in Decimal('...') ===
assert repr(Decimal('1.23')) == "Decimal('1.23')", 'repr basic'
assert repr(Decimal('1.20')) == "Decimal('1.20')", 'repr trailing zero'
assert repr(Decimal('1E+5')) == "Decimal('1E+5')", 'repr exponent'
assert repr(Decimal('-Infinity')) == "Decimal('-Infinity')", 'repr -inf'
assert repr(Decimal('NaN')) == "Decimal('NaN')", 'repr nan'

# === bool(): only zero is falsy ===
assert bool(Decimal('1.5')) is True, 'nonzero is truthy'
assert bool(Decimal('-2')) is True, 'negative is truthy'
assert bool(Decimal('0')) is False, 'zero is falsy'
assert bool(Decimal('0.0')) is False, 'scaled zero is falsy'
assert bool(Decimal('NaN')) is True, 'NaN is truthy'
assert bool(Decimal('Infinity')) is True, 'infinity is truthy'

# === isinstance ===
assert isinstance(Decimal('1'), Decimal), 'isinstance true'
assert not isinstance(1, Decimal), 'int is not Decimal'
assert not isinstance(1.0, Decimal), 'float is not Decimal'

# === Rounding-mode constants are plain strings equal to their names ===
assert decimal.ROUND_HALF_EVEN == 'ROUND_HALF_EVEN', 'half even'
assert decimal.ROUND_CEILING == 'ROUND_CEILING', 'ceiling'
assert decimal.ROUND_FLOOR == 'ROUND_FLOOR', 'floor'
assert decimal.ROUND_UP == 'ROUND_UP', 'up'
assert decimal.ROUND_DOWN == 'ROUND_DOWN', 'down'
assert decimal.ROUND_HALF_UP == 'ROUND_HALF_UP', 'half up'
assert decimal.ROUND_HALF_DOWN == 'ROUND_HALF_DOWN', 'half down'
assert decimal.ROUND_05UP == 'ROUND_05UP', '05up'

# === Construction errors ===
# Unparsable string -> InvalidOperation([ConversionSyntax])
try:
    Decimal('abc')
    assert False, 'expected InvalidOperation'
except decimal.InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.ConversionSyntax'>]", 'conversion syntax message'

# Empty string -> InvalidOperation([ConversionSyntax])
try:
    Decimal('')
    assert False, 'expected InvalidOperation'
except decimal.InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.ConversionSyntax'>]", 'empty string message'

# InvalidOperation participates in the ArithmeticError / DecimalException tree
try:
    Decimal('not a number')
    assert False, 'expected ArithmeticError'
except ArithmeticError:
    pass

try:
    Decimal('not a number')
    assert False, 'expected DecimalException'
except decimal.DecimalException:
    pass

# None -> TypeError
try:
    Decimal(None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'conversion from NoneType to Decimal is not supported', 'none message'

# === Signaling NaN and NaN payloads ===
assert str(Decimal('snan')) == 'sNaN', 'sNaN spelling'
assert repr(Decimal('-snan123')) == "Decimal('-sNaN123')", 'signed sNaN payload repr'
assert str(Decimal('-nan456')) == '-NaN456', 'signed quiet NaN payload'
assert str(Decimal('nan0')) == 'NaN', 'zero payload prints bare NaN'
assert Decimal('sNaN').is_snan(), 'is_snan'
assert not Decimal('sNaN').is_qnan(), 'sNaN is not qNaN'
assert Decimal('sNaN').is_nan(), 'sNaN is a NaN'
assert Decimal('nan123').is_qnan(), 'payload NaN is quiet'
assert (Decimal('NaN123') + 1).is_qnan(), 'quiet NaN propagates through arithmetic'
assert str(Decimal('NaN123') + 1) == 'NaN123', 'payload survives propagation'
tup = Decimal('sNaN45').as_tuple()
assert tup.sign == 0 and tup.digits == (4, 5) and tup.exponent == 'N', 'sNaN as_tuple'
assert Decimal('NaN').as_tuple().digits == (), 'empty payload digits'
assert str(Decimal((1, (4, 5), 'N'))) == '-sNaN45', 'sNaN from tuple form'

# === Huge exponent literals (C-module bounds) ===
assert str(Decimal('1E+425000000')) == '1E+425000000', 'huge exponent literal'
assert str(Decimal('1E-32768')) == '1E-32768', 'tiny magnitude literal'
assert str(Decimal('1E+999999999999999998')) == '1E+999999999999999998', 'near-max exponent'

# === value= keyword ===
assert str(Decimal(value='3')) == '3', 'value keyword argument'
