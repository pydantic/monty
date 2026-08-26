import binascii

# === hexlify / b2a_hex ===
assert binascii.hexlify(b'') == b''
assert binascii.hexlify(b'\xde\xad') == b'dead'
assert binascii.hexlify(b'abc') == b'616263'
assert binascii.b2a_hex(b'\xde') == b'de'
assert binascii.hexlify(data=b'ab') == b'6162'

# === hexlify separators ===
# a positive bytes_per_sep groups from the right, so a short group leads
assert binascii.hexlify(b'abcd', b'-') == b'61-62-63-64'
assert binascii.hexlify(b'abcd', '-') == b'61-62-63-64'
assert binascii.hexlify(b'abcde', b'-', 2) == b'61-6263-6465'
assert binascii.hexlify(b'abcde', b'-', -2) == b'6162-6364-65'
assert binascii.hexlify(b'abcde', b'-', 3) == b'6162-636465'
assert binascii.hexlify(b'abcde', b'-', -3) == b'616263-6465'
# a group wider than the input, or a zero width, means no separator at all
assert binascii.hexlify(b'abcde', b'-', 99) == b'6162636465'
assert binascii.hexlify(b'abcd', b'-', 0) == b'61626364'
assert binascii.hexlify(b'', b'-') == b''
assert binascii.hexlify(b'ab', sep=b'-') == b'61-62'
assert binascii.hexlify(b'ab', b'-', bytes_per_sep=2) == b'6162'
# the separator is a byte, not a character, so it need not be ASCII
assert binascii.hexlify(b'ab', b'\xff') == b'61\xff62'
assert binascii.hexlify(b'ab', '\xff') == b'61\xff62'

# === hexlify errors ===
try:
    binascii.hexlify(b'abcd', b'--')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'sep must be length 1.'

try:
    binascii.hexlify(b'ab', b'')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'sep must be length 1.'

try:
    binascii.hexlify(b'ab', '€')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'sep must be ASCII.'

try:
    binascii.hexlify(b'ab', 5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'int' has no len()"

# a sized object of length one is still only accepted as `str` or `bytes`,
# and the length is measured first, so a longer one is a ValueError instead
try:
    binascii.hexlify(b'ab', [1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'sep must be str or bytes.'

try:
    binascii.hexlify(b'ab', (1,))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'sep must be str or bytes.'

try:
    binascii.hexlify(b'ab', [1, 2])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'sep must be length 1.'

# `sep` defaults to unset, not None, so an explicit None has no length
try:
    binascii.hexlify(b'ab', None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'NoneType' has no len()"

try:
    binascii.hexlify('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

try:
    binascii.hexlify(b'a', b'-', 1.5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"

# === unhexlify / a2b_hex ===
assert binascii.unhexlify(b'') == b''
assert binascii.unhexlify(b'dead') == b'\xde\xad'
# unlike b16decode, either case is accepted without a flag
assert binascii.unhexlify(b'DeAd') == b'\xde\xad'
assert binascii.unhexlify('dead') == b'\xde\xad'
assert binascii.a2b_hex(b'dead') == b'\xde\xad'

try:
    binascii.unhexlify(b'abc')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Odd-length string'

try:
    binascii.unhexlify(b'zz')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Non-hexadecimal digit found'

try:
    binascii.unhexlify('caf\xe9')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'string argument should contain only ASCII characters'

try:
    binascii.unhexlify(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument should be bytes, buffer or ASCII string, not 'int'"

# === b2a_base64 / a2b_base64 ===
assert binascii.b2a_base64(b'abc') == b'YWJj\n'
# the newline is unconditional, so empty input still produces one
assert binascii.b2a_base64(b'') == b'\n'
assert binascii.b2a_base64(b'abc', newline=False) == b'YWJj'
assert binascii.a2b_base64(b'YWJj') == b'abc'
assert binascii.a2b_base64('YWJj') == b'abc'
assert binascii.a2b_base64(b'') == b''
# the same forgiving scan base64.b64decode gets
assert binascii.a2b_base64(b'YQ==YQ==') == b'a\x06\x10'

try:
    binascii.a2b_base64(b'YWJ')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Incorrect padding'

try:
    binascii.a2b_base64(b'ab!cd', strict_mode=True)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Only base64 data is allowed'

try:
    binascii.a2b_base64(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument should be bytes, buffer or ASCII string, not 'int'"

try:
    binascii.b2a_base64('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

# === crc32 ===
assert binascii.crc32(b'') == 0
assert binascii.crc32(b'hello') == 907060870
# feeding the previous result back in resumes the checksum
assert binascii.crc32(b'lo', binascii.crc32(b'hel')) == binascii.crc32(b'hello')
# the seed is taken modulo 2**32, so negative and oversized values are fine
assert binascii.crc32(b'x', -1) == binascii.crc32(b'x', 4294967295)
assert binascii.crc32(b'x', 4294967296) == binascii.crc32(b'x', 0)
assert binascii.crc32(b'x', 1180591620717411303424) == binascii.crc32(b'x', 0)
assert binascii.crc32(b'x', True) == binascii.crc32(b'x', 1)

try:
    binascii.crc32('x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

try:
    binascii.crc32(b'x', 'y')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

# `crc` defaults to unset, so an explicit None is converted and rejected
try:
    binascii.crc32(b'x', None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'NoneType' object cannot be interpreted as an integer"

# === round trips over every byte value ===
_hex_digits = '0123456789abcdef'
every_byte = b''
for _i in range(256):
    every_byte += bytes.fromhex(_hex_digits[_i // 16] + _hex_digits[_i % 16])

assert binascii.unhexlify(binascii.hexlify(every_byte)) == every_byte
assert binascii.a2b_base64(binascii.b2a_base64(every_byte)) == every_byte
assert binascii.crc32(every_byte) == 688229491
