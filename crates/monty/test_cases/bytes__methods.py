# === bytes.decode() ===
assert b'hello'.decode() == 'hello'
assert b'hello'.decode('utf-8') == 'hello'
assert b'hello'.decode('utf8') == 'hello'
assert b'hello'.decode('UTF-8') == 'hello'
assert b''.decode() == ''

# Non-ASCII UTF-8
assert b'\xc3\xa9'.decode() == '\xe9'
assert b'\xe4\xb8\xad'.decode() == '\u4e2d'

# === bytes.count() ===
assert b'hello'.count(b'l') == 2
assert b'hello'.count(b'll') == 1
assert b'hello'.count(b'x') == 0
assert b'aaa'.count(b'aa') == 1
assert b''.count(b'x') == 0
assert b'hello'.count(b'') == 6

# count with start/end
assert b'abcabc'.count(b'ab', 1) == 1
assert b'abcabc'.count(b'ab', 0, 3) == 1

# === bytes.find() ===
assert b'hello'.find(b'e') == 1
assert b'hello'.find(b'll') == 2
assert b'hello'.find(b'x') == -1
assert b'hello'.find(b'') == 0
assert b''.find(b'x') == -1

# find with start/end
assert b'hello'.find(b'l', 3) == 3
assert b'hello'.find(b'l', 0, 2) == -1

# === bytes.index() ===
assert b'hello'.index(b'e') == 1
assert b'hello'.index(b'll') == 2
assert b'hello'.index(b'') == 0

# === bytes.startswith() ===
assert b'hello'.startswith(b'he')
assert not b'hello'.startswith(b'lo'), 'startswith false'
assert b'hello'.startswith(b'')
assert b''.startswith(b'')
assert not b''.startswith(b'x'), 'empty startswith non-empty'

# startswith with start/end
assert b'abcdef'.startswith(b'bc', 1)
assert b'abcdef'.startswith(b'bc', 1, 3)
assert not b'abcdef'.startswith(b'bc', 2), 'startswith with start past match'
assert not b'abcdef'.startswith(b'abc', 0, 2), 'startswith with end before match ends'

# === bytes.endswith() ===
assert b'hello'.endswith(b'lo')
assert not b'hello'.endswith(b'he'), 'endswith false'
assert b'hello'.endswith(b'')
assert b''.endswith(b'')
assert not b''.endswith(b'x'), 'empty endswith non-empty'

# endswith with start/end
assert b'abcdef'.endswith(b'de', 0, 5)
assert b'abcdef'.endswith(b'cd', 1, 4)
assert not b'abcdef'.endswith(b'de', 0, 4), 'endswith before suffix'

# === Edge case: start > end (should not panic, treat as empty slice) ===
assert b'hello'.find(b'e', 5, 2) == -1
assert b'hello'.count(b'l', 5, 2) == 0
assert not b'hello'.startswith(b'h', 5, 2), 'startswith with start > end is false'
assert not b'hello'.endswith(b'o', 5, 2), 'endswith with start > end is false'

# === Edge case: i64::MIN start/end — regression: `-index` on i64::MIN used to panic ===
_I64_MIN = -(2**63)
assert b'hello'.startswith(b'h', _I64_MIN)
assert not b'hello'.startswith(b'h', 0, _I64_MIN), 'startswith with i64::MIN end clamps to 0'
assert not b'hello'.endswith(b'o', _I64_MIN, _I64_MIN), 'endswith with i64::MIN bounds'

# === bytes.lower() ===
assert b'HELLO'.lower() == b'hello'
assert b'Hello World'.lower() == b'hello world'
assert b'hello'.lower() == b'hello'
assert b''.lower() == b''
assert b'123ABC'.lower() == b'123abc'
assert b'\x80\xff'.lower() == b'\x80\xff'

# === bytes.upper() ===
assert b'hello'.upper() == b'HELLO'
assert b'Hello World'.upper() == b'HELLO WORLD'
assert b'HELLO'.upper() == b'HELLO'
assert b''.upper() == b''
assert b'123abc'.upper() == b'123ABC'

# === bytes.capitalize() ===
assert b'hello'.capitalize() == b'Hello'
assert b'HELLO'.capitalize() == b'Hello'
assert b'hELLO wORLD'.capitalize() == b'Hello world'
assert b''.capitalize() == b''
assert b'123hello'.capitalize() == b'123hello'

# === bytes.title() ===
assert b'hello world'.title() == b'Hello World'
assert b'HELLO WORLD'.title() == b'Hello World'
assert b"they're bill's".title() == b"They'Re Bill'S"
assert b''.title() == b''

# === bytes.swapcase() ===
assert b'Hello World'.swapcase() == b'hELLO wORLD'
assert b'HELLO'.swapcase() == b'hello'
assert b'hello'.swapcase() == b'HELLO'
assert b''.swapcase() == b''

# === bytes.isalpha() ===
assert b'hello'.isalpha()
assert not b'hello123'.isalpha(), 'isalpha with digits'
assert not b'hello world'.isalpha(), 'isalpha with space'
assert not b''.isalpha(), 'isalpha empty is false'
assert b'ABC'.isalpha()

# === bytes.isdigit() ===
assert b'123'.isdigit()
assert not b'123abc'.isdigit(), 'isdigit with letters'
assert not b''.isdigit(), 'isdigit empty is false'

# === bytes.isalnum() ===
assert b'hello123'.isalnum()
assert b'hello'.isalnum()
assert b'123'.isalnum()
assert not b'hello world'.isalnum(), 'isalnum with space'
assert not b''.isalnum(), 'isalnum empty is false'

# === bytes.isspace() ===
assert b' \t\n\r'.isspace()
assert not b'hello'.isspace(), 'isspace not all whitespace'
assert not b''.isspace(), 'isspace empty is false'
assert b' '.isspace()

# === bytes.islower() ===
assert b'hello'.islower()
assert b'hello123'.islower()
assert not b'Hello'.islower(), 'islower with uppercase'
assert not b'HELLO'.islower(), 'islower all uppercase'
assert not b''.islower(), 'islower empty is false'
assert not b'123'.islower(), 'islower no cased chars is false'

# === bytes.isupper() ===
assert b'HELLO'.isupper()
assert b'HELLO123'.isupper()
assert not b'Hello'.isupper(), 'isupper with lowercase'
assert not b'hello'.isupper(), 'isupper all lowercase'
assert not b''.isupper(), 'isupper empty is false'
assert not b'123'.isupper(), 'isupper no cased chars is false'

# === bytes.isascii() ===
assert b'hello'.isascii()
assert b''.isascii()
assert b'\x00\x7f'.isascii()
assert not b'\x80'.isascii(), 'isascii non-ascii byte'
assert not b'hello\xff'.isascii(), 'isascii with non-ascii'

# === bytes.istitle() ===
assert b'Hello World'.istitle()
assert not b'hello world'.istitle(), 'istitle lowercase'
assert not b'HELLO WORLD'.istitle(), 'istitle uppercase'
assert b'Hello'.istitle()
assert not b''.istitle(), 'istitle empty is false'

# === bytes.rfind() ===
assert b'hello'.rfind(b'l') == 3
assert b'hello'.rfind(b'x') == -1
assert b'hello'.rfind(b'') == 5
assert b'aaaa'.rfind(b'aa') == 2
assert b'hello'.rfind(b'l', 0, 3) == 2

# === bytes.rindex() ===
assert b'hello'.rindex(b'l') == 3
assert b'hello'.rindex(b'') == 5

# === bytes.strip() ===
assert b'  hello  '.strip() == b'hello'
assert b'hello'.strip() == b'hello'
assert b'xxxhelloxxx'.strip(b'x') == b'hello'
assert b''.strip() == b''
assert b'   '.strip() == b''

# === bytes.lstrip() ===
assert b'  hello  '.lstrip() == b'hello  '
assert b'xxxhello'.lstrip(b'x') == b'hello'
assert b''.lstrip() == b''

# === bytes.rstrip() ===
assert b'  hello  '.rstrip() == b'  hello'
assert b'helloxxx'.rstrip(b'x') == b'hello'
assert b''.rstrip() == b''

# === bytes.removeprefix() ===
assert b'hello'.removeprefix(b'he') == b'llo'
assert b'hello'.removeprefix(b'xxx') == b'hello'
assert b'hello'.removeprefix(b'') == b'hello'
assert b''.removeprefix(b'x') == b''

# === bytes.removesuffix() ===
assert b'hello'.removesuffix(b'lo') == b'hel'
assert b'hello'.removesuffix(b'xxx') == b'hello'
assert b'hello'.removesuffix(b'') == b'hello'
assert b''.removesuffix(b'x') == b''

# === bytes.split() ===
assert b'a,b,c'.split(b',') == [b'a', b'b', b'c']
assert b'a b c'.split() == [b'a', b'b', b'c']
assert b'a  b  c'.split() == [b'a', b'b', b'c']
assert b'a,b,c'.split(b',', 1) == [b'a', b'b,c']
assert b''.split() == []
assert b'hello'.split(b'x') == [b'hello']

# === bytes.rsplit() ===
assert b'a,b,c'.rsplit(b',') == [b'a', b'b', b'c']
assert b'a,b,c'.rsplit(b',', 1) == [b'a,b', b'c']
assert b'a b c'.rsplit() == [b'a', b'b', b'c']

# === bytes.splitlines() ===
assert b'a\nb\nc'.splitlines() == [b'a', b'b', b'c']
assert b'a\r\nb\rc'.splitlines() == [b'a', b'b', b'c']
assert b'a\nb\n'.splitlines() == [b'a', b'b']
assert b'a\nb'.splitlines(True) == [b'a\n', b'b']
assert b''.splitlines() == []
assert b'a\nb'.splitlines(keepends=True) == [b'a\n', b'b']
assert b'a\nb'.splitlines(keepends=False) == [b'a', b'b']

# === bytes.partition() ===
assert b'hello world'.partition(b' ') == (b'hello', b' ', b'world')
assert b'hello'.partition(b'x') == (b'hello', b'', b'')
assert b'hello world here'.partition(b' ') == (b'hello', b' ', b'world here')

# === bytes.rpartition() ===
assert b'hello world'.rpartition(b' ') == (b'hello', b' ', b'world')
assert b'hello'.rpartition(b'x') == (b'', b'', b'hello')
assert b'hello world here'.rpartition(b' ') == (b'hello world', b' ', b'here')

# === bytes.replace() ===
assert b'hello'.replace(b'l', b'L') == b'heLLo'
assert b'hello'.replace(b'l', b'L', 1) == b'heLlo'
assert b'hello'.replace(b'x', b'y') == b'hello'
assert b'aaa'.replace(b'a', b'bb') == b'bbbbbb'
assert b'aaa'.replace(b'aa', b'b') == b'ba'

# === bytes.center() ===
assert b'hello'.center(10) == b'  hello   '
assert b'hello'.center(10, b'*') == b'**hello***'
assert b'hello'.center(3) == b'hello'

# === bytes.ljust() ===
assert b'hello'.ljust(10) == b'hello     '
assert b'hello'.ljust(10, b'*') == b'hello*****'
assert b'hello'.ljust(3) == b'hello'

# === bytes.rjust() ===
assert b'hello'.rjust(10) == b'     hello'
assert b'hello'.rjust(10, b'*') == b'*****hello'
assert b'hello'.rjust(3) == b'hello'

# === bytes.zfill() ===
assert b'42'.zfill(5) == b'00042'
assert b'-42'.zfill(5) == b'-0042'
assert b'+42'.zfill(5) == b'+0042'
assert b'hello'.zfill(3) == b'hello'

# === bytes.join() ===
assert b','.join([b'a', b'b', b'c']) == b'a,b,c'
assert b''.join([b'a', b'b']) == b'ab'
assert b','.join([]) == b''
assert b'-'.join([b'hello']) == b'hello'

# === bytes.hex() ===
assert b'\xde\xad\xbe\xef'.hex() == 'deadbeef'
assert b''.hex() == ''
assert b'AB'.hex() == '4142'
assert b'\x00\xff'.hex() == '00ff'
assert b'\xde\xad\xbe\xef'.hex(':') == 'de:ad:be:ef'
assert b'\xde\xad\xbe\xef'.hex(':', 2) == 'dead:beef'
# Test positive bytes_per_sep (partial group at start)
assert b'\x01\x02\x03\x04\x05'.hex(':', 2) == '01:0203:0405'
assert b'\x01\x02\x03'.hex(':', 2) == '01:0203'
# Test negative bytes_per_sep (partial group at end)
assert b'\x01\x02\x03\x04\x05'.hex(':', -2) == '0102:0304:05'
assert b'\x01\x02\x03'.hex(':', -2) == '0102:03'
# bytes_per_sep is parsed as a C int by CPython; out-of-range values raise OverflowError.
try:
    b'A'.hex(':', -9223372036854775808)
    assert False, 'expected OverflowError for i64::MIN'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C int'
try:
    b'A'.hex(':', 2147483648)
    assert False, 'expected OverflowError for i32::MAX + 1'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C int'
# Values at the i32 boundary are still accepted and processed as a single chunk.
assert b'\x01\x02\x03'.hex(':', -2147483648) == '010203'
assert b'\x01\x02\x03'.hex(':', 2147483647) == '010203'

# === bytes.fromhex() ===
assert bytes.fromhex('deadbeef') == b'\xde\xad\xbe\xef'
assert bytes.fromhex('DEADBEEF') == b'\xde\xad\xbe\xef'
assert bytes.fromhex('') == b''
assert bytes.fromhex('de ad be ef') == b'\xde\xad\xbe\xef'
assert bytes.fromhex('4142') == b'AB'

# === bytes.fromhex() with whitespace ===
# Whitespace is only allowed BETWEEN byte pairs, not within a pair
assert bytes.fromhex(' 01 ') == b'\x01'
assert bytes.fromhex('01 23') == b'\x01\x23'

# === bytes.fromhex() errors ===
# Odd number of hex digits (no invalid chars, just odd count)
try:
    bytes.fromhex('0')
    assert False, 'fromhex odd digits should error'
except ValueError as e:
    assert str(e) == 'fromhex() arg must contain an even number of hexadecimal digits', (
        f'fromhex odd digits message, error: {e}'
    )

try:
    bytes.fromhex(' 0')
    assert False, 'fromhex odd digits after whitespace should error'
except ValueError as e:
    assert str(e) == 'fromhex() arg must contain an even number of hexadecimal digits', (
        f'fromhex odd digits after whitespace message, error: {e}'
    )

# Whitespace within a byte pair is invalid (space is not a hex digit)
try:
    bytes.fromhex('0 1')
    assert False, 'fromhex whitespace within pair should error'
except ValueError as e:
    assert str(e) == 'non-hexadecimal number found in fromhex() arg at position 1', (
        f'fromhex whitespace within pair message, error: {e}'
    )

# Invalid hex character
try:
    bytes.fromhex('0g')
    assert False, 'fromhex invalid hex char should error'
except ValueError as e:
    assert str(e) == 'non-hexadecimal number found in fromhex() arg at position 1', (
        f'fromhex invalid hex char message, error: {e}'
    )

# === bytes.fromhex() instance access ===
# fromhex is a classmethod but should also work on instances
assert b''.fromhex('4142') == b'AB'
assert b'hello'.fromhex('deadbeef') == b'\xde\xad\xbe\xef'

# === bytes.startswith/endswith with tuple of prefixes ===
assert b'hello'.startswith((b'he', b'wo'))
assert b'hello'.startswith((b'wo', b'he'))
assert not b'hello'.startswith((b'wo', b'ab')), 'startswith tuple no match'
assert b'hello'.startswith((b'',))
assert b'hello'.startswith((b'hello', b'world'))

assert b'hello'.endswith((b'lo', b'ld'))
assert b'hello'.endswith((b'ld', b'lo'))
assert not b'hello'.endswith((b'he', b'ab')), 'endswith tuple no match'
assert b'hello'.endswith((b'',))
assert b'hello'.endswith((b'hello', b'world'))

# startswith/endswith tuple with start/end
assert b'abcdef'.startswith((b'bc', b'cd'), 1)
assert b'abcdef'.endswith((b'de', b'cd'), 0, 5)

# === Empty-substring edge cases ===
# Edge case: start == len (boundary) - this works
assert b'hello'.find(b'', 5) == 5
assert b'hello'.count(b'', 5) == 1
assert b'hello'.startswith(b'', 5)
assert b'hello'.endswith(b'', 5)

# TODO: These edge cases when start > len need to be fixed
# CPython returns -1/0/False for these, currently Monty doesn't handle this correctly
# assert b'hello'.find(b'', 10) == -1, 'find empty when start > len returns -1'
# assert b'hello'.count(b'', 10) == 0, 'count empty when start > len returns 0'
# assert not b'hello'.startswith(b'', 10), 'startswith empty when start > len is false'
# assert not b'hello'.endswith(b'', 10), 'endswith empty when start > len is false'
# assert b'hello'.rfind(b'', 10) == -1, 'rfind empty when start > len returns -1'

# === bytes.hex() non-ASCII separator errors ===
try:
    b'\x01\x02'.hex('\xff')
    assert False, 'hex with non-ASCII separator should error'
except ValueError as e:
    # CPython uses 'sep must be ASCII.' with period
    msg = str(e)
    assert 'sep' in msg.lower() and 'ascii' in msg.lower(), f'hex non-ASCII sep message, error: {e}'

# === bytes.decode() with errors argument ===
# Valid errors values
assert b'hello'.decode('utf-8', 'strict') == 'hello'
assert b'hello'.decode('utf-8', 'ignore') == 'hello'
assert b'hello'.decode('utf-8', 'replace') == 'hello'

# === bytes.decode() with the 'ascii' codec ===
assert b'hello'.decode('ascii') == 'hello'
assert b'hello'.decode('us-ascii') == 'hello'
assert b'hello'.decode('us_ascii') == 'hello'
assert b'hello'.decode('US_ASCII') == 'hello'
assert b'hello'.decode('ASCII') == 'hello'
assert b'hello \xe9 world'.decode('ascii', 'ignore') == 'hello  world'
assert b'hello \xe9 world'.decode('ascii', 'replace') == 'hello � world'
assert b'hello \xe9 world'.decode('ascii', 'backslashreplace') == 'hello \\xe9 world'
# Consecutive bad bytes: each is handled independently (no merging, unlike encode's strict range message).
assert b'\xe9\xf6\xff'.decode('ascii', 'ignore') == ''
assert b'\xe9\xf6\xff'.decode('ascii', 'replace') == '���'
assert b'\xe9\xf6\xff'.decode('ascii', 'backslashreplace') == '\\xe9\\xf6\\xff'
assert b'x\xe9\xf6\xffy'.decode('ascii', 'ignore') == 'xy'
assert b'x\xe9\xf6\xffy'.decode('ascii', 'replace') == 'x���y'
assert b'x\xe9\xf6\xffy'.decode('ascii', 'backslashreplace') == 'x\\xe9\\xf6\\xffy'
# encode(errors='ignore') then decode('ascii') round-trips since only ASCII bytes remain.
assert 'café — 日本語 test'.encode('ascii', 'ignore').decode('ascii') == 'caf   test'

# strict (the default) raises UnicodeDecodeError, a ValueError subclass, with CPython's exact wording.
try:
    b'hello \xe9 world'.decode('ascii')
    assert False, 'decode ascii of non-ascii bytes should error'
except ValueError as e:
    assert isinstance(e, UnicodeDecodeError)
    assert type(e).__name__ == 'UnicodeDecodeError', f'exception type name: {type(e).__name__}'
    assert str(e) == "'ascii' codec can't decode byte 0xe9 in position 6: ordinal not in range(128)", (
        f'decode ascii strict message: {e}'
    )

# Boundary: the bad byte at the very start (position 0) of the bytes object.
try:
    b'\xe9xyz'.decode('ascii')
    assert False, 'decode ascii of non-ascii bytes should error'
except UnicodeDecodeError as e:
    assert str(e) == "'ascii' codec can't decode byte 0xe9 in position 0: ordinal not in range(128)", (
        f'decode ascii strict bad byte at position 0: {e}'
    )
# Boundary: the bad byte at the very end of the bytes object.
try:
    b'xyz\xe9'.decode('ascii')
    assert False, 'decode ascii of non-ascii bytes should error'
except UnicodeDecodeError as e:
    assert str(e) == "'ascii' codec can't decode byte 0xe9 in position 3: ordinal not in range(128)", (
        f'decode ascii strict bad byte at last position: {e}'
    )
# Boundary: a single-byte bytes object that is itself non-ascii.
try:
    b'\xe9'.decode('ascii')
    assert False, 'decode ascii of non-ascii bytes should error'
except UnicodeDecodeError as e:
    assert str(e) == "'ascii' codec can't decode byte 0xe9 in position 0: ordinal not in range(128)", (
        f'decode ascii strict single-byte bytes: {e}'
    )

# surrogatepass only special-cases surrogate sequences in the UTF codecs, so
# with the ascii codec it re-raises exactly like strict.
assert b'hello'.decode('ascii', 'surrogatepass') == 'hello'
try:
    b'h\xe9llo'.decode('ascii', 'surrogatepass')
    assert False, 'decode ascii surrogatepass of non-ascii bytes should error'
except UnicodeDecodeError as e:
    assert str(e) == "'ascii' codec can't decode byte 0xe9 in position 1: ordinal not in range(128)", (
        f'decode ascii surrogatepass behaves like strict: {e}'
    )

# xmlcharrefreplace/namereplace are encode-only handlers: unused they pass
# (lazy lookup), but a bad byte triggers CPython's callback TypeError.
assert b'hello'.decode('ascii', 'xmlcharrefreplace') == 'hello'
assert b'hello'.decode('ascii', 'namereplace') == 'hello'
try:
    b'h\xe9llo'.decode('ascii', 'xmlcharrefreplace')
    assert False, 'decode ascii xmlcharrefreplace of non-ascii bytes should error'
except TypeError as e:
    assert str(e) == "don't know how to handle UnicodeDecodeError in error callback", (
        f'decode ascii xmlcharrefreplace callback error: {e}'
    )
try:
    b'h\xe9llo'.decode('ascii', 'namereplace')
    assert False, 'decode ascii namereplace of non-ascii bytes should error'
except TypeError as e:
    assert str(e) == "don't know how to handle UnicodeDecodeError in error callback", (
        f'decode ascii namereplace callback error: {e}'
    )

# surrogateescape passes unused (lazy lookup, like CPython); a bad byte raises
# NotImplementedError in Monty — CPython would produce a lone surrogate, which
# Monty strings cannot represent. The divergent-error path is covered by a
# Rust-side regression test (`crates/monty/tests/encoding.rs`) since this suite
# also runs under CPython.
assert b'hello'.decode('ascii', 'surrogateescape') == 'hello'

# Like CPython, an unknown error handler name is only looked up if it's actually needed.
assert b'hello'.decode('ascii', 'bogus') == 'hello'
try:
    b'hello \xe9 world'.decode('ascii', 'bogus')
    assert False, 'decode ascii with unknown error handler should error'
except LookupError as e:
    assert str(e) == "unknown error handler name 'bogus'", f'decode ascii unknown error handler: {e}'

try:
    b'hello'.decode('not-a-real-codec')
    assert False, 'decode with unsupported codec should error'
except LookupError as e:
    assert str(e) == 'unknown encoding: not-a-real-codec', f'decode unknown encoding: {e}'

# === bytes.decode() type-error wording (CPython `_PyArg_BadArgument`) ===
# Wrong-type encoding / errors must produce the named bad-arg wording
# (`decode() argument '<name>' must be str, not <type>`) including the
# `None`-vs-`NoneType` special case. Driven by `bad_arg_named` on
# `BytesDecodeArgs`.
for bad, expected_type in ((42, 'int'), (None, 'None'), (b'utf-8', 'bytes')):
    try:
        b'hello'.decode(bad)
        assert False, f'decode({bad!r}) should error'
    except TypeError as e:
        assert str(e) == f"decode() argument 'encoding' must be str, not {expected_type}", (
            f'decode({bad!r}) wrong type: {e}'
        )
    try:
        b'hello'.decode(encoding=bad)
        assert False, f'decode(encoding={bad!r}) should error'
    except TypeError as e:
        assert str(e) == f"decode() argument 'encoding' must be str, not {expected_type}", (
            f'decode(encoding={bad!r}) wrong type: {e}'
        )
    try:
        b'hello'.decode('utf-8', bad)
        assert False, f'decode(errors={bad!r}) should error'
    except TypeError as e:
        assert str(e) == f"decode() argument 'errors' must be str, not {expected_type}", (
            f'decode(errors={bad!r}) wrong type: {e}'
        )

# === Error message for unknown classmethod ===
# Error message should say 'bytes' not 'type'
try:
    bytes.nonexistent()
    assert False, 'should raise AttributeError'
except AttributeError as e:
    msg = str(e)
    assert 'bytes' in msg, f'error should mention bytes, got: {e}'
    assert 'nonexistent' in msg, f'error should mention method name, got: {e}'
