# === int.to_bytes() defaults and endianness ===
assert (0).to_bytes() == b'\x00'
assert (0).to_bytes(0) == b''
assert (0).to_bytes(0, signed=True) == b''
assert (1).to_bytes() == b'\x01'
assert (258).to_bytes(2) == b'\x01\x02'
assert (258).to_bytes(2, 'big') == b'\x01\x02'
assert (258).to_bytes(2, 'little') == b'\x02\x01'
assert (258).to_bytes(length=2, byteorder='little') == b'\x02\x01'
assert (258).to_bytes(byteorder='big', length=3) == b'\x00\x01\x02'
assert (0).to_bytes(byteorder='little') == b'\x00'

# === Signed two's complement ===
assert (-1).to_bytes(1, 'big', signed=True) == b'\xff'
assert (-1).to_bytes(4, 'little', signed=True) == b'\xff\xff\xff\xff'
assert (-128).to_bytes(1, 'big', signed=True) == b'\x80'
assert (127).to_bytes(1, 'big', signed=True) == b'\x7f'
assert (-1024).to_bytes(4, 'big', signed=True) == b'\xff\xff\xfc\x00'
assert (-1024).to_bytes(4, 'little', signed=True) == b'\x00\xfc\xff\xff'
assert (-128).to_bytes(4, 'big', signed=True) == b'\xff\xff\xff\x80'
assert (-128).to_bytes(4, 'little', signed=True) == b'\x80\xff\xff\xff'

# === Boundary i64 and LongInt values ===
for val in [
    0,
    1,
    -1,
    127,
    -128,
    255,
    256,
    -256,
    (1 << 63) - 1,
    -(1 << 63),
    (1 << 64) - 1,
    1 << 64,
    -(1 << 64),
    -(1 << 127),
    (1 << 127) - 1,
    (1 << 100) + 123456789,
    -((1 << 100) + 123456789),
]:
    for order in ('big', 'little'):
        b_signed = val.to_bytes(20, order, signed=True)
        assert int.from_bytes(b_signed, order, signed=True) == val
        if val >= 0:
            b_unsigned = val.to_bytes(20, order, signed=False)
            assert int.from_bytes(b_unsigned, order, signed=False) == val

# === int.from_bytes() defaults and representations ===
assert int.from_bytes(b'') == 0
assert int.from_bytes(b'', signed=True) == 0
assert int.from_bytes(b'\x01\x02') == 258
assert int.from_bytes(b'\x01\x02', 'little') == 513
assert int.from_bytes(bytes=b'\x01\x02', byteorder='little', signed=False) == 513
assert int.from_bytes(b'\xff\xff', 'big', signed=True) == -1
assert int.from_bytes(b'\x80', 'big', signed=True) == -128
assert int.from_bytes(b'\x7f', 'big', signed=True) == 127
assert int.from_bytes(b'\x80\x00\x00\x00\x00\x00\x00\x00', 'big', signed=True) == -(1 << 63)
assert int.from_bytes(b'\x00\x00\x00\x00\x00\x00\x00\x80', 'little', signed=True) == -(1 << 63)
assert int.from_bytes(b'\x80\x00\x00\x00\x00\x00\x00\x00', 'big', signed=False) == (1 << 63)

# === int.from_bytes() with iterables (PR #1341 sharing) ===
assert int.from_bytes([1, 2]) == 258
assert int.from_bytes((1, 2)) == 258
assert int.from_bytes(range(2)) == 1
assert int.from_bytes([x for x in [1, 2]]) == 258
assert int.from_bytes((x for x in [1, 2])) == 258
assert int.from_bytes([0xFF, 0xFF], 'big', signed=True) == -1
assert int.from_bytes([0x80], 'big', signed=True) == -128

# === bool.to_bytes() and bool.from_bytes() ===
assert True.to_bytes() == b'\x01'
assert False.to_bytes() == b'\x00'
assert True.to_bytes(2, 'little') == b'\x01\x00'
assert bool.from_bytes(b'') is False
assert bool.from_bytes(b'\x00\x00') is False
assert bool.from_bytes(b'\x00\x01') is True
assert bool.from_bytes(b'\xff', signed=True) is True
assert True.from_bytes(b'\x00') is False
assert (0).from_bytes(b'\x01\x02') == 258

# === Error cases for to_bytes() ===
try:
    (-1).to_bytes(1)
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == "can't convert negative int to unsigned"

for bad_val, length, signed in [
    (256, 1, False),
    (128, 1, True),
    (-129, 1, True),
    (1, 0, False),
    (-1, 0, True),
    (1 << 80, 10, False),
    (-(1 << 80) - 1, 10, True),
]:
    try:
        bad_val.to_bytes(length, 'big', signed=signed)
        assert False, f'expected OverflowError for {bad_val}, {length}, {signed}'
    except OverflowError as e:
        assert str(e) == 'int too big to convert'

try:
    (1).to_bytes(-1)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'length argument must be non-negative'

try:
    (1).to_bytes(-1, 123)
    assert False, 'expected TypeError before ValueError'
except TypeError as e:
    assert str(e) == "to_bytes() argument 'byteorder' must be str, not int"

try:
    (-1).to_bytes(1, 'middle')
    assert False, 'expected ValueError before OverflowError'
except ValueError as e:
    assert str(e) == "byteorder must be either 'little' or 'big'"

try:
    (-1).to_bytes(-1, 'middle')
    assert False, 'expected ValueError before negative length check'
except ValueError as e:
    assert str(e) == "byteorder must be either 'little' or 'big'"

try:
    (1).to_bytes(1, 'big', True)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'to_bytes() takes at most 2 positional arguments (3 given)'

try:
    (1).to_bytes(foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "to_bytes() got an unexpected keyword argument 'foo'"

try:
    (1).to_bytes('foo')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer"

# === Error cases for from_bytes() ===
try:
    int.from_bytes()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "from_bytes() missing required argument 'bytes' (pos 1)"

try:
    int.from_bytes('abc')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "cannot convert 'str' object to bytes"

try:
    int.from_bytes(123)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "cannot convert 'int' object to bytes"

try:
    int.from_bytes(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "cannot convert 'NoneType' object to bytes"

try:
    int.from_bytes(123, 'middle')
    assert False, 'expected ValueError before TypeError'
except ValueError as e:
    assert str(e) == "byteorder must be either 'little' or 'big'"

try:
    int.from_bytes(b'\x01', 'big', True)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'from_bytes() takes at most 2 positional arguments (3 given)'

try:
    int.from_bytes(b'', foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "from_bytes() got an unexpected keyword argument 'foo'"

try:
    int.from_bytes([-1])
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'bytes must be in range(0, 256)'

try:
    int.from_bytes([256])
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'bytes must be in range(0, 256)'

try:
    int.from_bytes(['a'])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer"
