import base64
import binascii

# === b64encode / b64decode ===
assert base64.b64encode(b'') == b''
assert base64.b64encode(b'a') == b'YQ=='
assert base64.b64encode(b'ab') == b'YWI='
assert base64.b64encode(b'abc') == b'YWJj'
assert base64.b64encode(b'\x00\xff\xfe') == b'AP/+'
assert base64.b64decode(b'YWJj') == b'abc'
assert base64.b64decode(b'YQ==') == b'a'
assert base64.b64decode(b'') == b''
assert base64.b64decode('YWJj') == b'abc'
assert base64.b64decode(s=b'YWJj') == b'abc'

# === b64 round trip over every byte value ===
every_byte = b''
_hex_digits = '0123456789abcdef'
for _i in range(256):
    every_byte += bytes.fromhex(_hex_digits[_i // 16] + _hex_digits[_i % 16])
assert base64.b64decode(base64.b64encode(every_byte)) == every_byte

# === altchars ===
assert base64.b64encode(b'\xfb\xef', altchars=b'-_') == b'--8='
assert base64.b64decode(b'--8=', altchars=b'-_') == b'\xfb\xef'
assert base64.b64decode('--8=', altchars='-_') == b'\xfb\xef'
assert base64.b64encode(b'\xfb\xef', b'-_') == b'--8='

# === standard_b64 / urlsafe_b64 ===
assert base64.standard_b64encode(b'\xfb\xef') == b'++8='
assert base64.standard_b64decode(b'++8=') == b'\xfb\xef'
assert base64.urlsafe_b64encode(b'\xfb\xef') == b'--8='
assert base64.urlsafe_b64decode(b'--8=') == b'\xfb\xef'
assert base64.urlsafe_b64decode('--8=') == b'\xfb\xef'
# urlsafe decoding leaves the standard alphabet working too
assert base64.urlsafe_b64decode(b'++8=') == b'\xfb\xef'

# === non-strict decoding discards junk ===
assert base64.b64decode(b'YWJj\n') == b'abc'
assert base64.b64decode('abcd!!') == b'i\xb7\x1d'
assert base64.b64decode(b'Y W J j') == b'abc'
assert base64.b64decode(b'YWJj=') == b'abc'
assert base64.b64decode(b'YWJjZA===') == b'abcd'

# === padding permits a quad to end, it does not stop the scan ===
# junk after the padding is still discarded
assert base64.b64decode(b'YWJjZA==!!') == b'abcd'
# but alphabet characters resume the same quad, so concatenated base64 runs together
assert base64.b64decode(b'YQ==YQ==') == b'a\x06\x10'
assert base64.b64decode(b'YWJjZA==xx') == b'abcd\x0cq'
try:
    base64.b64decode(b'YWJjZA==x')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Incorrect padding'

# a long padding run leaves the closed quad alone rather than accumulating
assert base64.b64decode(b'AA' + b'=' * 253) == b'\x00'
assert base64.b64decode(b'AA' + b'=' * 254) == b'\x00'
assert base64.b64decode(b'AA' + b'=' * 600) == b'\x00'

# === validate=True rejects what the default accepts ===
assert base64.b64decode(b'YWJj', validate=True) == b'abc'
try:
    base64.b64decode(b'abcd!!', validate=True)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Only base64 data is allowed'

try:
    base64.b64decode(b'=YWJj', validate=True)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Leading padding not allowed'

try:
    base64.b64decode(b'YWJj==', validate=True)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Excess padding not allowed'

try:
    base64.b64decode(b'YW=Jj', validate=True)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Discontinuous padding not allowed'

try:
    base64.b64decode(b'YWJjZA==x', validate=True)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Excess data after padding'

# padding one character into a quad reports the length error, not a padding one
try:
    base64.b64decode(b'YWJjZ=', validate=True)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert (
        str(exc) == 'Invalid base64-encoded string: number of data characters (5) cannot be 1 more than a multiple of 4'
    )

# === padding and length errors (both modes) ===
try:
    base64.b64decode(b'a')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert (
        str(exc) == 'Invalid base64-encoded string: number of data characters (1) cannot be 1 more than a multiple of 4'
    )

try:
    base64.b64decode(b'YWJjY')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert (
        str(exc) == 'Invalid base64-encoded string: number of data characters (5) cannot be 1 more than a multiple of 4'
    )

try:
    base64.b64decode(b'YWJ')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Incorrect padding'

# === binascii.Error is a ValueError subclass ===
try:
    base64.b64decode(b'YWJ')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Incorrect padding'

# === input type errors ===
try:
    base64.b64encode('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

try:
    base64.b64decode(123)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument should be a bytes-like object or ASCII string, not 'int'"

try:
    base64.b64decode('café')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'string argument should contain only ASCII characters'

try:
    base64.b64encode([1, 2, 3])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'list'"

try:
    base64.b64encode(b'abc', altchars='-_')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

# altchars is length-checked before its type, so a non-sized value fails in `len()`
try:
    base64.b64encode(b'abc', altchars=5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'int' has no len()"

# altchars of the wrong length trips base64.py's own assert
try:
    base64.b64encode(b'abc', altchars=b'-')
    assert False, 'expected AssertionError'
except AssertionError as exc:
    assert str(exc) == "b'-'"

try:
    base64.b64decode(b'YWJj', altchars=b'-')
    assert False, 'expected AssertionError'
except AssertionError as exc:
    assert str(exc) == "b'-'"

# === module constants ===
assert base64.MAXBINSIZE == 57
assert base64.MAXLINESIZE == 76

# === b32encode / b32decode ===
assert base64.b32encode(b'') == b''
assert base64.b32encode(b'a') == b'ME======'
assert base64.b32encode(b'ab') == b'MFRA===='
assert base64.b32encode(b'abc') == b'MFRGG==='
assert base64.b32encode(b'abcd') == b'MFRGGZA='
assert base64.b32encode(b'abcde') == b'MFRGGZDF'
assert base64.b32encode(b'abcdef') == b'MFRGGZDFMY======'
assert base64.b32decode(b'MFRGGZDF') == b'abcde'
assert base64.b32decode(b'ME======') == b'a'
assert base64.b32decode('MFRGG===') == b'abc'
assert base64.b32decode(base64.b32encode(every_byte)) == every_byte

# === b32 casefold and map01 ===
assert base64.b32decode(b'mfrggzdf', casefold=True) == b'abcde'
assert base64.b32decode(b'MFRGG0DF', map01=b'L') == base64.b32decode(b'MFRGGODF')
assert base64.b32decode(b'MFRGG1DF', map01=b'L') == base64.b32decode(b'MFRGGLDF')

try:
    base64.b32decode(b'mfrggzdf')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Non-base32 digit found'

try:
    base64.b32decode(b'MFRGG!!!')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Non-base32 digit found'

try:
    base64.b32decode(b'MFRGGZD')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Incorrect padding'

try:
    base64.b32decode(b'MFRGGZ==')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Incorrect padding'

try:
    base64.b32decode(b'MFRGG', map01=b'L')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Incorrect padding'

try:
    base64.b32decode(b'MFRGGZDF', map01=b'LL')
    assert False, 'expected AssertionError'
except AssertionError as exc:
    assert str(exc) == "b'LL'"

# === b32hex ===
assert base64.b32hexencode(b'abc') == b'C5H66==='
assert base64.b32hexdecode(b'C5H66===') == b'abc'
assert base64.b32hexdecode(b'c5h66===', casefold=True) == b'abc'
assert base64.b32hexdecode(base64.b32hexencode(every_byte)) == every_byte

try:
    base64.b32hexdecode(b'c5h66===')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Non-base32 digit found'

try:
    base64.b32hexdecode(b'C5H66')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Incorrect padding'

# the base32 encoders coerce through `memoryview`, so their TypeError is prefixed
try:
    base64.b32encode('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "memoryview: a bytes-like object is required, not 'str'"

try:
    base64.b32hexencode(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "memoryview: a bytes-like object is required, not 'int'"

# b16encode reaches binascii directly, so it keeps the unprefixed wording
try:
    base64.b16encode('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

# === b16encode / b16decode ===
assert base64.b16encode(b'') == b''
assert base64.b16encode(b'abc') == b'616263'
assert base64.b16encode(b'\xde\xad') == b'DEAD'
assert base64.b16decode(b'DEAD') == b'\xde\xad'
assert base64.b16decode('616263') == b'abc'
assert base64.b16decode(b'dead', casefold=True) == b'\xde\xad'
assert base64.b16decode(base64.b16encode(every_byte)) == every_byte

try:
    base64.b16decode(b'dead')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Non-base16 digit found'

try:
    base64.b16decode(b'ABC')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Odd-length string'

# === b85encode / b85decode ===
assert base64.b85encode(b'') == b''
assert base64.b85encode(b'a') == b'VE'
assert base64.b85encode(b'abcd') == b'VPa!s'
assert base64.b85encode(b'abcde') == b'VPa!sWd'
assert base64.b85encode(b'\xff\xff\xff\xff') == b'|NsC0'
# `pad` keeps the characters the zero-padding produced, so the length is a
# multiple of five
assert base64.b85encode(b'abc') == b'VPaz'
assert base64.b85encode(b'abc', pad=True) == b'VPazd'
assert base64.b85decode(b'VPa!s') == b'abcd'
assert base64.b85decode('VPaz') == b'abc'
assert base64.b85decode(b'') == b''
assert base64.b85decode(base64.b85encode(every_byte)) == every_byte
# a trailing partial group decodes to however many bytes it carries, and a
# single leftover character carries none
assert base64.b85decode(b'VPa') == b'ab'
assert base64.b85decode(b'v') == b''

# base85 failures are plain ValueErrors, not binascii.Error
try:
    base64.b85decode(b'|-.CM')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'bad base85 character at position 2'

try:
    base64.b85decode(b'|NsC1')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'base85 overflow in hunk starting at byte 0'

try:
    base64.b85encode('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "memoryview: a bytes-like object is required, not 'str'"

try:
    base64.b85decode(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument should be a bytes-like object or ASCII string, not 'int'"

# === z85encode / z85decode ===
assert base64.z85encode(b'abcd') == b'vpA.S'
assert base64.z85encode(b'abc') == b'vpAZ'
assert base64.z85decode(b'vpA.S') == b'abcd'
assert base64.z85decode('vpA.S') == b'abcd'
assert base64.z85decode(base64.z85encode(every_byte)) == every_byte

# the five base85 characters z85 does not share are rejected, not decoded
try:
    base64.z85decode(b'vpA.~')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'bad z85 character at position 4'

try:
    base64.z85decode(b'#####')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'z85 overflow in hunk starting at byte 0'

# === encodebytes / decodebytes ===
assert base64.encodebytes(b'') == b''
assert base64.encodebytes(b'abc') == b'YWJj\n'
# 57 input bytes fill exactly one 76-character line
assert base64.encodebytes(b'x' * 57) == base64.b64encode(b'x' * 57) + b'\n'
assert base64.encodebytes(b'x' * 58).count(b'\n') == 2
assert base64.decodebytes(base64.encodebytes(b'x' * 200)) == b'x' * 200
assert base64.decodebytes(b'YWJj\n') == b'abc'

try:
    base64.encodebytes('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'expected bytes-like object, not str'

# decodebytes takes bytes only — unlike b64decode it rejects str
try:
    base64.decodebytes('YWJj')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'expected bytes-like object, not str'
