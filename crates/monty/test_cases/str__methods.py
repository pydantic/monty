# === Phase 1: Simple transformations ===

# lower()
assert 'HELLO'.lower() == 'hello', 'lower basic'
assert 'Hello World'.lower() == 'hello world', 'lower mixed'
assert 'hello'.lower() == 'hello', 'lower already lower'
assert ''.lower() == '', 'lower empty'
assert '123'.lower() == '123', 'lower numbers unchanged'

# upper()
assert 'hello'.upper() == 'HELLO', 'upper basic'
assert 'Hello World'.upper() == 'HELLO WORLD', 'upper mixed'
assert 'HELLO'.upper() == 'HELLO', 'upper already upper'
assert ''.upper() == '', 'upper empty'
assert '123'.upper() == '123', 'upper numbers unchanged'

# capitalize()
assert 'hello'.capitalize() == 'Hello', 'capitalize basic'
assert 'HELLO'.capitalize() == 'Hello', 'capitalize all upper'
assert 'hELLO wORLD'.capitalize() == 'Hello world', 'capitalize mixed'
assert ''.capitalize() == '', 'capitalize empty'
assert '123abc'.capitalize() == '123abc', 'capitalize number start'

# title()
assert 'hello world'.title() == 'Hello World', 'title basic'
assert 'HELLO WORLD'.title() == 'Hello World', 'title all upper'
assert "they're".title() == "They'Re", 'title apostrophe'
assert ''.title() == '', 'title empty'
assert '123 abc'.title() == '123 Abc', 'title number start'

# swapcase()
assert 'Hello World'.swapcase() == 'hELLO wORLD', 'swapcase basic'
assert 'HELLO'.swapcase() == 'hello', 'swapcase all upper'
assert 'hello'.swapcase() == 'HELLO', 'swapcase all lower'
assert ''.swapcase() == '', 'swapcase empty'

# casefold()
assert 'Hello'.casefold() == 'hello', 'casefold basic'
assert 'HELLO'.casefold() == 'hello', 'casefold all upper'
assert ''.casefold() == '', 'casefold empty'

# === Phase 2: Predicate methods ===

# isalpha()
assert 'hello'.isalpha() == True, 'isalpha basic'
assert 'Hello'.isalpha() == True, 'isalpha mixed case'
assert ''.isalpha() == False, 'isalpha empty'
assert 'hello123'.isalpha() == False, 'isalpha with digits'
assert 'hello world'.isalpha() == False, 'isalpha with space'

# isdigit()
assert '123'.isdigit() == True, 'isdigit basic'
assert ''.isdigit() == False, 'isdigit empty'
assert '123abc'.isdigit() == False, 'isdigit with letters'
assert '12 34'.isdigit() == False, 'isdigit with space'

# isalnum()
assert 'hello123'.isalnum() == True, 'isalnum basic'
assert 'hello'.isalnum() == True, 'isalnum letters only'
assert '123'.isalnum() == True, 'isalnum digits only'
assert ''.isalnum() == False, 'isalnum empty'
assert 'hello 123'.isalnum() == False, 'isalnum with space'

# isnumeric()
assert '123'.isnumeric() == True, 'isnumeric basic'
assert ''.isnumeric() == False, 'isnumeric empty'
assert '123abc'.isnumeric() == False, 'isnumeric with letters'

# isspace()
assert '   '.isspace() == True, 'isspace spaces'
assert '\t\n'.isspace() == True, 'isspace tabs and newlines'
assert ''.isspace() == False, 'isspace empty'
assert ' a '.isspace() == False, 'isspace with letter'

# islower()
assert 'hello'.islower() == True, 'islower basic'
assert 'Hello'.islower() == False, 'islower mixed'
assert ''.islower() == False, 'islower empty'
assert '123'.islower() == False, 'islower numbers only'
assert 'hello123'.islower() == True, 'islower with numbers'

# isupper()
assert 'HELLO'.isupper() == True, 'isupper basic'
assert 'Hello'.isupper() == False, 'isupper mixed'
assert ''.isupper() == False, 'isupper empty'
assert '123'.isupper() == False, 'isupper numbers only'
assert 'HELLO123'.isupper() == True, 'isupper with numbers'

# isascii()
assert 'hello'.isascii() == True, 'isascii basic'
assert ''.isascii() == True, 'isascii empty'
assert '\x00\x7f'.isascii() == True, 'isascii boundary'

# isdecimal()
assert '123'.isdecimal() == True, 'isdecimal basic'
assert ''.isdecimal() == False, 'isdecimal empty'
assert '123abc'.isdecimal() == False, 'isdecimal with letters'

# === Phase 3: Search methods ===

# find()
assert 'hello'.find('l') == 2, 'find basic'
assert 'hello'.find('ll') == 2, 'find substring'
assert 'hello'.find('x') == -1, 'find not found'
assert 'hello'.find('') == 0, 'find empty string'
assert 'hello'.find('l', 3) == 3, 'find with start'
assert 'hello'.find('l', 0, 3) == 2, 'find with start and end'

# find()/startswith() with i64::MIN start/end — regression: `-index` on i64::MIN used to panic
_I64_MIN = -(2**63)
assert 'hello'.find('h', _I64_MIN) == 0, 'find with i64::MIN start clamps to 0'
assert 'hello'.find('h', 0, _I64_MIN) == -1, 'find with i64::MIN end clamps to 0'
assert 'hello'.startswith('h', _I64_MIN) == True, 'startswith with i64::MIN start clamps to 0'
assert 'hello'.startswith('h', _I64_MIN, _I64_MIN) == False, 'startswith with i64::MIN end'

# rfind()
assert 'hello'.rfind('l') == 3, 'rfind basic'
assert 'hello'.rfind('x') == -1, 'rfind not found'
assert 'hello'.rfind('l', 0, 3) == 2, 'rfind with end'

# index()
assert 'hello'.index('l') == 2, 'index basic'
assert 'hello'.index('ll') == 2, 'index substring'

# rindex()
assert 'hello'.rindex('l') == 3, 'rindex basic'

# count()
assert 'hello'.count('l') == 2, 'count basic'
assert 'hello'.count('ll') == 1, 'count substring'
assert 'hello'.count('x') == 0, 'count not found'
assert 'hello'.count('') == 6, 'count empty string'
assert 'aaa'.count('a') == 3, 'count repeated'

# startswith()
assert 'hello'.startswith('he') == True, 'startswith basic'
assert 'hello'.startswith('lo') == False, 'startswith false'
assert 'hello'.startswith('') == True, 'startswith empty'
assert 'hello'.startswith('ell', 1) == True, 'startswith with start'

# endswith()
assert 'hello'.endswith('lo') == True, 'endswith basic'
assert 'hello'.endswith('he') == False, 'endswith false'
assert 'hello'.endswith('') == True, 'endswith empty'
assert 'hello'.endswith('ell', 0, 4) == True, 'endswith with end'

# === Phase 4: Strip/trim methods ===

# strip()
assert '  hello  '.strip() == 'hello', 'strip whitespace'
assert 'xxhelloxx'.strip('x') == 'hello', 'strip chars'
assert 'hello'.strip() == 'hello', 'strip nothing'
assert ''.strip() == '', 'strip empty'
assert '   '.strip() == '', 'strip only whitespace'

# lstrip()
assert '  hello  '.lstrip() == 'hello  ', 'lstrip whitespace'
assert 'xxhello'.lstrip('x') == 'hello', 'lstrip chars'
assert 'hello'.lstrip() == 'hello', 'lstrip nothing'

# rstrip()
assert '  hello  '.rstrip() == '  hello', 'rstrip whitespace'
assert 'helloxx'.rstrip('x') == 'hello', 'rstrip chars'
assert 'hello'.rstrip() == 'hello', 'rstrip nothing'

# removeprefix()
assert 'hello world'.removeprefix('hello ') == 'world', 'removeprefix basic'
assert 'hello world'.removeprefix('world') == 'hello world', 'removeprefix not found'
assert 'hello'.removeprefix('') == 'hello', 'removeprefix empty'

# removesuffix()
assert 'hello world'.removesuffix(' world') == 'hello', 'removesuffix basic'
assert 'hello world'.removesuffix('hello') == 'hello world', 'removesuffix not found'
assert 'hello'.removesuffix('') == 'hello', 'removesuffix empty'

# === Phase 5: Split methods ===

# split()
assert 'a b c'.split() == ['a', 'b', 'c'], 'split whitespace'
assert 'a,b,c'.split(',') == ['a', 'b', 'c'], 'split comma'
assert 'a,b,c'.split(',', 1) == ['a', 'b,c'], 'split maxsplit'
assert '  a  b  '.split() == ['a', 'b'], 'split multiple spaces'
assert 'hello'.split('x') == ['hello'], 'split not found'

# rsplit()
assert 'a b c'.rsplit() == ['a', 'b', 'c'], 'rsplit whitespace'
assert 'a,b,c'.rsplit(',') == ['a', 'b', 'c'], 'rsplit comma'
assert 'a,b,c'.rsplit(',', 1) == ['a,b', 'c'], 'rsplit maxsplit'
# Multi-byte whitespace must not panic on UTF-8 boundary (U+00A0, U+3000).
assert 'hello world'.rsplit(maxsplit=1) == ['hello', 'world'], 'rsplit maxsplit nbsp'
assert 'a　b　c'.rsplit(maxsplit=1) == ['a　b', 'c'], 'rsplit maxsplit ideographic space'
assert 'a b c'.rsplit(maxsplit=2) == ['a', 'b', 'c'], 'rsplit maxsplit=2 nbsp'
assert 'a b c'.rsplit(maxsplit=0) == ['a b c'], 'rsplit maxsplit=0 does no splits'
# Runs of whitespace count as one separator.
assert 'a  b'.rsplit(maxsplit=2) == ['a', 'b'], 'rsplit consecutive ascii whitespace'
assert 'a\xa0\xa0b'.rsplit(maxsplit=2) == ['a', 'b'], 'rsplit consecutive nbsp'
assert '  a  b  '.rsplit(maxsplit=1) == ['  a', 'b'], 'rsplit trailing whitespace trimmed'

# splitlines()
assert 'a\nb\nc'.splitlines() == ['a', 'b', 'c'], 'splitlines basic'
assert 'a\nb\nc'.splitlines(True) == ['a\n', 'b\n', 'c'], 'splitlines keepends'
assert 'a\r\nb'.splitlines() == ['a', 'b'], 'splitlines crlf'
assert ''.splitlines() == [], 'splitlines empty'

# partition()
assert 'hello world'.partition(' ') == ('hello', ' ', 'world'), 'partition basic'
assert 'hello'.partition('x') == ('hello', '', ''), 'partition not found'
assert 'hello world test'.partition(' ') == ('hello', ' ', 'world test'), 'partition first'

# rpartition()
assert 'hello world'.rpartition(' ') == ('hello', ' ', 'world'), 'rpartition basic'
assert 'hello'.rpartition('x') == ('', '', 'hello'), 'rpartition not found'
assert 'hello world test'.rpartition(' ') == ('hello world', ' ', 'test'), 'rpartition last'

# === Phase 6: Replace/modify methods ===

# replace()
assert 'hello'.replace('l', 'L') == 'heLLo', 'replace basic'
assert 'hello'.replace('l', 'L', 1) == 'heLlo', 'replace count'
assert 'hello'.replace('x', 'y') == 'hello', 'replace not found'
assert 'aaa'.replace('a', 'b') == 'bbb', 'replace all'
assert ''.replace('a', 'b') == '', 'replace empty'

# center()
assert 'hi'.center(6) == '  hi  ', 'center basic'
assert 'hi'.center(6, '-') == '--hi--', 'center fillchar'
assert 'hi'.center(2) == 'hi', 'center no padding'
assert 'hi'.center(1) == 'hi', 'center smaller'

# ljust()
assert 'hi'.ljust(6) == 'hi    ', 'ljust basic'
assert 'hi'.ljust(6, '-') == 'hi----', 'ljust fillchar'
assert 'hi'.ljust(2) == 'hi', 'ljust no padding'

# rjust()
assert 'hi'.rjust(6) == '    hi', 'rjust basic'
assert 'hi'.rjust(6, '-') == '----hi', 'rjust fillchar'
assert 'hi'.rjust(2) == 'hi', 'rjust no padding'

# zfill()
assert '42'.zfill(5) == '00042', 'zfill basic'
assert '-42'.zfill(5) == '-0042', 'zfill negative'
assert '+42'.zfill(5) == '+0042', 'zfill positive'
assert '42'.zfill(2) == '42', 'zfill no padding'
assert ''.zfill(3) == '000', 'zfill empty'

# === Phase 7: Additional tests for Python compatibility ===

# startswith/endswith with tuple
assert 'hello'.startswith(('he', 'lo')) == True, 'startswith tuple first match'
assert 'hello'.startswith(('lo', 'he')) == True, 'startswith tuple second match'
assert 'hello'.startswith(('x', 'y')) == False, 'startswith tuple no match'
assert 'hello'.endswith(('he', 'lo')) == True, 'endswith tuple first match'
assert 'hello'.endswith(('lo', 'he')) == True, 'endswith tuple second match'
assert 'hello'.endswith(('x', 'y')) == False, 'endswith tuple no match'
assert 'hello'.startswith(('ell',), 1) == True, 'startswith tuple with start'

# find/rfind/index/rindex/count with None as start/end
assert 'hello'.find('l', None) == 2, 'find with None start'
assert 'hello'.find('l', None, None) == 2, 'find with None start and end'
assert 'hello'.find('l', 0, None) == 2, 'find with None end'
assert 'hello'.rfind('l', None, None) == 3, 'rfind with None start and end'
assert 'hello'.count('l', None, None) == 2, 'count with None start and end'
assert 'hello'.startswith('he', None) == True, 'startswith with None start'
assert 'hello'.endswith('lo', None, None) == True, 'endswith with None start and end'

# strip with None
assert '  hello  '.strip(None) == 'hello', 'strip None same as no arg'
assert '  hello  '.lstrip(None) == 'hello  ', 'lstrip None same as no arg'
assert '  hello  '.rstrip(None) == '  hello', 'rstrip None same as no arg'

# === Phase 8: Keyword argument tests ===

# split with keyword args
assert 'a,b,c'.split(sep=',') == ['a', 'b', 'c'], 'split sep kwarg'
assert 'a,b,c'.split(',', maxsplit=1) == ['a', 'b,c'], 'split maxsplit kwarg'
assert 'a,b,c'.split(sep=',', maxsplit=1) == ['a', 'b,c'], 'split both kwargs'

# rsplit with keyword args
assert 'a,b,c'.rsplit(sep=',') == ['a', 'b', 'c'], 'rsplit sep kwarg'
assert 'a,b,c'.rsplit(',', maxsplit=1) == ['a,b', 'c'], 'rsplit maxsplit kwarg'
assert 'a,b,c'.rsplit(sep=',', maxsplit=1) == ['a,b', 'c'], 'rsplit both kwargs'

# splitlines with keyword args
assert 'a\nb\nc'.splitlines(keepends=True) == ['a\n', 'b\n', 'c'], 'splitlines keepends kwarg'
assert 'a\nb\nc'.splitlines(keepends=False) == ['a', 'b', 'c'], 'splitlines keepends=False'

# replace with keyword args
assert 'aaa'.replace('a', 'b', count=2) == 'bba', 'replace count kwarg'

# === Phase 9: Additional methods ===

# encode()
assert 'hello'.encode() == b'hello', 'encode default'
assert 'hello'.encode('utf-8') == b'hello', 'encode utf-8'
assert 'hello'.encode('utf8') == b'hello', 'encode utf8 alias'
assert 'hello'.encode('utf_8') == b'hello', 'encode utf_8 alias'
assert 'hello'.encode('UTF-8') == b'hello', 'encode UTF-8 case insensitive'
assert ''.encode() == b'', 'encode empty'
assert 'hello'.encode('utf-8', 'strict') == b'hello', 'encode with errors'

# === encode() with the 'ascii' codec ===
assert 'hello'.encode('ascii') == b'hello', 'encode plain ascii'
assert 'hello'.encode('us-ascii') == b'hello', 'encode us-ascii alias'
assert 'hello'.encode('us_ascii') == b'hello', 'encode us_ascii (underscore) alias'
assert 'hello'.encode('US_ASCII') == b'hello', 'encode US_ASCII case insensitive underscore alias'
assert 'hello'.encode('ASCII') == b'hello', 'encode ASCII case insensitive'
assert 'héllo wörld ⚡'.encode('ascii', 'ignore') == b'hllo wrld ', 'encode ascii ignore drops non-ascii chars'
assert 'héllo wörld ⚡'.encode('ascii', 'replace') == b'h?llo w?rld ?', 'encode ascii replace uses ?'
assert 'héllo wörld ⚡'.encode('ascii', 'backslashreplace') == b'h\\xe9llo w\\xf6rld \\u26a1', (
    'encode ascii backslashreplace escapes non-ascii chars'
)
# Non-BMP characters (> U+FFFF) escape via the \Uxxxxxxxx form, not \uxxxx.
assert 'a\U0001f600b'.encode('ascii', 'backslashreplace') == b'a\\U0001f600b', (
    'encode ascii backslashreplace escapes non-BMP chars with \\U'
)
# The 'ignore' handler round-trips through decode('ascii') since only ASCII bytes remain.
assert 'café — 日本語 test'.encode('ascii', 'ignore').decode('ascii') == 'caf   test', (
    'encode ignore then decode ascii strips non-ascii characters'
)

# strict (the default) raises UnicodeEncodeError, a ValueError subclass, with CPython's exact wording.
try:
    'héllo'.encode('ascii')
    assert False, 'encode ascii of non-ascii string should error'
except ValueError as e:
    assert isinstance(e, UnicodeEncodeError), 'UnicodeEncodeError should be a ValueError subclass'
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
assert 'héllo ⚡'.encode('ascii', 'xmlcharrefreplace') == b'h&#233;llo &#9889;', (
    'encode ascii xmlcharrefreplace uses decimal character references'
)
assert 'a\U0001f600b'.encode('ascii', 'xmlcharrefreplace') == b'a&#128512;b', (
    'encode ascii xmlcharrefreplace handles non-BMP chars'
)
# namereplace substitutes \N{...} escapes, falling back to backslash escapes
# for characters with no Unicode name (e.g. C1 controls).
assert (
    'héllo ⚡'.encode('ascii', 'namereplace') == b'h\\N{LATIN SMALL LETTER E WITH ACUTE}llo \\N{HIGH VOLTAGE SIGN}'
), 'encode ascii namereplace uses unicode name escapes'
assert '一'.encode('ascii', 'namereplace') == b'\\N{CJK UNIFIED IDEOGRAPH-4E00}', (
    'encode ascii namereplace handles algorithmic CJK names'
)
assert 'a\x80\x9fb'.encode('ascii', 'namereplace') == b'a\\x80\\x9fb', (
    'encode ascii namereplace falls back to backslash escapes for unnamed chars'
)
# surrogateescape/surrogatepass only special-case lone surrogates, which a
# valid str can never contain here, so they re-raise exactly like strict.
assert 'hello'.encode('ascii', 'surrogateescape') == b'hello', 'unused surrogateescape handler'
assert 'hello'.encode('ascii', 'surrogatepass') == b'hello', 'unused surrogatepass handler'
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
assert 'hello'.encode('ascii', 'bogus') == b'hello', 'unused error handler name is never validated'
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
assert 'hello'.isidentifier() == True, 'isidentifier basic'
assert '_hello'.isidentifier() == True, 'isidentifier underscore'
assert '__init__'.isidentifier() == True, 'isidentifier dunder'
assert 'hello123'.isidentifier() == True, 'isidentifier with digits'
assert ''.isidentifier() == False, 'isidentifier empty'
assert '123hello'.isidentifier() == False, 'isidentifier digit start'
assert 'hello world'.isidentifier() == False, 'isidentifier with space'
assert 'hello-world'.isidentifier() == False, 'isidentifier with dash'
assert 'class'.isidentifier() == True, 'isidentifier keyword'  # isidentifier doesn't check keywords

# istitle()
assert 'Hello World'.istitle() == True, 'istitle basic'
assert 'Hello'.istitle() == True, 'istitle single word'
assert 'HELLO'.istitle() == False, 'istitle all upper'
assert 'hello'.istitle() == False, 'istitle all lower'
assert ''.istitle() == False, 'istitle empty'
assert 'Hello world'.istitle() == False, 'istitle lowercase word'
assert '123'.istitle() == False, 'istitle numbers only'
assert 'Hello 123 World'.istitle() == True, 'istitle with numbers'
assert "They'Re".istitle() == True, 'istitle apostrophe'

# === Phase 10: Unicode support for is* methods ===

# isdecimal with Unicode decimal digits
assert '٠١٢٣٤٥٦٧٨٩'.isdecimal() == True, 'isdecimal Arabic-Indic'
assert '０１２３４５６７８９'.isdecimal() == True, 'isdecimal Fullwidth'
assert '०१२३४५६७८९'.isdecimal() == True, 'isdecimal Devanagari'
assert '²'.isdecimal() == False, 'isdecimal superscript not decimal'
assert '½'.isdecimal() == False, 'isdecimal fraction not decimal'

# isdigit with superscripts and subscripts
assert '²³'.isdigit() == True, 'isdigit superscripts'
assert '₀₁₂₃₄₅₆₇₈₉'.isdigit() == True, 'isdigit subscripts'
assert '0123456789'.isdigit() == True, 'isdigit ASCII'
assert '٠١٢٣٤٥٦٧٨٩'.isdigit() == True, 'isdigit Arabic-Indic'
assert '½'.isdigit() == False, 'isdigit fraction not digit'

# isnumeric with fractions and other numerics
assert '½'.isnumeric() == True, 'isnumeric fraction'
assert '²'.isnumeric() == True, 'isnumeric superscript'
assert '٠١٢٣٤٥٦٧٨٩'.isnumeric() == True, 'isnumeric Arabic-Indic'
assert '0123456789'.isnumeric() == True, 'isnumeric ASCII'

# === Phase 11: expandtabs ===

# expandtabs() default tabsize=8
assert '\thello'.expandtabs() == '        hello', 'expandtabs default'
assert ''.expandtabs() == '', 'expandtabs empty'
assert 'no tabs here'.expandtabs() == 'no tabs here', 'expandtabs no tabs'

# expandtabs() with explicit tabsize
assert '\thello'.expandtabs(4) == '    hello', 'expandtabs tabsize=4'
assert '\thello'.expandtabs(8) == '        hello', 'expandtabs tabsize=8 explicit'
assert '\thello'.expandtabs(1) == ' hello', 'expandtabs tabsize=1'

# expandtabs() column tracking (tabs align to next tabstop)
assert 'a\tb'.expandtabs() == 'a       b', 'expandtabs column align'
assert 'ab\tcd'.expandtabs() == 'ab      cd', 'expandtabs 2 chars then tab'
assert 'abcdefg\th'.expandtabs() == 'abcdefg h', 'expandtabs 7 chars then tab'
assert 'abcdefgh\ti'.expandtabs() == 'abcdefgh        i', 'expandtabs 8 chars then tab'
assert 'a\tb\tc'.expandtabs(4) == 'a   b   c', 'expandtabs multiple tabs tabsize=4'

# expandtabs() with tabsize=0 (tabs become nothing)
assert '\thello'.expandtabs(0) == 'hello', 'expandtabs tabsize=0'
assert 'a\tb\tc'.expandtabs(0) == 'abc', 'expandtabs tabsize=0 multiple'

# expandtabs() with negative tabsize (treated as 0)
assert '\thello'.expandtabs(-1) == 'hello', 'expandtabs negative tabsize'

# expandtabs() with newlines resetting column
assert 'a\tb\nc\td'.expandtabs(4) == 'a   b\nc   d', 'expandtabs newline resets column'
assert '\t\n\t'.expandtabs(4) == '    \n    ', 'expandtabs tab newline tab'
assert 'a\tb\rc\td'.expandtabs(4) == 'a   b\rc   d', 'expandtabs carriage return resets column'

# expandtabs() with keyword argument
assert '\thello'.expandtabs(tabsize=4) == '    hello', 'expandtabs tabsize kwarg'

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
