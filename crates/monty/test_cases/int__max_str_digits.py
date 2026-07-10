# === int-to-str: within limit ===
assert str(10**4299) is not None
assert len(str(10**4299)) == 4300
assert str(0) == '0'
assert str(-1) == '-1'
assert str(10**18) == '1000000000000000000'
assert repr(10**18) == '1000000000000000000'

# === int-to-str: exceeds limit ===
try:
    str(10**4300)
    assert False, 'str(10**4300) should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

try:
    repr(10**4300)
    assert False, 'repr(10**4300) should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === int-to-str: negative big int ===
try:
    str(-(10**4300))
    assert False, 'str(negative huge) should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === int-to-str: print() ===
try:
    print(10**4300)
    assert False, 'print(10**4300) should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === int-to-str: f-strings ===
x = 10**4300
try:
    f'{x}'
    assert False, 'f-string with huge int should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === str-to-int: within limit ===
assert int('1' * 4300) is not None

# === str-to-int: exceeds limit ===
try:
    int('1' * 4301)
    assert False, 'int() with 4301 digits should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === str-to-int: sign does not count ===
try:
    int('-' + '1' * 4301)
    assert False, 'int() with negative 4301 digits should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === non-decimal conversions are NOT limited ===
big = 2**20000
assert bin(big) is not None
assert hex(big) is not None
assert oct(big) is not None

# === KeyError with huge int key ===
# CPython raises KeyError (stores the key object). Monty falls back to the
# type name when the key is too large to stringify, but still raises KeyError.
d = {}
try:
    d[10**5000]
    assert False, 'should raise KeyError'
except KeyError:
    pass

# === f-string with !s conversion ===
y = 10**4300
try:
    f'{y!s}'
    assert False, 'f-string !s should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === f-string with !r conversion ===
try:
    f'{y!r}'
    assert False, 'f-string !r should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === str() on container with huge int ===
try:
    str([10**5000])
    assert False, 'str([huge]) should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === print() container with huge int ===
try:
    print([10**5000])
    assert False, 'print([huge]) should raise ValueError'
except ValueError as e:
    assert str(e).startswith('Exceeds the limit (4300 digits) for integer string conversion'), f'wrong message: {e}'

# === int() with invalid large string gives "invalid literal", not digit-limit error ===
try:
    int('1' * 4301 + 'x')
    assert False, 'int() with invalid large string should raise ValueError'
except ValueError as e:
    assert str(e).startswith('invalid literal for int() with base 10'), f'wrong error: {e}'
