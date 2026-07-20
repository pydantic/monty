# === int() with explicit base ===
assert int('ff', 16) == 255
assert int('ff', base=16) == 255
assert int('0xff', 16) == 255
assert int('0Xff', 16) == 255
assert int('-0xff', 16) == -255
assert int('0o17', 8) == 15
assert int('0b101', 2) == 5
# 'b' is a valid hex digit, so '0b10' in base 16 is 0xb10
assert int('0b10', 16) == 2832
assert int('  ff  ', 16) == 255
assert int('5\n', 16) == 5
assert int(' 0x5 ', 16) == 5
assert int('f_f', 16) == 255
assert int('0x_ff', 16) == 255
assert int('zz', 36) == 1295
assert int('ZZ', 36) == 1295
assert int('11', 2) == 3
assert int('0', 2) == 0

# === base 0 auto-detection ===
assert int('+0xff', 0) == 255
assert int('0x_ff', 0) == 255
assert int('  -0x10  ', 0) == -16
assert int('0o17', 0) == 15
assert int('0O17', 0) == 15
assert int('0b101', 0) == 5
assert int('123', 0) == 123
assert int('0', 0) == 0
assert int('00', 0) == 0
assert int('0_0', 0) == 0
assert int('00_0', 0) == 0
assert int('-0', 0) == 0

# === bytes sources ===
assert int(b'12') == 12
assert int(b'ff', 16) == 255
assert int(b' -0x10 ', 0) == -16
assert int('\u00a012\u00a0') == 12

# Bytes accept ASCII whitespace only, not UTF-8 encodings of Unicode whitespace.
try:
    int(b'\xc2\xa012\xc2\xa0')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 10: b'\\xc2\\xa012\\xc2\\xa0'"

# === LongInt promotion with base ===
assert int('f' * 40, 16) == 1461501637330902918203684832716283019655932542975
assert int('z' * 20, 36) == 13367494538843734067838845976575

# === invalid literals report the given base ===
try:
    int('010', 0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 0: '010'"

try:
    int('0xff', 8)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 8: '0xff'"

try:
    int('', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: ''"

try:
    int('0x', 0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 0: '0x'"

try:
    int(' zz ', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: ' zz '"

try:
    int('12', 2)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 2: '12'"

try:
    int('++5', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: '++5'"

try:
    int('0x0x5', 0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 0: '0x0x5'"

try:
    int('0b2', 0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 0: '0b2'"

try:
    int(b'zz', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: b'zz'"

# === underscore placement rules ===
try:
    int('_ff', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: '_ff'"

try:
    int('ff_', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: 'ff_'"

try:
    int('f__f', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: 'f__f'"

try:
    int('0x__ff', 16)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "invalid literal for int() with base 16: '0x__ff'"

# === base validation ===
try:
    int('1', 1)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'int() base must be >= 2 and <= 36, or 0'

try:
    int('1', 37)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'int() base must be >= 2 and <= 36, or 0'

try:
    int('1', -2)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'int() base must be >= 2 and <= 36, or 0'

try:
    int('1', True)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'int() base must be >= 2 and <= 36, or 0'

# an int wider than i64 clamps (PyNumber_AsSsize_t) and lands out of range
try:
    int('1', 10**20)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'int() base must be >= 2 and <= 36, or 0'

try:
    int('1', 16.0)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'float' object cannot be interpreted as an integer"

try:
    int('1', '16')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer"

try:
    int('1', None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'NoneType' object cannot be interpreted as an integer"

# === non-string with explicit base ===
try:
    int(5, 16)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "int() can't convert non-string with explicit base"

try:
    int(5.5, 16)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "int() can't convert non-string with explicit base"

try:
    int([], 16)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "int() can't convert non-string with explicit base"

# base range is validated before the non-string check
try:
    int(5, 1)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'int() base must be >= 2 and <= 36, or 0'

# === argument binding quirks (long_vectorcall + clinic fallback) ===
try:
    int(x='5')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "int() got an unexpected keyword argument 'x'"

try:
    int(base=16)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int() missing string argument'

# the missing string argument beats base validation
try:
    int(base='q')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int() missing string argument'

try:
    int('5', bogus=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "int() got an unexpected keyword argument 'bogus'"

# no kwargs: the vectorcall fast path's un-parenthesised wording
try:
    int('5', 10, 3)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int expected at most 2 arguments, got 3'

# with kwargs: the clinic parser's parenthesised total-count wording
try:
    int('5', 10, base=16)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int() takes at most 2 arguments (3 given)'

# === digit limit applies only to non-power-of-two bases ===
assert int('1' * 5000, 16) > 0
assert int('1' * 5000, 2) > 0

try:
    int('1' * 5000, 10)
    assert False, 'expected ValueError'
except ValueError as e:
    assert (
        str(e)
        == 'Exceeds the limit (4300 digits) for integer string conversion: value has 5000 digits; use sys.set_int_max_str_digits() to increase the limit'
    )

try:
    int('1' * 5000, 12)
    assert False, 'expected ValueError'
except ValueError as e:
    assert (
        str(e)
        == 'Exceeds the limit (4300 digits) for integer string conversion: value has 5000 digits; use sys.set_int_max_str_digits() to increase the limit'
    )

# base 0 resolves to 10 before the limit is applied
try:
    int('1' * 5000, 0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert (
        str(e)
        == 'Exceeds the limit (4300 digits) for integer string conversion: value has 5000 digits; use sys.set_int_max_str_digits() to increase the limit'
    )
