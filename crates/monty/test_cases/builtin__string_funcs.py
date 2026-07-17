# === ord() ===
# Basic ord operations
assert ord('a') == 97
assert ord('A') == 65
assert ord('0') == 48
assert ord(' ') == 32
assert ord('\n') == 10
assert ord('\t') == 9

# Unicode characters
assert ord('\u00e9') == 233
assert ord('\u4e2d') == 20013
assert ord('\U0001f600') == 128512

# === chr() ===
# Basic chr operations
assert chr(97) == 'a'
assert chr(65) == 'A'
assert chr(48) == '0'
assert chr(32) == ' '
assert chr(10) == '\n'

# Unicode characters
assert chr(233) == '\u00e9'
assert chr(20013) == '\u4e2d'
assert chr(128512) == '\U0001f600'

# Edge cases
assert chr(0) == '\x00'
assert chr(0x10FFFF) != ''

# Round-trip test
assert chr(ord('x')) == 'x'
assert ord(chr(1000)) == 1000

# === bin() ===
# Basic bin operations
assert bin(0) == '0b0'
assert bin(1) == '0b1'
assert bin(2) == '0b10'
assert bin(5) == '0b101'
assert bin(255) == '0b11111111'
assert bin(-5) == '-0b101'
assert bin(True) == '0b1'
assert bin(False) == '0b0'
MIN_I64 = -9223372036854775807 - 1  # Smallest i64
MIN_I64_BIN = '1' + '0' * 63
MIN_I64_HEX = '8' + '0' * 15
MIN_I64_OCT = '1' + '0' * 21
assert bin(MIN_I64) == '-0b' + MIN_I64_BIN

# === hex() ===
# Basic hex operations
assert hex(0) == '0x0'
assert hex(15) == '0xf'
assert hex(16) == '0x10'
assert hex(255) == '0xff'
assert hex(256) == '0x100'
assert hex(-42) == '-0x2a'
assert hex(True) == '0x1'
assert hex(False) == '0x0'
assert hex(MIN_I64) == '-0x' + MIN_I64_HEX

# === oct() ===
# Basic oct operations
assert oct(0) == '0o0'
assert oct(7) == '0o7'
assert oct(8) == '0o10'
assert oct(64) == '0o100'
assert oct(-56) == '-0o70'
assert oct(True) == '0o1'
assert oct(False) == '0o0'
assert oct(MIN_I64) == '-0o' + MIN_I64_OCT
