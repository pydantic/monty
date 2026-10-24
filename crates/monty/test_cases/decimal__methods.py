import copy
import math
from decimal import (
    ROUND_CEILING,
    ROUND_DOWN,
    ROUND_FLOOR,
    ROUND_HALF_DOWN,
    ROUND_HALF_UP,
    ROUND_UP,
    Context,
    Decimal,
    DecimalTuple,
    Inexact,
    InvalidOperation,
    Rounded,
    getcontext,
    localcontext,
    setcontext,
)

# CPython runs cases on shared threads, so start from a fresh default context.
setcontext(Context())
D = Decimal

# === quantize ===
assert D('1.41421356').quantize(D('1.000')) == D('1.414')
assert str(D('7.325').quantize(D('.01'), rounding=ROUND_DOWN)) == '7.32'
assert str(D('7.325').quantize(D('1.'), rounding=ROUND_UP)) == '8'
assert str(D('2.675').quantize(D('0.01'), ROUND_HALF_UP)) == '2.68'
assert str(D('2.665').quantize(D('0.01'))) == '2.66'
assert str(D('2.675').quantize(D('0.01'), ROUND_HALF_DOWN)) == '2.67'
assert str(D('-2.675').quantize(D('0.01'), ROUND_CEILING)) == '-2.67'
assert str(D('-2.675').quantize(D('0.01'), ROUND_FLOOR)) == '-2.68'
assert str(D('123').quantize(D('1e1'))) == '1.2E+2'
assert str(D('0').quantize(D('1e-5'))) == '0.00000'
assert str(D('-0').quantize(D('1e2'))) == '-0E+2'
assert str(D('1.5').quantize(D('1e-3'))) == '1.500'
assert str(D('inf').quantize(D('-inf'))) == 'Infinity'
assert str(D('nan').quantize(D(1))) == 'NaN'
assert str(D('1.23').quantize(2)) == '1'
with localcontext() as ctx:
    ctx.traps[InvalidOperation] = False
    assert str(D('inf').quantize(D(1))) == 'NaN'
    assert str(D(1).quantize(D('inf'))) == 'NaN'
    assert str(D('1e30').quantize(D(1))) == 'NaN'
    assert ctx.flags[InvalidOperation]
try:
    D(1).quantize(D('1e-30'))
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
try:
    D(1).quantize(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'conversion from float to Decimal is not supported'
try:
    D(1).quantize(D(1), 'ROUND_SIDEWAYS')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == (
        'valid values for rounding are:\n'
        '  [ROUND_CEILING, ROUND_FLOOR, ROUND_UP, ROUND_DOWN,\n'
        '   ROUND_HALF_UP, ROUND_HALF_DOWN, ROUND_HALF_EVEN,\n'
        '   ROUND_05UP]'
    )
try:
    D(1).quantize()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'exp' (pos 1)"
try:
    D(1).quantize(D(1), None, None, None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'function takes at most 3 arguments (4 given)'
try:
    D(1).quantize(D(1), context=5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'optional argument must be a context'
assert str(D('1.2345').quantize(exp=D('0.01'), rounding=ROUND_UP, context=Context(prec=3))) == '1.24'

# === same_quantum ===
assert D('2.17').same_quantum(D('7.01'))
assert not D('2.17').same_quantum(D('7.0'))
assert D('inf').same_quantum(D('-inf'))
assert D('nan').same_quantum(D('snan'))
assert not D('nan').same_quantum(D(1))
assert D(1).same_quantum(5)

# === to_integral ===
assert str(D('2.5').to_integral()) == '2'
assert str(D('3.5').to_integral()) == '4'
assert str(D('-2.5').to_integral_value(ROUND_HALF_UP)) == '-3'
assert str(D('1.9').to_integral_exact(rounding=ROUND_DOWN)) == '1'
assert str(D('1e3').to_integral()) == '1E+3'
assert str(D('-0.5').to_integral()) == '-0'
assert str(D('inf').to_integral()) == 'Infinity'
with localcontext() as ctx:
    ctx.clear_flags()
    D('7.5').to_integral_value()
    assert not ctx.flags[Rounded]
    D('7.0').to_integral_exact()
    assert ctx.flags[Rounded]
    assert not ctx.flags[Inexact]
    D('7.5').to_integral_exact()
    assert ctx.flags[Inexact]

# === normalize ===
assert str(D('1.200').normalize()) == '1.2'
assert str(D('120.0').normalize()) == '1.2E+2'
assert str(D('-0.00').normalize()) == '-0'
assert str(D('0E+5').normalize()) == '0'
assert str(D('1.23456789012345678901234567890').normalize()) == '1.234567890123456789012345679'
assert str(D('1.2300').normalize(Context(prec=2))) == '1.2'

# === round() and integer conversions ===
assert round(D('2.5')) == 2
assert round(D('3.5')) == 4
assert round(D('-2.5')) == -2
assert type(round(D('1.5'))) is int
assert str(round(D('2.675'), 2)) == '2.68'
assert str(round(D('123.456'), -1)) == '1.2E+2'
assert str(round(D('123.456'), -5)) == '0E+5'
assert str(round(D('nan'), 2)) == 'NaN'
assert str(round(D('1.25'), 1)) == '1.2'
assert int(D('-7.9')) == -7
assert int(D('1e30')) == 10**30
assert math.floor(D('-1.5')) == -2
assert math.ceil(D('-1.5')) == -1
assert math.trunc(D('-1.5')) == -1
assert math.floor(D('2')) == 2
try:
    round(D('nan'))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'cannot convert NaN to integer'
try:
    int(D('-inf'))
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'cannot convert Infinity to integer'
try:
    math.floor(D('snan'))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'cannot convert NaN to integer'
try:
    round(D('1.5'), 1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'optional arg must be an integer'
try:
    round(D('123.456'), 50)
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"

# === float and complex ===
assert float(D('1.1')) == 1.1
assert float(D('-0')) == 0.0
assert str(float(D('-0'))) == '-0.0'
assert float(D('1e400')) == float('inf')
assert float(D('-inf')) == float('-inf')
assert str(float(D('nan'))) == 'nan'
assert str(float(D('-nan'))) == 'nan'
assert float(D('0.1000000000000000055511151231257827021181583404541015625')) == 0.1
assert complex(D('2.5')) == 2.5 + 0j
assert complex(D('1.5'), D('2')) == 1.5 + 2j
assert complex(1, D('-0.5')) == 1 - 0.5j
try:
    float(D('snan'))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'cannot convert signaling NaN to float'

# === as_integer_ratio and as_tuple ===
assert D('3.14').as_integer_ratio() == (157, 50)
assert D('-123e5').as_integer_ratio() == (-12300000, 1)
assert D('0.00').as_integer_ratio() == (0, 1)
assert D('0.125').as_integer_ratio() == (1, 8)
try:
    D('nan').as_integer_ratio()
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'cannot convert NaN to integer ratio'
try:
    D('inf').as_integer_ratio()
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'cannot convert Infinity to integer ratio'
assert D('-3.14').as_tuple() == DecimalTuple(sign=1, digits=(3, 1, 4), exponent=-2)
assert D('-3.14').as_tuple() == (1, (3, 1, 4), -2)
assert D('inf').as_tuple() == (0, (0,), 'F')
assert D('-snan12').as_tuple() == (1, (1, 2), 'N')
assert D('nan').as_tuple() == (0, (), 'n')
t = D('1.50').as_tuple()
assert t.sign == 0
assert t.digits == (1, 5, 0)
assert t.exponent == -2
assert D(t) == D('1.50')

# === next_minus / next_plus / next_toward ===
assert str(D(1).next_minus()) == '0.9999999999999999999999999999'
assert str(D(1).next_plus()) == '1.000000000000000000000000001'
assert str(D('-1e-1000026').next_minus()) == '-2E-1000026'
assert str(D('inf').next_minus()) == '9.999999999999999999999999999E+999999'
assert str(D('-inf').next_plus()) == '-9.999999999999999999999999999E+999999'
assert str(D('-inf').next_minus()) == '-Infinity'
assert str(D(0).next_plus()) == '1E-1000026'
assert str(D(1).next_toward(2)) == '1.000000000000000000000000001'
assert str(D(1).next_toward(0)) == '0.9999999999999999999999999999'
assert str(D('-0').next_toward(0)) == '0'
assert str(D(1).next_toward(D(1))) == '1'

# === number_class, logb, scaleb ===
assert D('1.5').number_class() == '+Normal'
assert D('-0').number_class() == '-Zero'
assert D('1e-1000000').number_class() == '+Subnormal'
assert D('-inf').number_class() == '-Infinity'
assert D('snan').number_class() == 'sNaN'
assert D('-nan').number_class() == 'NaN'
assert str(D('250').logb()) == '2'
assert str(D('0.03').logb()) == '-2'
assert str(D('inf').logb()) == 'Infinity'
assert str(D('7.50').scaleb(3)) == '7.50E+3'
assert str(D('7.50').scaleb(-2)) == '0.0750'
assert str(D('inf').scaleb(5)) == 'Infinity'
try:
    D(0).logb()
    assert False, 'expected DivisionByZero'
except ZeroDivisionError as e:
    assert str(e) == "[<class 'decimal.DivisionByZero'>]"
try:
    D(1).scaleb(D('1.5'))
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"

# === rotate and shift ===
assert str(D('123456789').rotate(-2)) == '8900000000000000000001234567'
assert str(D('123456789').rotate(2)) == '12345678900'
assert str(D('123456789').shift(2)) == '12345678900'
assert str(D('123456789').shift(-2)) == '1234567'
assert str(D('-12').shift(1)) == '-120'
assert str(D('inf').rotate(3)) == 'Infinity'
with localcontext(prec=5):
    assert str(D('1234567').rotate(1)) == '45673'
    assert str(D('1234567').shift(3)) == '67000'
try:
    D(1).rotate(29)
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"

# === logical operations ===
assert str(D('1100').logical_and(D('1010'))) == '1000'
assert str(D('1100').logical_or(D('1010'))) == '1110'
assert str(D('1100').logical_xor(D('1010'))) == '110'
assert str(D('0').logical_invert()) == '1111111111111111111111111111'
assert str(D('101').logical_invert(Context(prec=5))) == '11010'
try:
    D('12').logical_and(D(1))
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"

# === small predicates and copies ===
assert D('1.20').adjusted() == 0
assert D('-1.2e5').adjusted() == 5
assert D('inf').adjusted() == 0
assert D(5).canonical() == D(5)
assert D(5).is_canonical()
assert D('1').is_normal()
assert not D('0').is_normal()
assert D('1e-1000000').is_subnormal()
assert D('-0').is_signed()
assert D('-nan').is_nan()
assert D('snan').is_snan()
assert not D('snan').is_qnan()
assert D('inf').is_infinite()
assert not D('inf').is_finite()
assert D('0e9').is_zero()
assert str(D('-1.5').copy_abs()) == '1.5'
assert str(D('1.5').copy_negate()) == '-1.5'
assert str(D('1.5').copy_sign(D('-2'))) == '-1.5'
assert str(D('1.5').copy_sign(-2)) == '-1.5'
assert str(D(7).radix()) == '10'
assert D('1.5').real == D('1.5')
assert D('1.5').imag == D(0)
assert D('1.5').conjugate() == D('1.5')
try:
    D(1).is_nan(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Decimal.is_nan() takes no arguments (1 given)'
try:
    D(1).copy_sign(2.0)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'conversion from float to Decimal is not supported'
try:
    D(1).compare()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'other' (pos 1)"
try:
    D(1).sqrt(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'optional argument must be a context'
try:
    D(1).foo()
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'decimal.Decimal' object has no attribute 'foo'"
try:
    D(1).real = 5
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "attribute 'real' of 'decimal.Decimal' objects is not writable"

# === comparisons returning Decimals ===
assert str(D(1).compare(2)) == '-1'
assert str(D(2).compare(D('2.0'))) == '0'
assert str(D('nan').compare(1)) == 'NaN'
assert str(D(1).compare_total(D('1.0'))) == '1'
assert str(D('-1').compare_total_mag(D('1.0'))) == '1'
assert str(D(3).max(D('nan'))) == '3'
assert str(D(-3).max_mag(D(2))) == '-3'
assert str(D(1).min(D('1.0'))) == '1.0'
assert str(D(-3).min_mag(D(2))) == '2'
try:
    D(1).compare_signal(D('nan'))
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"

# === construction ===
assert str(D.from_float(0.1)) == '0.1000000000000000055511151231257827021181583404541015625'
assert str(D.from_float(-0.0)) == '-0'
assert str(D.from_float(float('-inf'))) == '-Infinity'
assert str(D.from_float(7)) == '7'
assert str(D.from_number(D('1.5'))) == '1.5'
assert str(D.from_number(2.5)) == '2.5'
assert str(D((1, (0, 0, 1, 2), -2))) == '-0.12'
assert str(D([0, [1], 0])) == '1'
assert str(D((0, (), 'F'))) == 'Infinity'
assert str(D((1, (1,), 'n'))) == '-NaN1'
assert str(D('  -1_000.5  ')) == '-1000.5'
assert str(D('١٢')) == '12'
try:
    D.from_float('1')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'argument must be int or float'
try:
    D.from_number('1')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'conversion from str to Decimal is not supported'
try:
    D(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'conversion from NoneType to Decimal is not supported'
try:
    D((0, (1,)))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'argument must be a sequence of length 3'
try:
    D((2, (1,), 0))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'sign must be an integer with the value 0 or 1'
try:
    D((0, (10,), 0))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'coefficient must be a tuple of digits'
try:
    D((0, (1,), 'x'))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "string argument in the third position must be 'F', 'n' or 'N'"
try:
    D((0, (1,), 1.5))
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'exponent must be an integer'
try:
    D(1, 2, 3)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'function takes at most 2 arguments (3 given)'
try:
    D('1', 5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'optional argument must be a context'

# === copy and pickling support ===
d = D('1.50')
assert copy.copy(d) is d
assert copy.deepcopy(d) is d
assert d.__copy__() is d
assert d.__deepcopy__({}) is d
assert d.__reduce__() == (Decimal, ('1.50',))
assert str(d.__round__(1)) == '1.5'
assert d.__float__() == 1.5
assert d.__int__() == 1
assert d.__floor__() == 1
assert d.__ceil__() == 2
assert d.__complex__() == 1.5 + 0j
