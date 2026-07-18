# === Phase 1: Simple transformations ===

# lower()
assert 'HELLO'.lower() == 'hello'
assert 'Hello World'.lower() == 'hello world'
assert 'hello'.lower() == 'hello'
assert ''.lower() == ''
assert '123'.lower() == '123'

# upper()
assert 'hello'.upper() == 'HELLO'
assert 'Hello World'.upper() == 'HELLO WORLD'
assert 'HELLO'.upper() == 'HELLO'
assert ''.upper() == ''
assert '123'.upper() == '123'

# capitalize()
assert 'hello'.capitalize() == 'Hello'
assert 'HELLO'.capitalize() == 'Hello'
assert 'hELLO wORLD'.capitalize() == 'Hello world'
assert ''.capitalize() == ''
assert '123abc'.capitalize() == '123abc'

# title()
assert 'hello world'.title() == 'Hello World'
assert 'HELLO WORLD'.title() == 'Hello World'
assert "they're".title() == "They'Re"
assert ''.title() == ''
assert '123 abc'.title() == '123 Abc'

# swapcase()
assert 'Hello World'.swapcase() == 'hELLO wORLD'
assert 'HELLO'.swapcase() == 'hello'
assert 'hello'.swapcase() == 'HELLO'
assert ''.swapcase() == ''

# casefold()
assert 'Hello'.casefold() == 'hello'
assert 'HELLO'.casefold() == 'hello'
assert ''.casefold() == ''

# === Phase 2: Predicate methods ===

# isalpha()
assert 'hello'.isalpha() == True
assert 'Hello'.isalpha() == True
assert ''.isalpha() == False
assert 'hello123'.isalpha() == False
assert 'hello world'.isalpha() == False

# isdigit()
assert '123'.isdigit() == True
assert ''.isdigit() == False
assert '123abc'.isdigit() == False
assert '12 34'.isdigit() == False

# isalnum()
assert 'hello123'.isalnum() == True
assert 'hello'.isalnum() == True
assert '123'.isalnum() == True
assert ''.isalnum() == False
assert 'hello 123'.isalnum() == False

# isnumeric()
assert '123'.isnumeric() == True
assert ''.isnumeric() == False
assert '123abc'.isnumeric() == False

# isspace()
assert '   '.isspace() == True
assert '\t\n'.isspace() == True
assert ''.isspace() == False
assert ' a '.isspace() == False

# islower()
assert 'hello'.islower() == True
assert 'Hello'.islower() == False
assert ''.islower() == False
assert '123'.islower() == False
assert 'hello123'.islower() == True

# isupper()
assert 'HELLO'.isupper() == True
assert 'Hello'.isupper() == False
assert ''.isupper() == False
assert '123'.isupper() == False
assert 'HELLO123'.isupper() == True

# isascii()
assert 'hello'.isascii() == True
assert ''.isascii() == True
assert '\x00\x7f'.isascii() == True

# isdecimal()
assert '123'.isdecimal() == True
assert ''.isdecimal() == False
assert '123abc'.isdecimal() == False

# === Phase 3: Search methods ===

# find()
assert 'hello'.find('l') == 2
assert 'hello'.find('ll') == 2
assert 'hello'.find('x') == -1
assert 'hello'.find('') == 0
assert 'hello'.find('l', 3) == 3
assert 'hello'.find('l', 0, 3) == 2

# find()/startswith() with i64::MIN start/end — regression: `-index` on i64::MIN used to panic
_I64_MIN = -(2**63)
assert 'hello'.find('h', _I64_MIN) == 0
assert 'hello'.find('h', 0, _I64_MIN) == -1
assert 'hello'.startswith('h', _I64_MIN) == True
assert 'hello'.startswith('h', _I64_MIN, _I64_MIN) == False

# rfind()
assert 'hello'.rfind('l') == 3
assert 'hello'.rfind('x') == -1
assert 'hello'.rfind('l', 0, 3) == 2

# index()
assert 'hello'.index('l') == 2
assert 'hello'.index('ll') == 2

# rindex()
assert 'hello'.rindex('l') == 3

# count()
assert 'hello'.count('l') == 2
assert 'hello'.count('ll') == 1
assert 'hello'.count('x') == 0
assert 'hello'.count('') == 6
assert 'aaa'.count('a') == 3

# startswith()
assert 'hello'.startswith('he') == True
assert 'hello'.startswith('lo') == False
assert 'hello'.startswith('') == True
assert 'hello'.startswith('ell', 1) == True

# endswith()
assert 'hello'.endswith('lo') == True
assert 'hello'.endswith('he') == False
assert 'hello'.endswith('') == True
assert 'hello'.endswith('ell', 0, 4) == True

# === Phase 4: Strip/trim methods ===

# strip()
assert '  hello  '.strip() == 'hello'
assert 'xxhelloxx'.strip('x') == 'hello'
assert 'hello'.strip() == 'hello'
assert ''.strip() == ''
assert '   '.strip() == ''

# lstrip()
assert '  hello  '.lstrip() == 'hello  '
assert 'xxhello'.lstrip('x') == 'hello'
assert 'hello'.lstrip() == 'hello'

# rstrip()
assert '  hello  '.rstrip() == '  hello'
assert 'helloxx'.rstrip('x') == 'hello'
assert 'hello'.rstrip() == 'hello'

# removeprefix()
assert 'hello world'.removeprefix('hello ') == 'world'
assert 'hello world'.removeprefix('world') == 'hello world'
assert 'hello'.removeprefix('') == 'hello'

# removesuffix()
assert 'hello world'.removesuffix(' world') == 'hello'
assert 'hello world'.removesuffix('hello') == 'hello world'
assert 'hello'.removesuffix('') == 'hello'

# === Phase 5: Split methods ===

# split()
assert 'a b c'.split() == ['a', 'b', 'c']
assert 'a,b,c'.split(',') == ['a', 'b', 'c']
assert 'a,b,c'.split(',', 1) == ['a', 'b,c']
assert '  a  b  '.split() == ['a', 'b']
assert 'hello'.split('x') == ['hello']

# rsplit()
assert 'a b c'.rsplit() == ['a', 'b', 'c']
assert 'a,b,c'.rsplit(',') == ['a', 'b', 'c']
assert 'a,b,c'.rsplit(',', 1) == ['a,b', 'c']
# Multi-byte whitespace must not panic on UTF-8 boundary (U+00A0, U+3000).
assert 'hello world'.rsplit(maxsplit=1) == ['hello', 'world']
assert 'a　b　c'.rsplit(maxsplit=1) == ['a　b', 'c']
assert 'a b c'.rsplit(maxsplit=2) == ['a', 'b', 'c']
assert 'a b c'.rsplit(maxsplit=0) == ['a b c']
# Runs of whitespace count as one separator.
assert 'a  b'.rsplit(maxsplit=2) == ['a', 'b']
assert 'a\xa0\xa0b'.rsplit(maxsplit=2) == ['a', 'b']
assert '  a  b  '.rsplit(maxsplit=1) == ['  a', 'b']

# splitlines()
assert 'a\nb\nc'.splitlines() == ['a', 'b', 'c']
assert 'a\nb\nc'.splitlines(True) == ['a\n', 'b\n', 'c']
assert 'a\r\nb'.splitlines() == ['a', 'b']
assert ''.splitlines() == []

# partition()
assert 'hello world'.partition(' ') == ('hello', ' ', 'world')
assert 'hello'.partition('x') == ('hello', '', '')
assert 'hello world test'.partition(' ') == ('hello', ' ', 'world test')

# rpartition()
assert 'hello world'.rpartition(' ') == ('hello', ' ', 'world')
assert 'hello'.rpartition('x') == ('', '', 'hello')
assert 'hello world test'.rpartition(' ') == ('hello world', ' ', 'test')

# === Phase 6: Replace/modify methods ===

# replace()
assert 'hello'.replace('l', 'L') == 'heLLo'
assert 'hello'.replace('l', 'L', 1) == 'heLlo'
assert 'hello'.replace('x', 'y') == 'hello'
assert 'aaa'.replace('a', 'b') == 'bbb'
assert ''.replace('a', 'b') == ''

# center()
assert 'hi'.center(6) == '  hi  '
assert 'hi'.center(6, '-') == '--hi--'
assert 'hi'.center(2) == 'hi'
assert 'hi'.center(1) == 'hi'

# ljust()
assert 'hi'.ljust(6) == 'hi    '
assert 'hi'.ljust(6, '-') == 'hi----'
assert 'hi'.ljust(2) == 'hi'

# rjust()
assert 'hi'.rjust(6) == '    hi'
assert 'hi'.rjust(6, '-') == '----hi'
assert 'hi'.rjust(2) == 'hi'

# zfill()
assert '42'.zfill(5) == '00042'
assert '-42'.zfill(5) == '-0042'
assert '+42'.zfill(5) == '+0042'
assert '42'.zfill(2) == '42'
assert ''.zfill(3) == '000'

# === Phase 7: Additional tests for Python compatibility ===

# startswith/endswith with tuple
assert 'hello'.startswith(('he', 'lo')) == True
assert 'hello'.startswith(('lo', 'he')) == True
assert 'hello'.startswith(('x', 'y')) == False
assert 'hello'.endswith(('he', 'lo')) == True
assert 'hello'.endswith(('lo', 'he')) == True
assert 'hello'.endswith(('x', 'y')) == False
assert 'hello'.startswith(('ell',), 1) == True

# startswith/endswith affix validation matches CPython: tuple elements are
# validated lazily and in order, so a match short-circuits before later
# elements are type-checked
assert 'hello'.startswith(('he', 1)) == True
assert 'hello'.endswith(('lo', 1)) == True
try:
    'hello'.startswith(('xx', 1))
    assert False, 'expected startswith invalid tuple element to raise'
except TypeError as exc:
    assert str(exc) == 'tuple for startswith must only contain str, not int'
try:
    'hello'.endswith((1, 'lo'))
    assert False, 'expected endswith invalid element before match to raise'
except TypeError as exc:
    assert str(exc) == 'tuple for endswith must only contain str, not int'
try:
    'hello'.startswith(42)
    assert False, 'expected startswith non-str affix to raise'
except TypeError as exc:
    assert str(exc) == 'startswith first arg must be str or a tuple of str, not int'
try:
    'hello'.endswith(None)
    assert False, 'expected endswith None affix to raise'
except TypeError as exc:
    assert str(exc) == 'endswith first arg must be str or a tuple of str, not NoneType'

# bad start/end indices raise before the affix is inspected, with CPython's message
try:
    'hello'.startswith(42, 'x')
    assert False, 'expected startswith bad start to raise'
except TypeError as exc:
    assert str(exc) == 'slice indices must be integers or None or have an __index__ method'
try:
    'hello'.endswith(('lo', 1), None, 'x')
    assert False, 'expected endswith bad end to raise'
except TypeError as exc:
    assert str(exc) == 'slice indices must be integers or None or have an __index__ method'

# bool is accepted as a slice index (int subtype)
assert 'hello'.startswith('ello', True) == True
assert 'hello'.count('l', True) == 2

# find/rfind/index/rindex/count with None as start/end
assert 'hello'.find('l', None) == 2
assert 'hello'.find('l', None, None) == 2
assert 'hello'.find('l', 0, None) == 2
assert 'hello'.rfind('l', None, None) == 3
assert 'hello'.count('l', None, None) == 2
assert 'hello'.startswith('he', None) == True
assert 'hello'.endswith('lo', None, None) == True

# strip with None
assert '  hello  '.strip(None) == 'hello'
assert '  hello  '.lstrip(None) == 'hello  '
assert '  hello  '.rstrip(None) == '  hello'

# === Phase 8: Keyword argument tests ===

# split with keyword args
assert 'a,b,c'.split(sep=',') == ['a', 'b', 'c']
assert 'a,b,c'.split(',', maxsplit=1) == ['a', 'b,c']
assert 'a,b,c'.split(sep=',', maxsplit=1) == ['a', 'b,c']

# rsplit with keyword args
assert 'a,b,c'.rsplit(sep=',') == ['a', 'b', 'c']
assert 'a,b,c'.rsplit(',', maxsplit=1) == ['a,b', 'c']
assert 'a,b,c'.rsplit(sep=',', maxsplit=1) == ['a,b', 'c']

# splitlines with keyword args
assert 'a\nb\nc'.splitlines(keepends=True) == ['a\n', 'b\n', 'c']
assert 'a\nb\nc'.splitlines(keepends=False) == ['a', 'b', 'c']

# replace with keyword args
assert 'aaa'.replace('a', 'b', count=2) == 'bba'

# === Phase 9: Additional methods ===

# encode()
assert 'hello'.encode() == b'hello'
assert 'hello'.encode('utf-8') == b'hello'
assert 'hello'.encode('utf8') == b'hello'
assert 'hello'.encode('utf_8') == b'hello'
assert 'hello'.encode('UTF-8') == b'hello'
assert ''.encode() == b''
assert 'hello'.encode('utf-8', 'strict') == b'hello'

# === encode() with the 'ascii' codec ===
assert 'hello'.encode('ascii') == b'hello'
assert 'hello'.encode('us-ascii') == b'hello'
assert 'hello'.encode('us_ascii') == b'hello'
assert 'hello'.encode('US_ASCII') == b'hello'
assert 'hello'.encode('ASCII') == b'hello'
assert 'héllo wörld ⚡'.encode('ascii', 'ignore') == b'hllo wrld '
assert 'héllo wörld ⚡'.encode('ascii', 'replace') == b'h?llo w?rld ?'
assert 'héllo wörld ⚡'.encode('ascii', 'backslashreplace') == b'h\\xe9llo w\\xf6rld \\u26a1'
# Non-BMP characters (> U+FFFF) escape via the \Uxxxxxxxx form, not \uxxxx.
assert 'a\U0001f600b'.encode('ascii', 'backslashreplace') == b'a\\U0001f600b'
# The 'ignore' handler round-trips through decode('ascii') since only ASCII bytes remain.
assert 'café — 日本語 test'.encode('ascii', 'ignore').decode('ascii') == 'caf   test'

# strict (the default) raises UnicodeEncodeError, a ValueError subclass, with CPython's exact wording.
try:
    'héllo'.encode('ascii')
    assert False, 'encode ascii of non-ascii string should error'
except ValueError as e:
    assert isinstance(e, UnicodeEncodeError)
    assert type(e).__name__ == 'UnicodeEncodeError', f'exception type name: {type(e).__name__}'
    assert str(e) == "'ascii' codec can't encode character '\\xe9' in position 1: ordinal not in range(128)", (
        f'encode ascii strict single-char message: {e}'
    )
try:
    'aéöbé'.encode('ascii')
    assert False, 'encode ascii of non-ascii string should error'
except UnicodeEncodeError as e:
    assert str(e) == "'ascii' codec can't encode characters in position 1-2: ordinal not in range(128)", (
        f'encode ascii strict multi-char range message: {e}'
    )

# Boundary: the bad character at the very start (position 0) of the string.
try:
    'éxyz'.encode('ascii')
    assert False, 'encode ascii of non-ascii string should error'
except UnicodeEncodeError as e:
    assert str(e) == "'ascii' codec can't encode character '\\xe9' in position 0: ordinal not in range(128)", (
        f'encode ascii strict bad char at position 0: {e}'
    )
# Boundary: the bad character at the very end of the string.
try:
    'xyzé'.encode('ascii')
    assert False, 'encode ascii of non-ascii string should error'
except UnicodeEncodeError as e:
    assert str(e) == "'ascii' codec can't encode character '\\xe9' in position 3: ordinal not in range(128)", (
        f'encode ascii strict bad char at last position: {e}'
    )
# Boundary: a single-character string that is itself non-ascii.
try:
    'é'.encode('ascii')
    assert False, 'encode ascii of non-ascii string should error'
except UnicodeEncodeError as e:
    assert str(e) == "'ascii' codec can't encode character '\\xe9' in position 0: ordinal not in range(128)", (
        f'encode ascii strict single-char string: {e}'
    )

# xmlcharrefreplace substitutes decimal XML character references.
assert 'héllo ⚡'.encode('ascii', 'xmlcharrefreplace') == b'h&#233;llo &#9889;'
assert 'a\U0001f600b'.encode('ascii', 'xmlcharrefreplace') == b'a&#128512;b'
# namereplace substitutes \N{...} escapes, falling back to backslash escapes
# for characters with no Unicode name (e.g. C1 controls).
assert 'héllo ⚡'.encode('ascii', 'namereplace') == b'h\\N{LATIN SMALL LETTER E WITH ACUTE}llo \\N{HIGH VOLTAGE SIGN}'
assert '一'.encode('ascii', 'namereplace') == b'\\N{CJK UNIFIED IDEOGRAPH-4E00}'
assert 'a\x80\x9fb'.encode('ascii', 'namereplace') == b'a\\x80\\x9fb'
# surrogateescape/surrogatepass only special-case lone surrogates, which a
# valid str can never contain here, so they re-raise exactly like strict.
assert 'hello'.encode('ascii', 'surrogateescape') == b'hello'
assert 'hello'.encode('ascii', 'surrogatepass') == b'hello'
try:
    'héllo'.encode('ascii', 'surrogateescape')
    assert False, 'encode ascii surrogateescape of non-ascii string should error'
except UnicodeEncodeError as e:
    assert str(e) == "'ascii' codec can't encode character '\\xe9' in position 1: ordinal not in range(128)", (
        f'encode ascii surrogateescape behaves like strict: {e}'
    )
try:
    'héllo'.encode('ascii', 'surrogatepass')
    assert False, 'encode ascii surrogatepass of non-ascii string should error'
except UnicodeEncodeError as e:
    assert str(e) == "'ascii' codec can't encode character '\\xe9' in position 1: ordinal not in range(128)", (
        f'encode ascii surrogatepass behaves like strict: {e}'
    )

# Like CPython, an unknown error handler name is only looked up if it's actually needed.
assert 'hello'.encode('ascii', 'bogus') == b'hello'
try:
    'héllo'.encode('ascii', 'bogus')
    assert False, 'encode ascii with unknown error handler should error'
except LookupError as e:
    assert str(e) == "unknown error handler name 'bogus'", f'encode ascii unknown error handler: {e}'

try:
    'hello'.encode('not-a-real-codec')
    assert False, 'encode with unsupported codec should error'
except LookupError as e:
    assert str(e) == 'unknown encoding: not-a-real-codec', f'encode unknown encoding: {e}'

# Wrong-type encoding/errors → CPython's `_PyArg_BadArgument` named wording.
# Routed through `bad_arg_named` on `EncodeArgs`; matches CPython exactly,
# including `None`-vs-`NoneType` (lone `None` reads as `"not None"`).
for bad, expected_type in ((42, 'int'), (None, 'None'), (b'utf-8', 'bytes')):
    try:
        'hello'.encode(bad)
        assert False, f'encode({bad!r}) should error'
    except TypeError as e:
        assert str(e) == f"encode() argument 'encoding' must be str, not {expected_type}", (
            f'encode({bad!r}) wrong type: {e}'
        )
    try:
        'hello'.encode('utf-8', bad)
        assert False, f'encode(errors={bad!r}) should error'
    except TypeError as e:
        assert str(e) == f"encode() argument 'errors' must be str, not {expected_type}", (
            f'encode(errors={bad!r}) wrong type: {e}'
        )

# isidentifier()
assert 'hello'.isidentifier() == True
assert '_hello'.isidentifier() == True
assert '__init__'.isidentifier() == True
assert 'hello123'.isidentifier() == True
assert ''.isidentifier() == False
assert '123hello'.isidentifier() == False
assert 'hello world'.isidentifier() == False
assert 'hello-world'.isidentifier() == False
assert 'class'.isidentifier() == True  # isidentifier doesn't check keywords

# istitle()
assert 'Hello World'.istitle() == True
assert 'Hello'.istitle() == True
assert 'HELLO'.istitle() == False
assert 'hello'.istitle() == False
assert ''.istitle() == False
assert 'Hello world'.istitle() == False
assert '123'.istitle() == False
assert 'Hello 123 World'.istitle() == True
assert "They'Re".istitle() == True

# === Phase 10: Unicode support for is* methods ===

# isdecimal with Unicode decimal digits
assert '٠١٢٣٤٥٦٧٨٩'.isdecimal() == True
assert '０１２３４５６７８９'.isdecimal() == True
assert '०१२३४५६७८९'.isdecimal() == True
assert '²'.isdecimal() == False
assert '½'.isdecimal() == False

# isdigit with superscripts and subscripts
assert '²³'.isdigit() == True
assert '₀₁₂₃₄₅₆₇₈₉'.isdigit() == True
assert '0123456789'.isdigit() == True
assert '٠١٢٣٤٥٦٧٨٩'.isdigit() == True
assert '½'.isdigit() == False

# isnumeric with fractions and other numerics
assert '½'.isnumeric() == True
assert '²'.isnumeric() == True
assert '٠١٢٣٤٥٦٧٨٩'.isnumeric() == True
assert '0123456789'.isnumeric() == True

# === Phase 11: expandtabs ===

# expandtabs() default tabsize=8
assert '\thello'.expandtabs() == '        hello'
assert ''.expandtabs() == ''
assert 'no tabs here'.expandtabs() == 'no tabs here'

# expandtabs() with explicit tabsize
assert '\thello'.expandtabs(4) == '    hello'
assert '\thello'.expandtabs(8) == '        hello'
assert '\thello'.expandtabs(1) == ' hello'

# expandtabs() column tracking (tabs align to next tabstop)
assert 'a\tb'.expandtabs() == 'a       b'
assert 'ab\tcd'.expandtabs() == 'ab      cd'
assert 'abcdefg\th'.expandtabs() == 'abcdefg h'
assert 'abcdefgh\ti'.expandtabs() == 'abcdefgh        i'
assert 'a\tb\tc'.expandtabs(4) == 'a   b   c'

# expandtabs() with tabsize=0 (tabs become nothing)
assert '\thello'.expandtabs(0) == 'hello'
assert 'a\tb\tc'.expandtabs(0) == 'abc'

# expandtabs() with negative tabsize (treated as 0)
assert '\thello'.expandtabs(-1) == 'hello'

# expandtabs() with newlines resetting column
assert 'a\tb\nc\td'.expandtabs(4) == 'a   b\nc   d'
assert '\t\n\t'.expandtabs(4) == '    \n    '
assert 'a\tb\rc\td'.expandtabs(4) == 'a   b\rc   d'

# expandtabs() with keyword argument
assert '\thello'.expandtabs(tabsize=4) == '    hello'

# expandtabs() error cases
try:
    'hello'.expandtabs(wrong=4)
    assert False, 'expandtabs wrong kwarg should raise'
except TypeError as e:
    assert str(e) == "expandtabs() got an unexpected keyword argument 'wrong'", f'wrong: {e}'

try:
    'hello'.expandtabs(4, tabsize=8)
    assert False, 'expandtabs pos + kwarg should raise'
except TypeError as e:
    assert str(e) == 'expandtabs() takes at most 1 argument (2 given)', f'dup: {e}'

try:
    'hello'.expandtabs(4, 5)
    assert False, 'expandtabs too many args should raise'
except TypeError as e:
    assert str(e) == 'expandtabs() takes at most 1 argument (2 given)', f'toomany: {e}'

# splitlines() error cases
try:
    'hello'.splitlines(wrong=True)
    assert False, 'splitlines wrong kwarg should raise'
except TypeError as e:
    assert str(e) == "splitlines() got an unexpected keyword argument 'wrong'", f'wrong: {e}'

try:
    'hello'.splitlines(True, keepends=False)
    assert False, 'splitlines pos + kwarg should raise'
except TypeError as e:
    assert str(e) == 'splitlines() takes at most 1 argument (2 given)', f'dup: {e}'
