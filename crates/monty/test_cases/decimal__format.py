from decimal import ROUND_DOWN, ROUND_UP, Context, Decimal, localcontext


def format_error(value, spec):
    try:
        format(value, spec)
    except (ValueError, OverflowError, TypeError) as exc:
        return f'{type(exc).__name__}: {exc}'
    return 'no error'


x = Decimal('1234567.891')

# === str, repr and to_eng_string ===
assert str(Decimal('1.50')) == '1.50'
assert repr(Decimal('-1e-7')) == "Decimal('-1E-7')"
assert str(Decimal('1E+5')) == '1E+5'
assert str(Decimal('0.000001')) == '0.000001'
assert str(Decimal('1E-7')) == '1E-7'
assert str(Decimal('-sNaN12')) == '-sNaN12'
assert str(Decimal('-Infinity')) == '-Infinity'
assert Decimal('123e7').to_eng_string() == '1.23E+9'
assert Decimal('0e7').to_eng_string() == '0.00E+9'
assert Decimal('-0.00001234').to_eng_string() == '-0.00001234'
assert Decimal('1E-7').to_eng_string() == '100E-9'
assert Decimal('-0E-8').to_eng_string() == '-0.00E-6'

# === capitals=0 lowers the exponent character ===
with localcontext() as ctx:
    ctx.capitals = 0
    assert str(Decimal('1e10')) == '1e+10'
    assert repr(Decimal('1e10')) == "Decimal('1e+10')"
    assert Decimal('1e10').to_eng_string() == '10e+9'
    assert format(Decimal('1e10'), '') == '1e+10'
    assert format(Decimal('1e10'), 'G') == '1E+10'
    assert format(Decimal('-inf'), 'e') == '-Infinity'

# === default and fixed-point formatting ===
assert format(x, '') == '1234567.891'
assert format(x, 'f') == '1234567.891'
assert format(x, 'F') == '1234567.891'
assert format(x, '.2f') == '1234567.89'
assert format(x, '.0f') == '1234568'
assert format(x, '#.0f') == '1234568.'
assert format(Decimal('2.5'), '.0f') == '2'
assert format(Decimal('1.23456789E-8'), 'f') == '0.0000000123456789'
assert format(Decimal('1.23456E-8'), '.20f') == '0.00000001234560000000'
assert format(Decimal('1E+5'), 'f') == '100000'
assert format(Decimal('0E+5'), 'f') == '0'
assert format(Decimal('0E+5'), '.2f') == '0.00'
assert format(Decimal('1E+5'), '') == '1E+5'
assert format(Decimal('0E+5'), '') == '0E+5'

# === exponent formatting ===
assert format(x, 'e') == '1.234567891e+6'
assert format(x, 'E') == '1.234567891E+6'
assert format(x, '.3e') == '1.235e+6'
assert format(x, '.0e') == '1e+6'
assert format(Decimal('5'), '#.0e') == '5.e+0'
assert format(Decimal('-0'), '.2e') == '-0.00e+2'
assert format(Decimal('0E+5'), 'e') == '0e+5'
assert format(Decimal('1.01'), '030,.4e') == '00,000,000,000,000,001.0100e+0'

# === general formatting ===
assert format(x, 'g') == '1234567.891'
assert format(x, '.3g') == '1.23e+6'
assert format(x, '.0g') == '1e+6'
assert format(x, '.10g') == '1234567.891'
assert format(Decimal('1.23456789'), '.3') == '1.23'
assert format(Decimal('1.23456E-8'), '.5') == '1.2346E-8'
assert format(Decimal('1.23456E-8'), 'g') == '1.23456e-8'
assert format(Decimal('99.97'), '.3') == '100'
assert format(Decimal('5'), '.0') == '5'
assert format(Decimal('5'), '#.0') == '5.'
assert format(Decimal('0.000001'), 'g') == '0.000001'
assert format(Decimal('1E-7'), 'g') == '1e-7'

# === the n type uses the C locale ===
assert format(x, 'n') == '1234567.891'
assert format(x, '.3n') == '1.23e+6'
assert format(Decimal('5'), '.0n') == '5'

# === percentages ===
assert format(x, '%') == '123456789.1%'
assert format(x, '.2%') == '123456789.10%'
assert format(x, ',.2%') == '123,456,789.10%'
assert format(Decimal('-0.0001'), '.0%') == '-0%'
assert format(Decimal('0E+5'), '.3%') == '0.000%'

# === grouping ===
assert format(x, ',') == '1,234,567.891'
assert format(x, '_') == '1_234_567.891'
assert format(x, ',.2f') == '1,234,567.89'
assert format(x, '12,.3f') == '1,234,567.891'
assert format(x, '015,.2f') == '0,001,234,567.89'
assert format(x, '.10_f') == '1234567.891_000_000_0'
assert format(x, '.10,f') == '1234567.891,000,000,0'
assert format(x, ',._f') == '1,234,567.891'
assert format(Decimal('-123'), '-012,') == '-000,000,123'
assert format(Decimal(5), '015_') == '000_000_000_005'

# === fill, alignment and sign ===
assert format(x, '>15') == '    1234567.891'
assert format(x, '<15') == '1234567.891    '
assert format(x, '^15') == '  1234567.891  '
assert format(x, '*^15') == '**1234567.891**'
assert format(x, '015') == '00001234567.891'
assert format(x, '0=15') == '00001234567.891'
assert format(Decimal('-123'), '=+12') == '-        123'
assert format(Decimal('2.5'), '+08.2f') == '+0002.50'
assert format(Decimal('5'), 'é<9') == '5éééééééé'
assert format(x, '+') == '+1234567.891'
assert format(x, ' ') == ' 1234567.891'
assert format(Decimal('-0'), '010') == '-000000000'

# === negative zero and z ===
assert format(Decimal('-0'), '') == '-0'
assert format(Decimal('-0'), 'z') == '0'
assert format(Decimal('-0.0001'), '.2f') == '-0.00'
assert format(Decimal('-0.0001'), 'z.2f') == '0.00'
assert format(Decimal('-0.0001'), 'z.1e') == '-1.0e-4'
assert format(Decimal('-0.0001'), '+.1f') == '-0.0'

# === NaN and infinity ===
assert format(Decimal('NaN'), '>6') == '   NaN'
assert format(Decimal('NaN'), '+') == '+NaN'
assert format(Decimal('NaN'), '%') == 'NaN%'
assert format(Decimal('NaN'), '010') == '       NaN'
assert format(Decimal('-Infinity'), '10') == ' -Infinity'
assert format(Decimal('-Infinity'), '+') == '-Infinity'
assert format(Decimal('-sNaN12'), 'f') == '-sNaN12'

# === rounding follows the context ===
with localcontext() as ctx:
    ctx.rounding = ROUND_DOWN
    assert format(Decimal('2.675'), '.2f') == '2.67'
    assert format(Decimal('-2.675'), '.2f') == '-2.67'
    assert format(Decimal('99.99'), '.2') == '99'
    ctx.rounding = ROUND_UP
    assert format(Decimal('2.671'), '.2f') == '2.68'
    assert format(Decimal('99.91'), '.2') == '1.0E+2'
assert format(Decimal('2.675'), '.2f') == '2.68'

# === f-strings, str.format and __format__ ===
y = Decimal('1234.5')
assert f'{y:.2f}' == '1234.50'
assert f'{y:,.2f}' == '1,234.50'
assert f'{y:>10}' == '    1234.5'
assert f'{y:.2%}' == '123450.00%'
assert f'{y!r}' == "Decimal('1234.5')"
assert f'{y!s:>8}' == '  1234.5'
assert f'{y}' == '1234.5'
spec = '10.1f'
assert f'{y:{spec}}' == '    1234.5'
assert f'{y:z.0f}' == '1234'
assert '{:,.2f}'.format(y) == '1,234.50'
assert '{0!r}'.format(y) == "Decimal('1234.5')"
assert '{:^12}'.format(y) == '   1234.5   '
assert format(y) == '1234.5'
assert y.__format__('.1f') == '1234.5'
try:
    y.__format__('.1f', Context())
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'optional argument must be a dict'

# === percent formatting converts through float and int ===
assert '%.2f' % Decimal('2.675') == '2.67'
assert '%d' % Decimal('2.7') == '2'
assert '%i' % Decimal('-2.7') == '-2'
assert '%s' % Decimal('2.7') == '2.7'
assert '%r' % Decimal('2.7') == "Decimal('2.7')"
assert '%e' % Decimal('1.5') == '1.500000e+00'
assert '%5.1f%%' % Decimal('12.345') == ' 12.3%'
assert '%g' % Decimal('0.00001') == '1e-05'
assert '%(a).1f' % {'a': Decimal('0.25')} == '0.2'
assert '%.3s' % Decimal('1.2345') == '1.2'

# === large outputs ===
assert len(format(Decimal('1e9999'), 'f')) == 10000
assert len(format(Decimal('1'), '.10000f')) == 10002
assert len(format(Decimal(5), '10000')) == 10000
assert len(format(Decimal(5), '010000,')) == 10001

# === errors ===
assert format_error(x, '.f') == 'ValueError: invalid format string'
assert format_error(x, 'x') == 'ValueError: invalid format string'
assert format_error(x, 'd') == 'ValueError: invalid format string'
assert format_error(x, 's') == 'ValueError: invalid format string'
assert format_error(x, ',_') == 'ValueError: invalid format string'
assert format_error(x, '<020') == 'ValueError: invalid format string'
try:
    f'{x:<020}'
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'invalid format string'
assert f'{Decimal(1):001}' == '1'
assert f'{Decimal(1): <5}' == '1    '
assert f'{x:<20}' == '1234567.891         '
assert format_error(x, ',n') == 'ValueError: invalid format string'
assert format_error(x, 'ff') == 'ValueError: invalid format string'
assert format_error(x, '99999999999999999999') == "OverflowError: cannot fit 'int' into an index-sized integer"
assert format_error(x, '.99999999999999999999f') == "OverflowError: cannot fit 'int' into an index-sized integer"
assert format_error(x, 1) == 'TypeError: format() argument 2 must be str, not int'
try:
    f'{x:x}'
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'invalid format string'
try:
    '%x' % Decimal(10)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '%x format: an integer is required, not decimal.Decimal'
try:
    '%c' % Decimal(65)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '%c requires an int or a unicode character, not decimal.Decimal'
try:
    x.__format__(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'format arg must be str'
