def capture_error(template, args):
    try:
        template % args
    except Exception as exc:
        return type(exc).__name__, str(exc)
    return None


class Shown:
    def __str__(self):
        return 'shown-str'

    def __repr__(self):
        return 'shown-repr'


class Indexable:
    def __index__(self):
        return 66


# === Conversions ===
assert b'%s' % b'x' == b'x'
assert b'%b' % b'x' == b'x'
assert b'%s %s' % (b'a', b'b') == b'a b'
assert b'%s' % b'' == b''
assert b'%s' % b'\xff\x00' == b'\xff\x00'
assert b'%r' % 'é' == b"'\\xe9'"
assert b'%a' % 'é' == b"'\\xe9'"
assert b'%a' % b'x' == b"b'x'"
assert b'%r' % 1.5 == b'1.5'
assert b'%r' % Shown() == b'shown-repr'
assert b'%a' % Shown() == b'shown-repr'
assert b'%c' % 65 == b'A'
assert b'%c' % b'A' == b'A'
assert b'%c' % 255 == b'\xff'
assert b'%c' % 0 == b'\x00'
assert b'%c' % b'\xff' == b'\xff'
assert b'%c' % True == b'\x01'
assert b'%c' % Indexable() == b'B'
assert b'%d' % 5 == b'5'
assert b'%d' % 3.9 == b'3'
assert b'%d' % -3.9 == b'-3'
assert b'%d' % True == b'1'
assert b'%d' % Indexable() == b'66'
assert b'%i' % 5 == b'5'
assert b'%u' % 5 == b'5'
assert b'%x' % 255 == b'ff'
assert b'%x' % -255 == b'-ff'
assert b'%X' % 255 == b'FF'
assert b'%x' % Indexable() == b'42'
assert b'%o' % 8 == b'10'
assert b'%#x' % 255 == b'0xff'
assert b'%#o' % 8 == b'0o10'
assert b'%e' % 1 == b'1.000000e+00'
assert b'%E' % 1234.5 == b'1.234500E+03'
assert b'%f' % 1.5 == b'1.500000'
assert b'%F' % float('inf') == b'INF'
assert b'%g' % 1e20 == b'1e+20'
assert b'%G' % 1e-10 == b'1E-10'
assert b'%f' % Indexable() == b'66.000000'
assert b'%%' % () == b'%'
assert b'%d%%' % 5 == b'5%'
assert b'%s%%%s' % (b'a', b'b') == b'a%b'
assert b'abc' % () == b'abc'
assert b'%hd' % 5 == b'5'
assert b'%ld' % 5 == b'5'
assert b'%Lf' % 1.5 == b'1.500000'

# === Big integers ===
assert b'%d' % 10**30 == b'1000000000000000000000000000000'
assert b'%x' % 10**30 == b'c9f2c9cd04674edea40000000'
assert b'%+d' % 10**30 == b'+1000000000000000000000000000000'
assert b'%e' % 10**30 == b'1.000000e+30'

# === Flags, width and precision ===
assert b'%5d|' % 3 == b'    3|'
assert b'%-5d|' % 3 == b'3    |'
assert b'%05d' % -3 == b'-0003'
assert b'%+d' % 3 == b'+3'
assert b'% d' % 3 == b' 3'
assert b'%.3d' % 5 == b'005'
assert b'%08.3d' % 5 == b'00000005'
assert b'%#08.3x' % 1 == b'0x000001'
assert b'%08.3f' % 3.14159 == b'0003.142'
assert b'%-8.2f|' % 3.14159 == b'3.14    |'
assert b'%010.2f' % float('nan') == b'0000000nan'
assert b'%#.0f' % 2.0 == b'2.'
assert b'%.2s' % b'abcdef' == b'ab'
assert b'%.1s' % b'\xff\x00' == b'\xff'
assert b'%5.2s|' % b'abcdef' == b'   ab|'
assert b'%-5s|' % b'ab' == b'ab   |'
assert b'%05s' % b'ab' == b'   ab'
assert b'%3s|' % b'\xff' == b'  \xff|'
assert b'%5r|' % 1 == b'    1|'
assert b'%.2r' % 'abc' == b"'a"
assert b'%5c|' % 65 == b'    A|'
assert b'%-5c|' % b'A' == b'A    |'
assert b'%.3c' % 65 == b'A'
assert b'%*d|' % (5, 3) == b'    3|'
assert b'%*d|' % (-5, 3) == b'3    |'
assert b'%.*f' % (2, 3.14159) == b'3.14'
assert b'%.*s' % (2, b'abc') == b'ab'
assert b'%.*s' % (-1, b'abc') == b''
assert b'%.*f' % (-1, 1.5) == b'2'
assert b'%-*.*s|' % (6, 2, b'abcdef') == b'ab    |'

# === Mapping arguments ===
assert b'%(a)s' % {b'a': b'x'} == b'x'
assert b'%(a)s %(b)d' % {b'a': b'x', b'b': 2.5} == b'x 2'
assert b'%(a)5.1f|' % {b'a': 3.14159} == b'  3.1|'
assert capture_error(b'%s %(a)s', {b'a': b'x'}) == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'dict'",
)
assert b'abc' % {} == b'abc'
assert b'abc' % [] == b'abc'
assert b'abc' % range(2) == b'abc'

# === Argument count errors ===
assert capture_error(b'%s %s', b'a') == ('TypeError', 'not enough arguments for format string')
assert capture_error(b'%s', ()) == ('TypeError', 'not enough arguments for format string')
assert capture_error(b'%(a)s %s', {b'a': b'x'}) == ('TypeError', 'not enough arguments for format string')
assert capture_error(b'%5%', ()) == ('TypeError', 'not enough arguments for format string')
assert capture_error(b'%*%', (5,)) == ('TypeError', 'not enough arguments for format string')
assert capture_error(b'%s', (b'a', b'b')) == ('TypeError', 'not all arguments converted during bytes formatting')
assert capture_error(b'%d %s', (1, b'x', b'y')) == ('TypeError', 'not all arguments converted during bytes formatting')
assert capture_error(b'%%', 5) == ('TypeError', 'not all arguments converted during bytes formatting')
assert capture_error(b'abc', 5) == ('TypeError', 'not all arguments converted during bytes formatting')
assert capture_error(b'abc', b'x') == ('TypeError', 'not all arguments converted during bytes formatting')

# === Mapping errors ===
# the key text differs, see limitations/exceptions.md on `KeyError` arguments
missing_key = capture_error(b'%(a)s', {'a': b'x'})
assert missing_key is not None
assert missing_key[0] == 'KeyError'
assert capture_error(b'%(a)s', (1,)) == ('TypeError', 'format requires a mapping')
assert capture_error(b'%(a)s', 5) == ('TypeError', 'format requires a mapping')
assert capture_error(b'%(a)s', [1]) == ('TypeError', 'list indices must be integers or slices, not bytes')
assert capture_error(b'%(a', {}) == ('ValueError', 'incomplete format key')
assert capture_error(b'%(a)', {b'a': 1}) == ('ValueError', 'incomplete format')

# === Malformed directives ===
assert capture_error(b'%', ()) == ('ValueError', 'incomplete format')
assert capture_error(b'%5', 1) == ('ValueError', 'incomplete format')
assert capture_error(b'%z', 1) == ('ValueError', "unsupported format character 'z' (0x7a) at index 1")
assert capture_error(b'%5%', 5) == ('ValueError', "unsupported format character '%' (0x25) at index 2")
assert capture_error(b'%hhd', 5) == ('ValueError', "unsupported format character 'h' (0x68) at index 2")
assert capture_error(b'%ll', 1) == ('ValueError', "unsupported format character 'l' (0x6c) at index 2")
assert capture_error(b'%\xe9', 1) == ('OverflowError', 'character argument not in range(0x110000)')
assert capture_error(b'ab%\xe9', 1) == ('OverflowError', 'character argument not in range(0x110000)')
assert capture_error(b'%\x1f', 1) == ('ValueError', "unsupported format character '\x1f' (0x1f) at index 1")
assert capture_error(b'%\x7f', 1) == ('ValueError', "unsupported format character '\x7f' (0x7f) at index 1")
assert capture_error(b'%.2147483645d', 1) == ('OverflowError', 'precision too large')
assert capture_error(b'%.*x', (2147483647, 1)) == ('OverflowError', 'precision too large')
assert capture_error(b'%99999999999999999999d', 1) == ('ValueError', 'width too big')
assert capture_error(b'%*d', (b'a', 1)) == ('TypeError', '* wants int')
assert capture_error(b'%*d', (10**30, 1)) == ('OverflowError', 'Python int too large to convert to C ssize_t')
assert capture_error(b'%.*s', (10**30, b'x')) == ('OverflowError', 'Python int too large to convert to C int')
assert capture_error(b'%.*s', (2**31, b'x')) == ('OverflowError', 'Python int too large to convert to C int')
assert b'%.*s' % (2**31 - 1, b'x') == b'x'
assert capture_error(b'%.3000000000d', 1) == ('ValueError', 'prec too big')
assert capture_error(b'%.99999999999999999999d', 1) == ('ValueError', 'prec too big')

# === Operand type errors ===
assert capture_error(b'%s', 'x') == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'str'",
)
assert capture_error(b'%b', 'x') == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'str'",
)
assert capture_error(b'%s', 5) == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'int'",
)
assert capture_error(b'%s', None) == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'NoneType'",
)
assert capture_error(b'%s', ['x']) == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'list'",
)
assert capture_error(b'%s', {b'a': b'x'}) == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'dict'",
)
assert capture_error(b'%s', Shown()) == (
    'TypeError',
    "%b requires a bytes-like object, or an object that implements __bytes__, not 'Shown'",
)
assert capture_error(b'%c', 256) == ('OverflowError', '%c arg not in range(256)')
assert capture_error(b'%c', -1) == ('OverflowError', '%c arg not in range(256)')
assert capture_error(b'%c', 2**70) == ('OverflowError', '%c arg not in range(256)')
assert capture_error(b'%c', b'AB') == (
    'TypeError',
    '%c requires an integer in range(256) or a single byte, not a bytes object of length 2',
)
assert capture_error(b'%c', b'') == (
    'TypeError',
    '%c requires an integer in range(256) or a single byte, not a bytes object of length 0',
)
assert capture_error(b'%c', 'A') == ('TypeError', '%c requires an integer in range(256) or a single byte, not str')
assert capture_error(b'%c', 3.5) == ('TypeError', '%c requires an integer in range(256) or a single byte, not float')
assert capture_error(b'%c', None) == (
    'TypeError',
    '%c requires an integer in range(256) or a single byte, not NoneType',
)
assert capture_error(b'%d', 'x') == ('TypeError', '%d format: a real number is required, not str')
assert capture_error(b'%x', 3.5) == ('TypeError', '%x format: an integer is required, not float')
assert capture_error(b'%f', 'x') == ('TypeError', 'float argument required, not str')
assert capture_error(b'%f', None) == ('TypeError', 'float argument required, not NoneType')
assert capture_error(b'%d', float('inf')) == ('OverflowError', 'cannot convert float infinity to integer')
assert capture_error(b'%f', 10**400) == ('TypeError', 'float argument required, not int')
assert capture_error(b'%e', 2**1024) == ('TypeError', 'float argument required, not int')

# === Other operand types stay unsupported ===
assert capture_error(5, b'x') == ('TypeError', "unsupported operand type(s) for %: 'int' and 'bytes'")
