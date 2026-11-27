import math
import sys

# === float.hex() ===
assert (0.0).hex() == '0x0.0p+0'
assert (-0.0).hex() == '-0x0.0p+0'
assert (1.0).hex() == '0x1.0000000000000p+0'
assert (-1.0).hex() == '-0x1.0000000000000p+0'
assert (1.5).hex() == '0x1.8000000000000p+0'
assert (-1.5).hex() == '-0x1.8000000000000p+0'
assert (0.1).hex() == '0x1.999999999999ap-4'
assert (255.0).hex() == '0x1.fe00000000000p+7'
assert sys.float_info.max.hex() == '0x1.fffffffffffffp+1023'
assert sys.float_info.min.hex() == '0x1.0000000000000p-1022'
assert (5e-324).hex() == '0x0.0000000000001p-1022'
assert (-5e-324).hex() == '-0x0.0000000000001p-1022'
assert float('inf').hex() == 'inf'
assert float('-inf').hex() == '-inf'
assert float('nan').hex() == 'nan'
assert (-float('nan')).hex() == 'nan'

# === float.fromhex() basic & round-trip ===
for val in [0.0, -0.0, 1.0, -1.0, 1.5, -1.5, 0.1, 255.0, 5e-324, -5e-324, sys.float_info.max, sys.float_info.min]:
    rt = float.fromhex(val.hex())
    assert rt == val
    assert math.copysign(1.0, rt) == math.copysign(1.0, val)

# Callable on float instance as well as class
assert (1.5).fromhex('0x1.8p+0') == 1.5

# Flexible syntax forms
assert float.fromhex('1.8p0') == 1.5
assert float.fromhex('0X1.8P+0') == 1.5
assert float.fromhex('ff') == 255.0
assert float.fromhex('0xff') == 255.0
assert float.fromhex('.8') == 0.5
assert float.fromhex('1.') == 1.0
assert float.fromhex('  \t\n -0x1.0p-1 \r\x0b\x0c ') == -0.5

# Subnormals and underflow with signed zero
assert float.fromhex('0x1p-1074') == 5e-324
assert float.fromhex('0x0.0000000000001p-1022') == 5e-324
assert float.fromhex('0x1p-1075') == 0.0
neg_underflow = float.fromhex('-0x1p-2000')
assert neg_underflow == 0.0 and math.copysign(1.0, neg_underflow) == -1.0
assert float.fromhex('0x0p1000000000') == 0.0
neg_zero_exp = float.fromhex('-0x0p1000000000')
assert neg_zero_exp == 0.0 and math.copysign(1.0, neg_zero_exp) == -1.0

# Leading zeroes
assert float.fromhex('0x0.000000000000000000000000000000000000000000000000000001p0') == 9.495567745759799e-66
assert float.fromhex('0x' + '0' * 500 + '1.0p0') == 1.0

# Rounding half-to-even at subnormal & normal boundaries
assert float.fromhex('0x1.00000000000008p+0') == 1.0
assert float.fromhex('0x1.00000000000018p+0') == float.fromhex('0x1.0000000000002p+0')
assert float.fromhex('0x1.fffffffffffff7fffffffffffffffp+1023') == sys.float_info.max
assert float.fromhex('0x0.fffffffffffff8p-1022') == sys.float_info.min

# Special values
assert math.isinf(float.fromhex('inf')) and float.fromhex('inf') > 0
assert math.isinf(float.fromhex('+Infinity')) and float.fromhex('+Infinity') > 0
assert math.isinf(float.fromhex(' -INF ')) and float.fromhex(' -INF ') < 0
assert math.isnan(float.fromhex('nan'))
assert math.isnan(float.fromhex('-NaN'))

# === Error cases ===
for bad_overflow in ['0x1.fffffffffffff8p+1023', '0x1p1024', '-0x1p1024', '0x1p99999999999999999999']:
    try:
        float.fromhex(bad_overflow)
        assert False, f'expected OverflowError for {bad_overflow!r}'
    except OverflowError as e:
        assert str(e) == 'hexadecimal value too large to represent as a float'

for bad_val in [
    '',
    '   ',
    '0x',
    '0x.',
    '.',
    'p0',
    '0x1p',
    '0x1p+',
    '0x1p-',
    '0x1.2.3p0',
    '0x1_0p0',
    '0x1p0 extra',
    '\u00a00x1p0',
]:
    try:
        float.fromhex(bad_val)
        assert False, f'expected ValueError for {bad_val!r}'
    except ValueError as e:
        assert str(e) == 'invalid hexadecimal floating-point string'

try:
    float.fromhex()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.fromhex() takes exactly one argument (0 given)'

try:
    float.fromhex('0x1p0', '0x2p0')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.fromhex() takes exactly one argument (2 given)'

try:
    float.fromhex(string='0x1p0')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.fromhex() takes no keyword arguments'

for bad_type in [123, b'0x1p0', None]:
    try:
        float.fromhex(bad_type)
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == 'bad argument type for built-in operation'

try:
    (1.0).hex(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.hex() takes no arguments (1 given)'

try:
    (1.0).hex(x=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.hex() takes no keyword arguments'
