# === bytes.decode() ===
assert b'hello'.decode() == 'hello', 'decode default utf-8'
assert b'hello'.decode('utf-8') == 'hello', 'decode explicit utf-8'
assert b'hello'.decode('utf8') == 'hello', 'decode utf8 variant'
assert b'hello'.decode('UTF-8') == 'hello', 'decode uppercase UTF-8'
assert b''.decode() == '', 'decode empty bytes'

# Non-ASCII UTF-8
assert b'\xc3\xa9'.decode() == '\xe9', 'decode utf-8 e-acute'
assert b'\xe4\xb8\xad'.decode() == '\u4e2d', 'decode utf-8 CJK character'

# === bytes.count() ===
assert b'hello'.count(b'l') == 2, 'count single char'
assert b'hello'.count(b'll') == 1, 'count subsequence'
assert b'hello'.count(b'x') == 0, 'count not found'
assert b'aaa'.count(b'aa') == 1, 'count non-overlapping'
assert b''.count(b'x') == 0, 'count in empty bytes'
assert b'hello'.count(b'') == 6, 'count empty subsequence'

# count with start/end
assert b'abcabc'.count(b'ab', 1) == 1, 'count with start'
assert b'abcabc'.count(b'ab', 0, 3) == 1, 'count with start and end'

# === bytes.find() ===
assert b'hello'.find(b'e') == 1, 'find single char'
assert b'hello'.find(b'll') == 2, 'find subsequence'
assert b'hello'.find(b'x') == -1, 'find not found'
assert b'hello'.find(b'') == 0, 'find empty subsequence'
assert b''.find(b'x') == -1, 'find in empty bytes'

# find with start/end
assert b'hello'.find(b'l', 3) == 3, 'find with start'
assert b'hello'.find(b'l', 0, 2) == -1, 'find with end before match'

# === bytes.index() ===
assert b'hello'.index(b'e') == 1, 'index single char'
assert b'hello'.index(b'll') == 2, 'index subsequence'
assert b'hello'.index(b'') == 0, 'index empty subsequence'

# === bytes.startswith() ===
assert b'hello'.startswith(b'he'), 'startswith true'
assert not b'hello'.startswith(b'lo'), 'startswith false'
assert b'hello'.startswith(b''), 'startswith empty'
assert b''.startswith(b''), 'empty startswith empty'
assert not b''.startswith(b'x'), 'empty startswith non-empty'

# startswith with start/end
assert b'abcdef'.startswith(b'bc', 1), 'startswith with start'
assert b'abcdef'.startswith(b'bc', 1, 3), 'startswith with start and end'
assert not b'abcdef'.startswith(b'bc', 2), 'startswith with start past match'
assert not b'abcdef'.startswith(b'abc', 0, 2), 'startswith with end before match ends'

# === bytes.endswith() ===
assert b'hello'.endswith(b'lo'), 'endswith true'
assert not b'hello'.endswith(b'he'), 'endswith false'
assert b'hello'.endswith(b''), 'endswith empty'
assert b''.endswith(b''), 'empty endswith empty'
assert not b''.endswith(b'x'), 'empty endswith non-empty'

# endswith with start/end
assert b'abcdef'.endswith(b'de', 0, 5), 'endswith with end'
assert b'abcdef'.endswith(b'cd', 1, 4), 'endswith with start and end'
assert not b'abcdef'.endswith(b'de', 0, 4), 'endswith before suffix'

# === Edge case: start > end (should not panic, treat as empty slice) ===
assert b'hello'.find(b'e', 5, 2) == -1, 'find with start > end returns -1'
assert b'hello'.count(b'l', 5, 2) == 0, 'count with start > end returns 0'
assert not b'hello'.startswith(b'h', 5, 2), 'startswith with start > end is false'
assert not b'hello'.endswith(b'o', 5, 2), 'endswith with start > end is false'

# === bytes.lower() ===
assert b'HELLO'.lower() == b'hello', 'lower basic'
assert b'Hello World'.lower() == b'hello world', 'lower mixed case'
assert b'hello'.lower() == b'hello', 'lower already lowercase'
assert b''.lower() == b'', 'lower empty'
assert b'123ABC'.lower() == b'123abc', 'lower with digits'
assert b'\x80\xff'.lower() == b'\x80\xff', 'lower non-ascii unchanged'

# === bytes.upper() ===
assert b'hello'.upper() == b'HELLO', 'upper basic'
assert b'Hello World'.upper() == b'HELLO WORLD', 'upper mixed case'
assert b'HELLO'.upper() == b'HELLO', 'upper already uppercase'
assert b''.upper() == b'', 'upper empty'
assert b'123abc'.upper() == b'123ABC', 'upper with digits'

# === bytes.capitalize() ===
assert b'hello'.capitalize() == b'Hello', 'capitalize basic'
assert b'HELLO'.capitalize() == b'Hello', 'capitalize uppercase'
assert b'hELLO wORLD'.capitalize() == b'Hello world', 'capitalize mixed'
assert b''.capitalize() == b'', 'capitalize empty'
assert b'123hello'.capitalize() == b'123hello', 'capitalize starting with digit'

# === bytes.title() ===
assert b'hello world'.title() == b'Hello World', 'title basic'
assert b'HELLO WORLD'.title() == b'Hello World', 'title uppercase'
assert b"they're bill's".title() == b"They'Re Bill'S", 'title with apostrophe'
assert b''.title() == b'', 'title empty'

# === bytes.swapcase() ===
assert b'Hello World'.swapcase() == b'hELLO wORLD', 'swapcase basic'
assert b'HELLO'.swapcase() == b'hello', 'swapcase uppercase'
assert b'hello'.swapcase() == b'HELLO', 'swapcase lowercase'
assert b''.swapcase() == b'', 'swapcase empty'

# === bytes.isalpha() ===
assert b'hello'.isalpha(), 'isalpha all letters'
assert not b'hello123'.isalpha(), 'isalpha with digits'
assert not b'hello world'.isalpha(), 'isalpha with space'
assert not b''.isalpha(), 'isalpha empty is false'
assert b'ABC'.isalpha(), 'isalpha uppercase'

# === bytes.isdigit() ===
assert b'123'.isdigit(), 'isdigit all digits'
assert not b'123abc'.isdigit(), 'isdigit with letters'
assert not b''.isdigit(), 'isdigit empty is false'

# === bytes.isalnum() ===
assert b'hello123'.isalnum(), 'isalnum letters and digits'
assert b'hello'.isalnum(), 'isalnum all letters'
assert b'123'.isalnum(), 'isalnum all digits'
assert not b'hello world'.isalnum(), 'isalnum with space'
assert not b''.isalnum(), 'isalnum empty is false'

# === bytes.isspace() ===
assert b' \t\n\r'.isspace(), 'isspace whitespace chars'
assert not b'hello'.isspace(), 'isspace not all whitespace'
assert not b''.isspace(), 'isspace empty is false'
assert b' '.isspace(), 'isspace single space'

# === bytes.islower() ===
assert b'hello'.islower(), 'islower all lowercase'
assert b'hello123'.islower(), 'islower with digits'
assert not b'Hello'.islower(), 'islower with uppercase'
assert not b'HELLO'.islower(), 'islower all uppercase'
assert not b''.islower(), 'islower empty is false'
assert not b'123'.islower(), 'islower no cased chars is false'

# === bytes.isupper() ===
assert b'HELLO'.isupper(), 'isupper all uppercase'
assert b'HELLO123'.isupper(), 'isupper with digits'
assert not b'Hello'.isupper(), 'isupper with lowercase'
assert not b'hello'.isupper(), 'isupper all lowercase'
assert not b''.isupper(), 'isupper empty is false'
assert not b'123'.isupper(), 'isupper no cased chars is false'

# === bytes.isascii() ===
assert b'hello'.isascii(), 'isascii all ascii'
assert b''.isascii(), 'isascii empty is true'
assert b'\x00\x7f'.isascii(), 'isascii boundary values'
assert not b'\x80'.isascii(), 'isascii non-ascii byte'
assert not b'hello\xff'.isascii(), 'isascii with non-ascii'

# === bytes.istitle() ===
assert b'Hello World'.istitle(), 'istitle basic'
assert not b'hello world'.istitle(), 'istitle lowercase'
assert not b'HELLO WORLD'.istitle(), 'istitle uppercase'
assert b'Hello'.istitle(), 'istitle single word'
assert not b''.istitle(), 'istitle empty is false'

# === bytes.rfind() ===
assert b'hello'.rfind(b'l') == 3, 'rfind finds last occurrence'
assert b'hello'.rfind(b'x') == -1, 'rfind not found'
assert b'hello'.rfind(b'') == 5, 'rfind empty at end'
assert b'aaaa'.rfind(b'aa') == 2, 'rfind non-overlapping from right'
assert b'hello'.rfind(b'l', 0, 3) == 2, 'rfind with range'

# === bytes.rindex() ===
assert b'hello'.rindex(b'l') == 3, 'rindex finds last occurrence'
assert b'hello'.rindex(b'') == 5, 'rindex empty at end'

# === bytes.strip() ===
assert b'  hello  '.strip() == b'hello', 'strip whitespace'
assert b'hello'.strip() == b'hello', 'strip no whitespace'
assert b'xxxhelloxxx'.strip(b'x') == b'hello', 'strip custom chars'
assert b''.strip() == b'', 'strip empty'
assert b'   '.strip() == b'', 'strip all whitespace'

# === bytes.lstrip() ===
assert b'  hello  '.lstrip() == b'hello  ', 'lstrip whitespace'
assert b'xxxhello'.lstrip(b'x') == b'hello', 'lstrip custom chars'
assert b''.lstrip() == b'', 'lstrip empty'

# === bytes.rstrip() ===
assert b'  hello  '.rstrip() == b'  hello', 'rstrip whitespace'
assert b'helloxxx'.rstrip(b'x') == b'hello', 'rstrip custom chars'
assert b''.rstrip() == b'', 'rstrip empty'

# === bytes.removeprefix() ===
assert b'hello'.removeprefix(b'he') == b'llo', 'removeprefix found'
assert b'hello'.removeprefix(b'xxx') == b'hello', 'removeprefix not found'
assert b'hello'.removeprefix(b'') == b'hello', 'removeprefix empty'
assert b''.removeprefix(b'x') == b'', 'removeprefix empty bytes'

# === bytes.removesuffix() ===
assert b'hello'.removesuffix(b'lo') == b'hel', 'removesuffix found'
assert b'hello'.removesuffix(b'xxx') == b'hello', 'removesuffix not found'
assert b'hello'.removesuffix(b'') == b'hello', 'removesuffix empty'
assert b''.removesuffix(b'x') == b'', 'removesuffix empty bytes'

# === bytes.split() ===
assert b'a,b,c'.split(b',') == [b'a', b'b', b'c'], 'split basic'
assert b'a b c'.split() == [b'a', b'b', b'c'], 'split whitespace'
assert b'a  b  c'.split() == [b'a', b'b', b'c'], 'split multiple whitespace'
assert b'a,b,c'.split(b',', 1) == [b'a', b'b,c'], 'split maxsplit'
assert b''.split() == [], 'split empty bytes'
assert b'hello'.split(b'x') == [b'hello'], 'split not found'

# === bytes.rsplit() ===
assert b'a,b,c'.rsplit(b',') == [b'a', b'b', b'c'], 'rsplit basic'
assert b'a,b,c'.rsplit(b',', 1) == [b'a,b', b'c'], 'rsplit maxsplit'
assert b'a b c'.rsplit() == [b'a', b'b', b'c'], 'rsplit whitespace'

# === bytes.splitlines() ===
assert b'a\nb\nc'.splitlines() == [b'a', b'b', b'c'], 'splitlines newline'
assert b'a\r\nb\rc'.splitlines() == [b'a', b'b', b'c'], 'splitlines mixed'
assert b'a\nb\n'.splitlines() == [b'a', b'b'], 'splitlines trailing'
assert b'a\nb'.splitlines(True) == [b'a\n', b'b'], 'splitlines keepends'
assert b''.splitlines() == [], 'splitlines empty'

# === bytes.partition() ===
assert b'hello world'.partition(b' ') == (b'hello', b' ', b'world'), 'partition found'
assert b'hello'.partition(b'x') == (b'hello', b'', b''), 'partition not found'
assert b'hello world here'.partition(b' ') == (b'hello', b' ', b'world here'), 'partition first'

# === bytes.rpartition() ===
assert b'hello world'.rpartition(b' ') == (b'hello', b' ', b'world'), 'rpartition found'
assert b'hello'.rpartition(b'x') == (b'', b'', b'hello'), 'rpartition not found'
assert b'hello world here'.rpartition(b' ') == (b'hello world', b' ', b'here'), 'rpartition last'

# === bytes.replace() ===
assert b'hello'.replace(b'l', b'L') == b'heLLo', 'replace all'
assert b'hello'.replace(b'l', b'L', 1) == b'heLlo', 'replace count'
assert b'hello'.replace(b'x', b'y') == b'hello', 'replace not found'
assert b'aaa'.replace(b'a', b'bb') == b'bbbbbb', 'replace longer'
assert b'aaa'.replace(b'aa', b'b') == b'ba', 'replace non-overlapping'

# === bytes.center() ===
assert b'hello'.center(10) == b'  hello   ', 'center basic'
assert b'hello'.center(10, b'*') == b'**hello***', 'center fillbyte'
assert b'hello'.center(3) == b'hello', 'center too short'

# === bytes.ljust() ===
assert b'hello'.ljust(10) == b'hello     ', 'ljust basic'
assert b'hello'.ljust(10, b'*') == b'hello*****', 'ljust fillbyte'
assert b'hello'.ljust(3) == b'hello', 'ljust too short'

# === bytes.rjust() ===
assert b'hello'.rjust(10) == b'     hello', 'rjust basic'
assert b'hello'.rjust(10, b'*') == b'*****hello', 'rjust fillbyte'
assert b'hello'.rjust(3) == b'hello', 'rjust too short'

# === bytes.zfill() ===
assert b'42'.zfill(5) == b'00042', 'zfill basic'
assert b'-42'.zfill(5) == b'-0042', 'zfill negative'
assert b'+42'.zfill(5) == b'+0042', 'zfill positive'
assert b'hello'.zfill(3) == b'hello', 'zfill too short'

# === bytes.join() ===
assert b','.join([b'a', b'b', b'c']) == b'a,b,c', 'join list'
assert b''.join([b'a', b'b']) == b'ab', 'join empty separator'
assert b','.join([]) == b'', 'join empty iterable'
assert b'-'.join([b'hello']) == b'hello', 'join single item'

# === bytes.hex() ===
assert b'\xde\xad\xbe\xef'.hex() == 'deadbeef', 'hex basic'
assert b''.hex() == '', 'hex empty'
assert b'AB'.hex() == '4142', 'hex letters'
assert b'\x00\xff'.hex() == '00ff', 'hex boundary'
assert b'\xde\xad\xbe\xef'.hex(':') == 'de:ad:be:ef', 'hex with separator'
assert b'\xde\xad\xbe\xef'.hex(':', 2) == 'dead:beef', 'hex with bytes_per_sep'
# Test positive bytes_per_sep (partial group at start)
assert b'\x01\x02\x03\x04\x05'.hex(':', 2) == '01:0203:0405', 'hex +2 odd bytes'
assert b'\x01\x02\x03'.hex(':', 2) == '01:0203', 'hex +2 three bytes'
# Test negative bytes_per_sep (partial group at end)
assert b'\x01\x02\x03\x04\x05'.hex(':', -2) == '0102:0304:05', 'hex -2 odd bytes'
assert b'\x01\x02\x03'.hex(':', -2) == '0102:03', 'hex -2 three bytes'

# === bytes.fromhex() ===
assert bytes.fromhex('deadbeef') == b'\xde\xad\xbe\xef', 'fromhex basic'
assert bytes.fromhex('DEADBEEF') == b'\xde\xad\xbe\xef', 'fromhex uppercase'
assert bytes.fromhex('') == b'', 'fromhex empty'
assert bytes.fromhex('de ad be ef') == b'\xde\xad\xbe\xef', 'fromhex with spaces'
assert bytes.fromhex('4142') == b'AB', 'fromhex letters'

# === bytes.fromhex() with whitespace ===
# Whitespace is only allowed BETWEEN byte pairs, not within a pair
assert bytes.fromhex(' 01 ') == b'\x01', 'fromhex whitespace around bytes is stripped'
assert bytes.fromhex('01 23') == b'\x01\x23', 'fromhex whitespace between byte pairs'

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
assert b''.fromhex('4142') == b'AB', 'fromhex on bytes instance'
assert b'hello'.fromhex('deadbeef') == b'\xde\xad\xbe\xef', 'fromhex on non-empty instance'

# === bytes.startswith/endswith with tuple of prefixes ===
assert b'hello'.startswith((b'he', b'wo')), 'startswith tuple first match'
assert b'hello'.startswith((b'wo', b'he')), 'startswith tuple second match'
assert not b'hello'.startswith((b'wo', b'ab')), 'startswith tuple no match'
assert b'hello'.startswith((b'',)), 'startswith tuple with empty bytes'
assert b'hello'.startswith((b'hello', b'world')), 'startswith tuple exact match'

assert b'hello'.endswith((b'lo', b'ld')), 'endswith tuple first match'
assert b'hello'.endswith((b'ld', b'lo')), 'endswith tuple second match'
assert not b'hello'.endswith((b'he', b'ab')), 'endswith tuple no match'
assert b'hello'.endswith((b'',)), 'endswith tuple with empty bytes'
assert b'hello'.endswith((b'hello', b'world')), 'endswith tuple exact match'

# startswith/endswith tuple with start/end
assert b'abcdef'.startswith((b'bc', b'cd'), 1), 'startswith tuple with start'
assert b'abcdef'.endswith((b'de', b'cd'), 0, 5), 'endswith tuple with end'

# === Empty-substring edge cases ===
# Edge case: start == len (boundary) - this works
assert b'hello'.find(b'', 5) == 5, 'find empty at len returns len'
assert b'hello'.count(b'', 5) == 1, 'count empty at len returns 1'
assert b'hello'.startswith(b'', 5), 'startswith empty at len is true'
assert b'hello'.endswith(b'', 5), 'endswith empty at len is true'

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
assert b'hello'.decode('utf-8', 'strict') == 'hello', 'decode with strict errors'
assert b'hello'.decode('utf-8', 'ignore') == 'hello', 'decode with ignore errors'
assert b'hello'.decode('utf-8', 'replace') == 'hello', 'decode with replace errors'

# TODO: errors argument type validation - CPython raises TypeError for non-string errors
# This is not implemented yet
# try:
#     b'hello'.decode('utf-8', 123)
#     assert False, 'decode with non-string errors should error'
# except TypeError as e:
#     assert 'str' in str(e), f'decode errors type error should mention str, error: {e}'

# === Error message for unknown classmethod ===
# Error message should say 'bytes' not 'type'
try:
    bytes.nonexistent()
    assert False, 'should raise AttributeError'
except AttributeError as e:
    msg = str(e)
    assert 'bytes' in msg, f'error should mention bytes, got: {e}'
    assert 'nonexistent' in msg, f'error should mention method name, got: {e}'
