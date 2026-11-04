from decimal import (
    ROUND_DOWN,
    ROUND_FLOOR,
    ROUND_HALF_UP,
    ROUND_UP,
    Clamped,
    Context,
    Decimal,
    DivisionByZero,
    DivisionImpossible,
    DivisionUndefined,
    Inexact,
    InvalidOperation,
    Overflow,
    Rounded,
    Subnormal,
    Underflow,
    getcontext,
    localcontext,
)

D = Decimal

# === Addition and subtraction ===
assert str(D('1.1') + D('2.2')) == '3.3'
assert str(D('1.10') + D('2.2')) == '3.30'
assert str(D('1') - D('0.001')) == '0.999'
assert str(D('-1.5') + D('1.5')) == '0.0'
assert str(D('1E+2') + D('1')) == '101'
assert str(D('1E+30') + D('1')) == '1.000000000000000000000000000E+30'
assert str(D('1E+30') + D('-1E-30')) == '1.000000000000000000000000000E+30'
assert str(D('123.456') - D('123.456')) == '0.000'
assert str(D('-0') + D('0')) == '0'
assert str(D('-0') + D('-0')) == '-0'
assert str(D('0.00') + D('5')) == '5.00'
assert str(D('5') + D('0E-10')) == '5.0000000000'
assert str(D('5E+40') + D('0')) == '5.000000000000000000000000000E+40'
assert str(D('0E+5') + D('0E+3')) == '0E+3'

# int operands, both directions
assert str(D('1.5') + 1) == '2.5'
assert str(1 + D('1.5')) == '2.5'
assert str(D('1.5') - 2) == '-0.5'
assert str(2 - D('1.5')) == '0.5'
assert str(True + D('1')) == '2'
assert str(D(1) + 10**30) == '1.000000000000000000000000000E+30'

# augmented assignment
total = D('0.10')
total += D('0.20')
total -= 1
assert str(total) == '-0.70'

# sum() starts from the int 0
assert str(sum([D('0.1'), D('0.2'), D('0.3')])) == '0.6'
assert str(sum([D('1.25'), D('2.50')], D('0.00'))) == '3.75'
assert str(sum([])) == '0'

# === Multiplication ===
assert str(D('1.5') * D('2')) == '3.0'
assert str(D('1.50') * D('2.0')) == '3.000'
assert str(D('-3') * D('0.25')) == '-0.75'
assert str(D('0') * D('-5')) == '-0'
assert str(D('1E+3') * D('7')) == '7E+3'
assert str(D('7') * D('1E-3')) == '0.007'
assert str(3 * D('0.1')) == '0.3'
assert str(D('19.99') * 3) == '59.97'
assert str(D('1.0825') * D('59.97')) == '64.917525'
assert str(D('123456789012345') * D('987654321098765')) == '1.219326311370210713595492539E+29'

# === Division ===
assert str(D(1) / D(3)) == '0.3333333333333333333333333333'
assert str(D(2) / D(3)) == '0.6666666666666666666666666667'
assert str(D(1) / D(4)) == '0.25'
assert str(D('1.00') / D('4')) == '0.25'
assert str(D('100') / D('4')) == '25'
assert str(D('1E+3') / D('1E+1')) == '1E+2'
assert str(D('12') / D('0.5')) == '24'
assert str(D('-7') / D('2')) == '-3.5'
assert str(D('0') / D('7')) == '0'
assert str(D('0.00') / D('7')) == '0.00'
assert str(D('0') / D('1E-5')) == '0E+5'
assert str(10 / D(4)) == '2.5'
assert str(D(10) / 4) == '2.5'

# === Floor division, modulo and divmod: truncation towards zero ===
assert str(D(7) // D(2)) == '3'
assert str(D(-7) // D(2)) == '-3'
assert str(D(7) // D(-2)) == '-3'
assert str(D('7.5') // D('2')) == '3'
assert str(D(7) % D(2)) == '1'
assert str(D(-7) % D(2)) == '-1'
assert str(D(7) % D(-2)) == '1'
assert str(D('7.5') % D('2')) == '1.5'
assert str(D('7.50') % D('0.2')) == '0.10'
assert str(D('0.5') // D('3')) == '0'
assert str(D('-0.5') // D('3')) == '-0'
assert str(D('0.5') % D('3')) == '0.5'
assert str(7 // D(2)) == '3'
assert str(7 % D(2)) == '1'
q, r = divmod(D(-7), D(2))
assert (str(q), str(r)) == ('-3', '-1')
q, r = divmod(D('10.5'), 3)
assert (str(q), str(r)) == ('3', '1.5')
q, r = divmod(10, D('3'))
assert (str(q), str(r)) == ('3', '1')
assert str(D('1E+27') // D(7)) == '142857142857142857142857142'

# === remainder_near and fma ===
assert str(D(18).remainder_near(D(10))) == '-2'
assert str(D(25).remainder_near(D(10))) == '5'
assert str(D(35).remainder_near(D(10))) == '-5'
assert str(D('10.2').remainder_near(D(1))) == '0.2'
assert str(D('10').remainder_near(D('0.3'))) == '0.1'
assert str(D('-10').remainder_near(D('3'))) == '-1'
assert str(D(2).remainder_near(D('Infinity'))) == '2'
assert str(D(0).remainder_near(D('0.01'))) == '0.00'
assert str(D(2).fma(3, 5)) == '11'
assert str(D('1.5').fma(D('2'), D('-0.5'))) == '2.5'
assert str(D('1E+30').fma(1, -1)) == '1.000000000000000000000000000E+30'
assert str(D('Infinity').fma(2, 1)) == 'Infinity'
assert str(D('NaN').fma(2, 1)) == 'NaN'
assert str(D(1).fma(D('NaN'), 1)) == 'NaN'

# === Specials ===
assert str(D('Infinity') + 1) == 'Infinity'
assert str(D('-Infinity') - D('Infinity')) == '-Infinity'
assert str(D('Infinity') * -2) == '-Infinity'
assert str(D(5) / D('Infinity')) == '0E-1000026'
assert str(D('Infinity') / 3) == 'Infinity'
assert str(D('Infinity') // 3) == 'Infinity'
assert str(D('NaN') + 1) == 'NaN'
assert str(1 - D('-NaN5')) == '-NaN5'
assert str(D('NaN') * D('Infinity')) == 'NaN'
assert str(D('NaN1') / D('NaN2')) == 'NaN1'

# === Unary operators round to the context ===
assert str(-D('1.5')) == '-1.5'
assert str(-D('0')) == '0'
assert str(+D('-0')) == '0'
assert str(abs(D('-2.50'))) == '2.50'
with localcontext() as ctx:
    ctx.prec = 3
    assert str(+D('1.23456')) == '1.23'
    assert str(-D('1.23456')) == '-1.23'
    assert str(abs(D('-9.9999'))) == '10.0'
    ctx.rounding = ROUND_FLOOR
    assert str(-D('0')) == '-0'
    assert str(D('1') - D('1')) == '-0'

# === Precision and rounding modes ===
with localcontext() as ctx:
    ctx.prec = 5
    assert str(D(1) / D(7)) == '0.14286'
    assert str(D('123456789') + 0) == '1.2346E+8'
    assert str(D('2.5') * D('1.00001')) == '2.5000'
    ctx.rounding = ROUND_DOWN
    assert str(D(2) / D(3)) == '0.66666'
    ctx.rounding = ROUND_UP
    assert str(D(1) / D(3)) == '0.33334'
    ctx.rounding = ROUND_HALF_UP
    assert str(D('1.00005') + 0) == '1.0001'
    assert str(D('-1.00005') * 1) == '-1.0001'

with localcontext(prec=2) as ctx:
    assert str(D('0.125') + D('0.0001')) == '0.13'
    assert str(D(1) / D(3)) == '0.33'

# === Flags ===
with localcontext(Context()) as ctx:
    assert str(D(1) / D(4)) == '0.25'
    assert not ctx.flags[Inexact]
    assert not ctx.flags[Rounded]
    assert str(D(1) / D(3)) == '0.3333333333333333333333333333'
    assert ctx.flags[Inexact]
    assert ctx.flags[Rounded]
    ctx.clear_flags()
    assert str(D('1.000000000000000000000000000000') + 0) == '1.000000000000000000000000000'
    assert ctx.flags[Rounded]
    assert not ctx.flags[Inexact]

# === Division by zero and invalid operations ===
try:
    D(1) / D(0)
    assert False, 'expected DivisionByZero'
except DivisionByZero as e:
    assert str(e) == "[<class 'decimal.DivisionByZero'>]"
try:
    D(1) / 0
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as e:
    assert str(e) == "[<class 'decimal.DivisionByZero'>]"
try:
    D(0) / D(0)
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.DivisionUndefined'>]"
try:
    D(5) // 0
    assert False, 'expected DivisionByZero'
except DivisionByZero as e:
    assert str(e) == "[<class 'decimal.DivisionByZero'>]"
try:
    D(5) % 0
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
try:
    D(0) % 0
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.DivisionUndefined'>]"
try:
    divmod(D(1), 0)
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>, <class 'decimal.DivisionByZero'>]"
try:
    D('Infinity') - D('Infinity')
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
try:
    D('Infinity') * 0
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
try:
    D('sNaN') + 1
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
try:
    D('1E+30') // D('0.001')
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.DivisionImpossible'>]"
try:
    D('1E+30').remainder_near(D(3))
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.DivisionImpossible'>]"
try:
    D(0).fma(D('Infinity'), 1)
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"

# === Untrapped signals give their default results ===
with localcontext(Context(traps=[])) as ctx:
    assert str(D(1) / D(0)) == 'Infinity'
    assert str(D(-1) / D(0)) == '-Infinity'
    assert str(D(0) / D(0)) == 'NaN'
    assert str(D(1) % D(0)) == 'NaN'
    q, r = divmod(D(-3), D(0))
    assert (str(q), str(r)) == ('-Infinity', 'NaN')
    assert str(D('sNaN7') + 1) == 'NaN7'
    assert str(D('-sNaN') * 2) == '-NaN'
    assert str(D('Infinity') + D('-Infinity')) == 'NaN'
    assert str(D('Infinity') % 2) == 'NaN'
    assert str(D('1E+30') // D('0.001')) == 'NaN'
    assert ctx.flags[DivisionByZero]
    assert ctx.flags[InvalidOperation]
    assert not ctx.flags[Overflow]

# === Overflow, underflow and subnormals ===
try:
    D('9E+999999') * 10
    assert False, 'expected Overflow'
except Overflow as e:
    assert str(e) == "[<class 'decimal.Overflow'>]"
with localcontext(Context(traps=[])) as ctx:
    assert str(D('9E+999999') * 10) == 'Infinity'
    assert str(D('-9E+999999') * 10) == '-Infinity'
    assert ctx.flags[Overflow]
    assert ctx.flags[Inexact]
    assert ctx.flags[Rounded]
    ctx.rounding = ROUND_DOWN
    assert str(D('9E+999999') * 10) == '9.999999999999999999999999999E+999999'
    ctx.clear_flags()
    ctx.rounding = ROUND_HALF_UP
    assert str(D('1E-999999') / 10) == '1E-1000000'
    assert ctx.flags[Subnormal]
    assert not ctx.flags[Underflow]
    assert str(D('1E-999999') * D('1E-27')) == '1E-1000026'
    assert str(D('1E-999999') * D('1E-28')) == '0E-1000026'
    assert ctx.flags[Underflow]
    assert ctx.flags[Clamped]
    assert str(D('1.5E-999999') / D('1E+27')) == '2E-1000026'

with localcontext(Context(prec=3, Emax=9, Emin=-9, traps=[])) as ctx:
    assert str(D('999') * D('1E+7')) == '9.99E+9'
    assert str(D('999') * D('1E+8')) == 'Infinity'
    ctx.clamp = 1
    assert str(+D('1E+9')) == '1.00E+9'
    assert ctx.flags[Clamped]

with localcontext(Context(traps=[Underflow, Inexact])) as ctx:
    try:
        D('1E-999999') / D('3E+30')
        assert False, 'expected Underflow'
    except Underflow as e:
        assert str(e) == "[<class 'decimal.Underflow'>, <class 'decimal.Inexact'>]"
    try:
        D(1) / 3
        assert False, 'expected Inexact'
    except Inexact as e:
        assert str(e) == "[<class 'decimal.Inexact'>]"
    assert str(D(1) / 4) == '0.25'

with localcontext(Context(traps=[Rounded])) as ctx:
    try:
        D('1.0000000000000000000000000000001') * 1
        assert False, 'expected Rounded'
    except Rounded as e:
        assert str(e) == "[<class 'decimal.Rounded'>]"

# === Operands that are not numbers ===
try:
    D(1) + 1.5
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for +: 'decimal.Decimal' and 'float'"
try:
    1.5 * D(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for *: 'float' and 'decimal.Decimal'"
try:
    D(1) / 'a'
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for /: 'decimal.Decimal' and 'str'"
try:
    D(1) - None
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for -: 'decimal.Decimal' and 'NoneType'"
try:
    D(2).fma(3, 1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'conversion from float to Decimal is not supported'
try:
    D(2).remainder_near('1')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'conversion from str to Decimal is not supported'

# === Context arithmetic methods ===
ctx = Context(prec=4)
assert str(ctx.add(D('1.23456'), 1)) == '2.235'
assert str(ctx.subtract(1, D('0.00001'))) == '1.000'
assert str(ctx.multiply(D('1.111'), D('1.111'))) == '1.234'
assert str(ctx.divide(1, 7)) == '0.1429'
assert str(ctx.divide_int(D(17), 5)) == '3'
assert str(ctx.remainder(D(-17), 5)) == '-2'
assert str(ctx.remainder_near(D(17), 5)) == '2'
assert [str(x) for x in ctx.divmod(D(17), 5)] == ['3', '2']
assert str(ctx.fma(2, 3, 4)) == '10'
assert ctx.flags[Inexact]
