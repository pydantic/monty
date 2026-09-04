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

# === crc_hqx ===
# CRC-16/XMODEM, which the check value 0x31c3 identifies
assert binascii.crc_hqx(b'123456789', 0) == 0x31C3
assert binascii.crc_hqx(b'', 0) == 0
assert binascii.crc_hqx(b'', 1234) == 1234
assert binascii.crc_hqx(b'hello', 0) == 50018
# resumable: feeding the running value back in matches one pass
assert binascii.crc_hqx(b'lo', binascii.crc_hqx(b'hel', 0)) == binascii.crc_hqx(b'hello', 0)
assert binascii.crc_hqx(every_byte, 0) == 32341
# the seed is taken modulo 2**32 and then narrowed to 16 bits
assert binascii.crc_hqx(b'a', 0x10000) == binascii.crc_hqx(b'a', 0)
assert binascii.crc_hqx(b'a', 2**64) == binascii.crc_hqx(b'a', 0)
assert binascii.crc_hqx(b'a', -1) == binascii.crc_hqx(b'a', 0xFFFF)
assert binascii.crc_hqx(b'a', True) == binascii.crc_hqx(b'a', 1)

# === crc_hqx errors ===
try:
    binascii.crc_hqx(b'a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'crc_hqx expected 2 arguments, got 1'

try:
    binascii.crc_hqx(data=b'a', crc=0)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'binascii.crc_hqx() takes no keyword arguments'

try:
    binascii.crc_hqx('a', 0)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

try:
    binascii.crc_hqx(b'a', 1.0)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"

# === b2a_uu ===
assert binascii.b2a_uu(b'') == b' \n'
assert binascii.b2a_uu(b'a') == b'!80  \n'
assert binascii.b2a_uu(b'ab') == b'"86( \n'
assert binascii.b2a_uu(b'hello') == b'%:&5L;&\\ \n'
assert binascii.b2a_uu(b'\x00\x00\x00') == b'#    \n'
assert binascii.b2a_uu(b'x' * 45) == b"M>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX>'AX\n"
# backtick spells a zero group as '`' so a mail gateway cannot trim it away,
# and it reaches the length byte too
assert binascii.b2a_uu(b'\x00\x00\x00', backtick=True) == b'#````\n'
assert binascii.b2a_uu(b'', backtick=True) == b'`\n'
assert binascii.b2a_uu(b'a', backtick=True) == b'!80``\n'
assert binascii.b2a_uu(b'a', backtick=2) == b'!80``\n'

# === b2a_uu errors ===
try:
    binascii.b2a_uu(b'x' * 46)
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'At most 45 bytes at once'

try:
    binascii.b2a_uu('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

# data is positional-only, so passing it by keyword leaves nothing positional
try:
    binascii.b2a_uu(data=b'a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'b2a_uu() takes exactly 1 positional argument (0 given)'

try:
    binascii.b2a_uu(b'a', bogus=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "b2a_uu() got an unexpected keyword argument 'bogus'"

# === a2b_uu ===
assert binascii.a2b_uu(b' \n') == b''
assert binascii.a2b_uu(b'`') == b''
assert binascii.a2b_uu(b'%:&5L;&\\ \n') == b'hello'
assert binascii.a2b_uu('%:&5L;&\\ \n') == b'hello'
# a line shorter than its length byte claims is zero-padded, not rejected
assert binascii.a2b_uu(b'#86') == b'a`\x00'
assert binascii.a2b_uu(b'~86AA\n') == b'aha' + b'\x00' * 27
# space and backtick both decode as a zero group
assert binascii.a2b_uu(b'!``') == b'\x00'
assert binascii.a2b_uu(b'!  ') == b'\x00'
# whitespace after the promised bytes is ignored, wherever the line ends
assert binascii.a2b_uu(b'!80\r\n') == b'a'
assert binascii.a2b_uu(b'!80   ') == b'a'

# === a2b_uu errors ===
try:
    binascii.a2b_uu(b'')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Missing length byte'

try:
    binascii.a2b_uu(b'#\x01\x02\x03')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Illegal char'

try:
    binascii.a2b_uu(b'!a  ')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Illegal char'

try:
    binascii.a2b_uu(b'!86AA\n')
    assert False, 'expected binascii.Error'
except binascii.Error as exc:
    assert str(exc) == 'Trailing garbage'

try:
    binascii.a2b_uu(data=b'!80')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'binascii.a2b_uu() takes no keyword arguments'

try:
    binascii.a2b_uu(b'!80', 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'binascii.a2b_uu() takes exactly one argument (2 given)'

try:
    binascii.a2b_uu(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument should be bytes, buffer or ASCII string, not 'int'"

# === uuencode round trips ===
for _length in range(46):
    _chunk = every_byte[_length : _length * 2]
    assert binascii.a2b_uu(binascii.b2a_uu(_chunk)) == _chunk
    assert binascii.a2b_uu(binascii.b2a_uu(_chunk, backtick=True)) == _chunk

# === b2a_qp ===
assert binascii.b2a_qp(b'') == b''
assert binascii.b2a_qp(b'hello') == b'hello'
assert binascii.b2a_qp(b'a=b') == b'a=3Db'
assert binascii.b2a_qp(b'caf\xe9') == b'caf=E9'
assert binascii.b2a_qp(b'\x00') == b'=00'
# whitespace is literal in the middle of a line but quoted at the end of one,
# where it would not survive transit
assert binascii.b2a_qp(b'a\tb') == b'a\tb'
assert binascii.b2a_qp(b'  ') == b' =20'
assert binascii.b2a_qp(b'a \n') == b'a=20\n'
assert binascii.b2a_qp(b'a  \nb  \n') == b'a =20\nb =20\n'
# quotetabs quotes it everywhere
assert binascii.b2a_qp(b'a\tb', quotetabs=True) == b'a=09b'
assert binascii.b2a_qp(b'  ', quotetabs=True) == b'=20=20'
# istext=False makes newlines data rather than line structure
assert binascii.b2a_qp(b'a\nb', istext=False) == b'a=0Ab'
assert binascii.b2a_qp(b'\r\n', istext=False) == b'=0D=0A'
# header mode writes a space as '_', so '_' itself has to be quoted
assert binascii.b2a_qp(b'a b', header=True) == b'a_b'
assert binascii.b2a_qp(b'a_b', header=True) == b'a=5Fb'
assert binascii.b2a_qp(b'a_b') == b'a_b'
# a '.' alone on a line is quoted: an SMTP relay would read it as the end
assert binascii.b2a_qp(b'.') == b'=2E'
assert binascii.b2a_qp(b'\n.\n.\n') == b'\n=2E\n=2E\n'
assert binascii.b2a_qp(b'...') == b'...'
assert binascii.b2a_qp(b'a\n.b\n') == b'a\n.b\n'

# === b2a_qp soft line breaks ===
assert binascii.b2a_qp(b'x' * 75) == b'x' * 75
assert binascii.b2a_qp(b'x' * 77) == b'x' * 75 + b'=\n' + b'xx'
# no break is spent right before a newline that would end the line anyway
assert binascii.b2a_qp(b'x' * 76 + b'\n') == b'x' * 76 + b'\n'
# an escape is three columns wide and moves to the next line whole
assert binascii.b2a_qp(b'\xe9' * 30) == b'=E9' * 25 + b'=\n' + b'=E9' * 5
# the newline the input uses first decides the newline soft breaks use
assert binascii.b2a_qp(b'a\r\nb' + b'x' * 80) == b'a\r\nb' + b'x' * 74 + b'=\r\n' + b'x' * 6
assert binascii.b2a_qp(b'a\nb' + b'x' * 80) == b'a\nb' + b'x' * 74 + b'=\n' + b'x' * 6
# a lone '\r' is data, not a line ending
assert binascii.b2a_qp(b'a\rb') == b'a\rb'
assert binascii.b2a_qp(b'a\r\r\nb') == b'a\r\r\nb'

# === b2a_qp errors ===
try:
    binascii.b2a_qp('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'"

try:
    binascii.b2a_qp(b'a', 1, 1, 1, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'b2a_qp() takes at most 4 arguments (5 given)'

# === a2b_qp ===
assert binascii.a2b_qp(b'') == b''
assert binascii.a2b_qp(b'hello') == b'hello'
assert binascii.a2b_qp(b'caf=E9') == b'caf\xe9'
assert binascii.a2b_qp(b'=e9') == b'\xe9'
assert binascii.a2b_qp('=41') == b'A'
assert binascii.a2b_qp(b'a_b', header=True) == b'a b'
assert binascii.a2b_qp(b'a_b') == b'a_b'
assert binascii.a2b_qp(b'a', True) == b'a'
# soft line breaks vanish, along with anything between '=' and the newline
assert binascii.a2b_qp(b'ab=\ncd') == b'abcd'
assert binascii.a2b_qp(b'a=\r\nb') == b'ab'
assert binascii.a2b_qp(b'a=\rb') == b'a'
# nothing here is an error: a broken escape is copied through as written
assert binascii.a2b_qp(b'a=ZZ') == b'a=ZZ'
assert binascii.a2b_qp(b'=3') == b'=3'
assert binascii.a2b_qp(b'ab=') == b'ab'
assert binascii.a2b_qp(b'==') == b'='
assert binascii.a2b_qp(b'===') == b'='

try:
    binascii.a2b_qp(b'a', True, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'a2b_qp() takes at most 2 arguments (3 given)'

try:
    binascii.a2b_qp(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument should be bytes, buffer or ASCII string, not 'int'"

# === quoted-printable round trips ===
# quotetabs + istext=False leaves nothing for a transport to mangle
assert binascii.a2b_qp(binascii.b2a_qp(every_byte, quotetabs=True, istext=False)) == every_byte
for _text in (b'caf\xe9 \n', b'x' * 200, b'a\tb \nc\n', b'.\n..\n', b'\r\n\r\n'):
    assert binascii.a2b_qp(binascii.b2a_qp(_text, quotetabs=True, istext=False)) == _text

# === Incomplete ===
# nothing raises it — the a2b_hqx family it belonged to left CPython in 3.11 —
# so the class exists only to be named in an `except` clause
try:
    raise binascii.Incomplete('partial')
except binascii.Incomplete as exc:
    assert str(exc) == 'partial'

# it hangs off Exception, unlike binascii.Error which is a ValueError
try:
    raise binascii.Incomplete('caught as Exception')
except ValueError:
    assert False, 'Incomplete is not a ValueError'
except Exception as exc:
    assert str(exc) == 'caught as Exception'

try:
    raise binascii.Error('caught as ValueError')
except ValueError as exc:
    assert str(exc) == 'caught as ValueError'
