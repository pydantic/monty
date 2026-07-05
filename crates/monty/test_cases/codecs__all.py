# === Encoding name normalization and aliases ===
# Names are normalized like CPython: case-insensitive, runs of spaces/hyphens
# collapse to '_', leading/trailing punctuation dropped, dots kept.
for alias in ('utf-8', 'utf8', 'utf_8', 'UTF-8', 'utf', 'u8', 'cp65001', 'utf8-ucs2', 'utf8-ucs4', ' UTF 8 ', 'UTF--8'):
    assert 'héllo'.encode(alias) == b'h\xc3\xa9llo', f'utf-8 alias {alias}'
    assert b'h\xc3\xa9llo'.decode(alias) == 'héllo', f'utf-8 decode alias {alias}'
for alias in (
    'ascii',
    'ASCII',
    '646',
    'us',
    'us-ascii',
    'us_ascii',
    'cp367',
    'ibm367',
    'csascii',
    'ansi_x3.4-1968',
    'ansi_x3.4-1986',
    'iso646-us',
    'ISO_646.IRV:1991',
    'iso-ir-6',
):
    assert 'hello'.encode(alias) == b'hello', f'ascii alias {alias}'
    assert b'hello'.decode(alias) == 'hello', f'ascii decode alias {alias}'
for alias in ('utf-16', 'utf16', 'utf_16', 'u16', 'UTF 16'):
    assert 'a'.encode(alias) == b'\xff\xfea\x00', f'utf-16 alias {alias}'
for alias in ('utf-16-le', 'utf-16le', 'UTF_16_LE', 'UnicodeLittleUnmarked'):
    assert 'a'.encode(alias) == b'a\x00', f'utf-16-le alias {alias}'
for alias in ('utf-16-be', 'utf-16be', 'UnicodeBigUnmarked'):
    assert 'a'.encode(alias) == b'\x00a', f'utf-16-be alias {alias}'
for alias in ('utf-32', 'utf32', 'u32'):
    assert 'a'.encode(alias) == b'\xff\xfe\x00\x00a\x00\x00\x00', f'utf-32 alias {alias}'
assert 'a'.encode('utf-32-le') == b'a\x00\x00\x00', 'utf-32-le'
assert 'a'.encode('utf-32le') == b'a\x00\x00\x00', 'utf-32le alias'
assert 'a'.encode('utf-32-be') == b'\x00\x00\x00a', 'utf-32-be'
assert 'a'.encode('utf-32be') == b'\x00\x00\x00a', 'utf-32be alias'

# Dots are preserved by normalization, so 'utf.8' is NOT an alias of utf-8.
try:
    'hi'.encode('utf.8')
    assert False, 'utf.8 should not resolve'
except LookupError as e:
    assert str(e) == 'unknown encoding: utf.8', f'unknown encoding message: {e}'
# The LookupError shows the original name, not the normalized form.
try:
    b'hi'.decode(' Not A Real--Codec ')
    assert False, 'unknown codec should error'
except LookupError as e:
    assert str(e) == 'unknown encoding:  Not A Real--Codec ', f'unknown encoding keeps original name: {e}'

# === utf-16 / utf-32 encode ===
# The bare variants prepend a BOM (little-endian), even for an empty string.
assert 'ab'.encode('utf-16') == b'\xff\xfea\x00b\x00', 'utf-16 encode with BOM'
assert ''.encode('utf-16') == b'\xff\xfe', 'utf-16 encode empty is BOM only'
assert ''.encode('utf-16-le') == b'', 'utf-16-le encode empty'
assert ''.encode('utf-32') == b'\xff\xfe\x00\x00', 'utf-32 encode empty is BOM only'
# Explicit -le/-be variants write no BOM.
assert 'ab'.encode('utf-16-le') == b'a\x00b\x00', 'utf-16-le encode'
assert 'ab'.encode('utf-16-be') == b'\x00a\x00b', 'utf-16-be encode'
# Astral characters become surrogate pairs in utf-16, single units in utf-32.
assert '\U0001f600'.encode('utf-16') == b'\xff\xfe=\xd8\x00\xde', 'utf-16 surrogate pair'
assert '\U0001f600'.encode('utf-16-be') == b'\xd8=\xde\x00', 'utf-16-be surrogate pair'
assert '\U0001f600'.encode('utf-32-le') == b'\x00\xf6\x01\x00', 'utf-32-le astral char'
assert '\U0001f600'.encode('utf-32-be') == b'\x00\x01\xf6\x00', 'utf-32-be astral char'
# Encoding can never fail (no lone surrogates exist), so `errors` is never
# consulted or validated, matching CPython's lazy handler lookup.
assert 'héllo ⚡'.encode('utf-16', 'bogus') == 'héllo ⚡'.encode('utf-16'), 'utf-16 errors never validated'
assert 'héllo'.encode('utf-32-be', 'strict') == 'héllo'.encode('utf-32-be'), 'utf-32 errors accepted'

# Round-trips through both endiannesses and the BOM variant.
s = 'héllo wörld ⚡ 日本語 \U0001f600 test'
for enc in ('utf-16', 'utf-16-le', 'utf-16-be', 'utf-32', 'utf-32-le', 'utf-32-be'):
    assert s.encode(enc).decode(enc) == s, f'{enc} round-trip'

# === utf-16 / utf-32 decode: BOM handling ===
# The bare variants consume a BOM of either endianness.
assert b'\xff\xfea\x00'.decode('utf-16') == 'a', 'utf-16 decode LE BOM'
assert b'\xfe\xff\x00a'.decode('utf-16') == 'a', 'utf-16 decode BE BOM'
assert b'\xff\xfe'.decode('utf-16') == '', 'utf-16 decode BOM only'
assert b''.decode('utf-16') == '', 'utf-16 decode empty'
assert b'\xff\xfe\x00\x00a\x00\x00\x00'.decode('utf-32') == 'a', 'utf-32 decode LE BOM'
assert b'\x00\x00\xfe\xff\x00\x00\x00a'.decode('utf-32') == 'a', 'utf-32 decode BE BOM'
assert b''.decode('utf-32') == '', 'utf-32 decode empty'
# A second BOM is real content (U+FEFF, zero width no-break space).
assert b'\xff\xfe\xff\xfe'.decode('utf-16') == '﻿', 'utf-16 double BOM keeps second'
# Explicit -le/-be variants never consume a BOM.
assert b'\xff\xfea\x00'.decode('utf-16-le') == '﻿a', 'utf-16-le keeps BOM as U+FEFF'
assert b'\x00\x00\xfe\xff'.decode('utf-32-be') == '﻿', 'utf-32-be keeps BOM as U+FEFF'

# === utf-16 decode errors ===
# Errors are reported under the resolved codec name ('utf-16-le', not
# 'utf-16'), with byte positions; a consumed BOM still counts in positions.
try:
    b'a\x00b'.decode('utf-16-le')
    assert False, 'odd-length utf-16 should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-16-le' codec can't decode byte 0x62 in position 2: truncated data", (
        f'utf-16 truncated single byte: {e}'
    )
try:
    b'\x00\xd8a\x00'.decode('utf-16-le')
    assert False, 'lone high surrogate should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-16-le' codec can't decode bytes in position 0-1: illegal UTF-16 surrogate", (
        f'utf-16 high surrogate without pair: {e}'
    )
try:
    b'\x00\xdca\x00'.decode('utf-16-le')
    assert False, 'lone low surrogate should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-16-le' codec can't decode bytes in position 0-1: illegal encoding", (
        f'utf-16 lone low surrogate: {e}'
    )
try:
    b'a\x00\x00\xd8'.decode('utf-16-le')
    assert False, 'high surrogate at end should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-16-le' codec can't decode bytes in position 2-3: unexpected end of data", (
        f'utf-16 high surrogate at end: {e}'
    )
# A high surrogate followed by a single trailing byte is one 3-byte error unit.
try:
    b'a\x00\x00\xd8a'.decode('utf-16-le')
    assert False, 'high surrogate + stray byte should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-16-le' codec can't decode bytes in position 2-4: unexpected end of data", (
        f'utf-16 3-byte tail is one unit: {e}'
    )
# Positions include the consumed BOM.
try:
    b'\xff\xfe\x00\xd8'.decode('utf-16')
    assert False, 'high surrogate after BOM should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-16-le' codec can't decode bytes in position 2-3: unexpected end of data", (
        f'utf-16 position includes BOM: {e}'
    )
# Big-endian errors report the -be name.
try:
    b'\xd8\x00a\x00'.decode('utf-16-be')
    assert False, 'utf-16-be lone high surrogate should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-16-be' codec can't decode bytes in position 0-1: illegal UTF-16 surrogate", (
        f'utf-16-be error name: {e}'
    )

# === utf-16 decode error handlers ===
assert b'a\x00\x00\xd8b\x00'.decode('utf-16-le', 'replace') == 'a�b', 'utf-16 replace bad unit'
assert b'a\x00\x00\xd8'.decode('utf-16-le', 'replace') == 'a�', 'utf-16 replace truncated tail'
assert b'a\x00\x00\xd8a'.decode('utf-16-le', 'replace') == 'a�', 'utf-16 replace 3-byte tail is one U+FFFD'
assert b'a\x00\x00\xd8b\x00'.decode('utf-16-le', 'ignore') == 'ab', 'utf-16 ignore bad unit'
assert b'a\x00b'.decode('utf-16-le', 'ignore') == 'a', 'utf-16 ignore truncated byte'
assert b'a\x00\x00\xd8b\x00'.decode('utf-16-le', 'backslashreplace') == 'a\\x00\\xd8b', (
    'utf-16 backslashreplace escapes each byte of the bad unit'
)
# Handler names are validated lazily, and encode-only handlers raise TypeError.
assert b'a\x00'.decode('utf-16-le', 'bogus') == 'a', 'utf-16 unused handler never validated'
try:
    b'a\x00b'.decode('utf-16-le', 'xmlcharrefreplace')
    assert False, 'encode-only handler on decode should error'
except TypeError as e:
    assert str(e) == "don't know how to handle UnicodeDecodeError in error callback", (
        f'utf-16 decode xmlcharrefreplace: {e}'
    )

# === utf-32 decode errors ===
try:
    b'\xff\xff\xff\x00'.decode('utf-32-le')
    assert False, 'out-of-range code point should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-32-le' codec can't decode bytes in position 0-3: code point not in range(0x110000)", (
        f'utf-32 out of range: {e}'
    )
try:
    b'\x00\xd8\x00\x00'.decode('utf-32-le')
    assert False, 'surrogate code point should error'
except UnicodeDecodeError as e:
    assert str(e) == (
        "'utf-32-le' codec can't decode bytes in position 0-3: code point in surrogate code point range(0xd800, 0xe000)"
    ), f'utf-32 surrogate code point: {e}'
try:
    b'a\x00\x00'.decode('utf-32-le')
    assert False, 'truncated utf-32 should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-32-le' codec can't decode bytes in position 0-2: truncated data", (
        f'utf-32 truncated 3-byte tail: {e}'
    )
# A single trailing byte uses the byte-form message.
try:
    b'a'.decode('utf-32-le')
    assert False, 'single byte utf-32 should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-32-le' codec can't decode byte 0x61 in position 0: truncated data", (
        f'utf-32 single-byte form: {e}'
    )
# Positions include the consumed BOM.
try:
    b'\xff\xfe\x00\x00a\x00'.decode('utf-32')
    assert False, 'truncated utf-32 after BOM should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-32-le' codec can't decode bytes in position 4-5: truncated data", (
        f'utf-32 position includes BOM: {e}'
    )
assert b'\xff\xff\xff\x00a\x00\x00\x00'.decode('utf-32-le', 'replace') == '�a', 'utf-32 replace bad code point'
assert b'\xff\xff\xff\x00a\x00\x00\x00'.decode('utf-32-le', 'ignore') == 'a', 'utf-32 ignore bad code point'
assert b'\x00\xd8\x00\x00'.decode('utf-32-le', 'backslashreplace') == '\\x00\\xd8\\x00\\x00', (
    'utf-32 backslashreplace escapes all four bytes'
)

# === utf-8 decode: precise strict messages ===
try:
    b'a\xffb'.decode('utf-8')
    assert False, 'invalid start byte should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode byte 0xff in position 1: invalid start byte", (
        f'utf-8 invalid start byte: {e}'
    )
try:
    b'\x80'.decode('utf-8')
    assert False, 'stray continuation byte should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode byte 0x80 in position 0: invalid start byte", (
        f'utf-8 stray continuation byte: {e}'
    )
try:
    b'a\xe2\x28b'.decode('utf-8')
    assert False, 'invalid continuation should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode byte 0xe2 in position 1: invalid continuation byte", (
        f'utf-8 invalid continuation byte: {e}'
    )
# A multi-byte maximal subpart uses the range-form message.
try:
    b'\xf0\x9f(a'.decode('utf-8')
    assert False, 'partial 4-byte sequence should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte", (
        f'utf-8 multi-byte subpart range form: {e}'
    )
try:
    b'a\xe2\x82'.decode('utf-8')
    assert False, 'truncated sequence should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data", (
        f'utf-8 truncated multi-byte sequence: {e}'
    )
try:
    b'\xc3'.decode('utf-8')
    assert False, 'truncated 2-byte sequence should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode byte 0xc3 in position 0: unexpected end of data", (
        f'utf-8 truncated single lead byte: {e}'
    )
# A CESU-8-encoded surrogate is rejected at its lead byte.
try:
    b'a\xed\xa0\x80b'.decode('utf-8')
    assert False, 'CESU-8 surrogate should error'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode byte 0xed in position 1: invalid continuation byte", (
        f'utf-8 CESU-8 surrogate: {e}'
    )

# === utf-8 decode error handlers ===
# replace substitutes one U+FFFD per maximal subpart (not per byte).
assert b'a\xe2\x82b\xf0\x9f\x98'.decode('utf-8', 'replace') == 'a�b�', 'utf-8 replace maximal subparts'
assert b'a\xf0\x28\x8c\x28b'.decode('utf-8', 'replace') == 'a�(�(b', 'utf-8 replace resumes after subpart'
assert b'a\xed\xa0\x80b'.decode('utf-8', 'replace') == 'a���b', 'utf-8 replace CESU-8 surrogate is three subparts'
assert b'a\xe2\x82b'.decode('utf-8', 'ignore') == 'ab', 'utf-8 ignore drops subpart'
assert b'a\xe2\x82b'.decode('utf-8', 'backslashreplace') == 'a\\xe2\\x82b', 'utf-8 backslashreplace escapes each byte'
# Handler validation stays lazy; surrogatepass re-raises for non-surrogate errors.
assert b'hello'.decode('utf-8', 'bogus') == 'hello', 'utf-8 unused handler never validated'
try:
    b'h\xffllo'.decode('utf-8', 'bogus')
    assert False, 'unknown handler should error once needed'
except LookupError as e:
    assert str(e) == "unknown error handler name 'bogus'", f'utf-8 unknown handler: {e}'
try:
    b'\xff'.decode('utf-8', 'surrogatepass')
    assert False, 'surrogatepass should re-raise for non-surrogate errors'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte", (
        f'utf-8 surrogatepass re-raises strict: {e}'
    )
try:
    b'a\xffb'.decode('utf-8', 'xmlcharrefreplace')
    assert False, 'encode-only handler on utf-8 decode should error'
except TypeError as e:
    assert str(e) == "don't know how to handle UnicodeDecodeError in error callback", (
        f'utf-8 decode xmlcharrefreplace: {e}'
    )
