from collections import deque


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
        return 7


# === Conversions ===
assert '%s' % 'x' == 'x'
assert '%s %s' % ('a', 'b') == 'a b'
assert '%s%s%s' % ('a', 'b', 'c') == 'abc'
assert '%r' % 'x' == "'x'"
assert '%a' % 'é' == "'\\xe9'"
assert '%d' % 3.9 == '3'
assert '%d' % -3.9 == '-3'
assert '%d' % -0.5 == '0'
assert '%i' % 5 == '5'
assert '%u' % 5 == '5'
assert '%x' % 255 == 'ff'
assert '%X' % 255 == 'FF'
assert '%o' % 8 == '10'
assert '%x' % -255 == '-ff'
assert '%x' % 0 == '0'
assert '%e' % 1234.5 == '1.234500e+03'
assert '%E' % 1234.5 == '1.234500E+03'
assert '%f' % 3.14159 == '3.141590'
assert '%F' % float('inf') == 'INF'
assert '%g' % 1e-5 == '1e-05'
assert '%g' % 0.0001 == '0.0001'
assert '%g' % 100000.0 == '100000'
assert '%g' % 1000000.0 == '1e+06'
assert '%g' % 123456789.0 == '1.23457e+08'
assert '%G' % 1e20 == '1E+20'
assert '%G' % float('nan') == 'NAN'
assert '%E' % float('-inf') == '-INF'
assert '%c' % 65 == 'A'
assert '%c' % 'A' == 'A'
assert '%c' % 'é' == 'é'
assert '%c' % 0x1F389 == '🎉'
assert '%c' % 0 == '\x00'
assert '%c' % True == '\x01'
assert '%c' % Indexable() == '\x07'
assert '%%' % () == '%'
assert '%d%%' % 5 == '5%'
assert '%s%%%s' % ('a', 'b') == 'a%b'
assert '%%%s' % 'x' == '%x'
assert 'abc' % () == 'abc'
assert '%s' % ((),) == '()'
assert '%s' % ((1, 2),) == '(1, 2)'
assert '%s' % [1, 2] == '[1, 2]'
assert '%s' % [('a', 'b')] == "[('a', 'b')]"
assert '%s' % {'a': 1} == "{'a': 1}"
assert '%s' % ('a',) == 'a'
assert '%s' % None == 'None'
assert '%s' % True == 'True'
assert '%s' % b'x' == "b'x'"
assert '%r' % b'x' == "b'x'"
assert '%s' % 1.0 == '1.0'
assert '%s' % 1e20 == '1e+20'
assert '%s' % -0.0 == '-0.0'
assert '%r' % 1.5 == '1.5'
assert '%s' % Shown() == 'shown-str'
assert '%r' % Shown() == 'shown-repr'
assert '%s' % 'é' == 'é'

# === Length modifiers are accepted and ignored ===
assert '%ld' % 5 == '5'
assert '%hd' % 5 == '5'
assert '%Ld' % 5 == '5'
assert '%lf' % 1.5 == '1.500000'
assert '%lx' % 255 == 'ff'

# === Flags, width and precision on integers ===
assert '%5d|' % 3 == '    3|'
assert '%-5d|' % 3 == '3    |'
assert '%05d' % -3 == '-0003'
assert '%+d' % 3 == '+3'
assert '% d' % 3 == ' 3'
assert '%+d' % 0 == '+0'
assert '% d' % 0 == ' 0'
assert '%-05d|' % 3 == '3    |'
assert '%0-5d|' % 3 == '3    |'
assert '%-+5d|' % 3 == '+3   |'
assert '%+-5d|' % 3 == '+3   |'
assert '%- d' % 3 == ' 3'
assert '%+ d' % 3 == '+3'
assert '% +d' % 3 == '+3'
assert '% 5d' % 3 == '    3'
assert '%+5d' % -3 == '   -3'
assert '% 05d' % 3 == ' 0003'
assert '%.2d' % 3 == '03'
assert '%.3d' % -5 == '-005'
assert '%8.3d|' % 5 == '     005|'
assert '%08.3d' % 5 == '00000005'
assert '%-8.3d|' % 5 == '005     |'
assert '%5.3d' % -7 == ' -007'
assert '%-5.3d|' % -7 == '-007 |'
assert '%05.3d' % -7 == '-0007'
assert '%+.3d' % 7 == '+007'
assert '%+.3d' % -7 == '-007'
assert '%+05d' % 7 == '+0007'
assert '%+05.3d' % 7 == '+0007'
assert '%.0d' % 0 == '0'
assert '%.0d' % 5 == '5'
assert '%.3d' % 0 == '000'
assert '%3.0d|' % 0 == '  0|'
assert '%03.0d|' % 0 == '000|'
assert '%+.0d' % 0 == '+0'
assert '%.d' % 5 == '5'
assert '%.10d' % 1 == '0000000001'
assert '%#d' % 5 == '5'
assert '%#x' % 255 == '0xff'
assert '%#X' % 255 == '0XFF'
assert '%#o' % 8 == '0o10'
assert '%#x' % -255 == '-0xff'
assert '%#x' % 0 == '0x0'
assert '%#o' % 0 == '0o0'
assert '%#.0x' % 0 == '0x0'
assert '%.x' % 0 == '0'
assert '%.4x' % 255 == '00ff'
assert '%#.4x' % 255 == '0x00ff'
assert '%.5x' % -255 == '-000ff'
assert '%0.5x' % 255 == '000ff'
assert '%.5o' % 8 == '00010'
assert '%#.5o' % 8 == '0o00010'
assert '%.5X' % 255 == '000FF'
assert '%+x' % 255 == '+ff'
assert '% x' % 255 == ' ff'
assert '%+o' % 8 == '+10'
assert '%+#x' % 255 == '+0xff'
assert '%+#05x' % 1 == '+0x01'
assert '%#5x' % 255 == ' 0xff'
assert '%#05x' % 255 == '0x0ff'
assert '%#-5x|' % 255 == '0xff |'
assert '%#08.3x' % 1 == '0x000001'
assert '%#-8.3x|' % 1 == '0x001   |'
assert '%5d|' % True == '    1|'
assert '%d' % True == '1'
assert '%x' % True == '1'
assert '%d' % Indexable() == '7'
assert '%x' % Indexable() == '7'

# === Big integers ===
assert '%d' % 10**30 == '1000000000000000000000000000000'
assert '%x' % 10**30 == 'c9f2c9cd04674edea40000000'
assert '%X' % 2**64 == '10000000000000000'
assert '%x' % 2**63 == '8000000000000000'
assert '%#o' % -(10**30) == '-0o1447626234640431647336510000000000'
assert '%o' % -(2**70) == '-200000000000000000000000'
assert '%d' % 2**63 == '9223372036854775808'
assert '%d' % (2**63 - 1) == '9223372036854775807'
assert '%d' % -(2**63) == '-9223372036854775808'
assert '%.3d' % 10**30 == '1000000000000000000000000000000'
assert '%+d' % 10**30 == '+1000000000000000000000000000000'
assert '%020d' % -(10**30) == '-1000000000000000000000000000000'
assert '%d' % 1e20 == '100000000000000000000'
assert '%d' % 3.5e15 == '3500000000000000'
assert '%e' % 10**30 == '1.000000e+30'
assert '%g' % 10**30 == '1e+30'
assert '%.2f' % 10**30 == '1000000000000000019884624838656.00'
assert '%.3f' % 2**63 == '9223372036854775808.000'

# === Flags, width and precision on floats ===
assert '%.2f' % 3 == '3.00'
assert '%e' % 5 == '5.000000e+00'
assert '%g' % 5 == '5'
assert '%f' % True == '1.000000'
assert '%f' % Indexable() == '7.000000'
assert '%.0f' % 2.5 == '2'
assert '%#.0f' % 2.0 == '2.'
assert '%#.0f' % 0.0 == '0.'
assert '%+.1e' % 12345.678 == '+1.2e+04'
assert '% .2f' % 3.14159 == ' 3.14'
assert '%08.2f' % -3.14159 == '-0003.14'
assert '%-8.2f|' % 3.14159 == '3.14    |'
assert '%010.3f' % 3.14159 == '000003.142'
assert '%-010.3f|' % 3.14159 == '3.142     |'
assert '%+010.2f' % 3.5 == '+000003.50'
assert '%-+010.2f|' % 3.5 == '+3.50     |'
assert '%f' % float('nan') == 'nan'
assert '%05f' % float('inf') == '00inf'
assert '%010f' % float('-inf') == '-000000inf'
assert '%010.2f' % float('nan') == '0000000nan'
assert '%+f' % float('inf') == '+inf'
assert '% f' % float('nan') == ' nan'
assert '%.2f' % float('-inf') == '-inf'
assert '%.20f' % 1.0 == '1.00000000000000000000'
assert '%f' % 1e50 == '100000000000000007629769841091887003294964970946560.000000'
assert '%.0f' % 1e22 == '10000000000000000000000'
assert '%f' % 1e-7 == '0.000000'
assert '%f' % -0.0 == '-0.000000'
assert '%.0e' % 12345.678 == '1e+04'
assert '%#.0e' % 12345.678 == '1.e+04'
assert '%.0e' % 0.5 == '5e-01'
assert '%e' % 0.0 == '0.000000e+00'
assert '%#g' % 1.0 == '1.00000'
assert '%#.3g' % 1.0 == '1.00'
assert '%#.0g' % 5.0 == '5.'
assert '%.3g' % 3.14159 == '3.14'
assert '%.0g' % 0.0 == '0'
assert '%G' % 1e-10 == '1E-10'

# === Flags, width and precision on text ===
assert '%05s' % 'a' == '    a'
assert '%+s' % 'a' == 'a'
assert '%#s' % 'a' == 'a'
assert '%5.2s|' % 'abcdef' == '   ab|'
assert '%.3s' % 'abcdef' == 'abc'
assert '%.s' % 'abc' == ''
assert '%.0s|' % 'abc' == '|'
assert '%5.0s|' % 'abc' == '     |'
assert '%.3r' % 'abcdef' == "'ab"
assert '%.0r' % 'abc' == ''
assert '%.2a' % 'éab' == "'\\"
assert '%5.1r|' % 'abc' == "    '|"
assert '%10r|' % 'a' == "       'a'|"
assert '%-10s|' % 'a' == 'a         |'
assert '%10s|' % Shown() == ' shown-str|'
assert '%5s|' % 'é' == '    é|'
assert '%.1s' % 'é🎉' == 'é'
assert '%.2s' % 12345 == '12'
assert '%5.1s|' % 12345 == '    1|'
assert '%5c|' % 65 == '    A|'
assert '%-5c|' % 'A' == 'A    |'
assert '%05c' % 65 == '    A'
assert '%.3c' % 65 == 'A'
assert '%#c' % 65 == 'A'
assert '%+c' % 65 == 'A'

# === Width and precision from arguments ===
assert '%*d|' % (5, 3) == '    3|'
assert '%-*d|' % (5, 3) == '3    |'
assert '%*d|' % (-5, 3) == '3    |'
assert '%*d' % (True, 3) == '3'
assert '%0*d' % (5, 3) == '00003'
assert '%0*d' % (-5, 3) == '3    '
assert '%.*d' % (5, 3) == '00003'
assert '%.*d' % (-2, 3) == '3'
assert '%.*s' % (-1, 'abc') == ''
assert '%.*f' % (-1, 1.5) == '2'
assert '%.*f' % (2, 3.14159) == '3.14'
assert '%*.*f|' % (8, 2, 3.14159) == '    3.14|'
assert '%*s|' % (-8, 'a') == 'a       |'
assert '%-*s|' % (-8, 'a') == 'a       |'
assert '%-*.*s|' % (6, 2, 'abcdef') == 'ab    |'

# === Mapping arguments ===
assert '%(a)s' % {'a': 1} == '1'
assert '%(a)s %(b)d' % {'a': 'x', 'b': 2.5} == 'x 2'
assert '%(a)s %(a)s' % {'a': 1} == '1 1'
assert '%()s' % {'': 'e'} == 'e'
assert '%(a)5.1f|' % {'a': 3.14159} == '  3.1|'
assert '%((a))s' % {'(a)': 1} == '1'
assert '%s %(a)s' % {'a': 1} == "{'a': 1} 1"
assert 'abc' % {} == 'abc'
assert 'abc' % {'a': 1} == 'abc'
assert 'abc' % [] == 'abc'
assert 'abc' % b'x' == 'abc'
assert 'abc' % range(2) == 'abc'

# === Argument count errors ===
assert capture_error('%s %s', 'a') == ('TypeError', 'not enough arguments for format string')
assert capture_error('%s %s', ('a',)) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%s', ()) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%s %s', {'a': 1}) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%(a)s %s', {'a': 1}) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%(a)*s', {'a': 1}) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%(n).*f', {'n': 2}) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%*d', (5,)) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%.*f', (2,)) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%5%', ()) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%*%', (5,)) == ('TypeError', 'not enough arguments for format string')
assert capture_error('%s', ('a', 'b')) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('%d %s', (1, 'x', 'y')) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('%%', 5) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('%%s', 'x') == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('abc', 5) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('abc', 'x') == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('abc', None) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('abc', 1.5) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('abc', set()) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('abc', deque()) == ('TypeError', 'not all arguments converted during string formatting')
assert capture_error('abc', Shown()) == ('TypeError', 'not all arguments converted during string formatting')

# === Mapping errors ===
assert capture_error('%(a)s', {'b': 1}) == ('KeyError', "'a'")
assert capture_error('%(a)', {}) == ('KeyError', "'a'")
assert capture_error('%(a)s', (1,)) == ('TypeError', 'format requires a mapping')
assert capture_error('%(a)s', 'x') == ('TypeError', 'format requires a mapping')
assert capture_error('%(a)s', 5) == ('TypeError', 'format requires a mapping')
assert capture_error('%(a)s', 3.5) == ('TypeError', 'format requires a mapping')
assert capture_error('%(a)s', Shown()) == ('TypeError', 'format requires a mapping')
assert capture_error('%(a)s', [1]) == ('TypeError', 'list indices must be integers or slices, not str')
assert capture_error('%(a', {'a': 1}) == ('ValueError', 'incomplete format key')
assert capture_error('%(', {}) == ('ValueError', 'incomplete format key')
assert capture_error('%(a)', {'a': 1}) == ('ValueError', 'incomplete format')
assert capture_error('%(a)5', {'a': 1}) == ('ValueError', 'incomplete format')
assert capture_error('%(a)%', {'a': 1}) == ('ValueError', "unsupported format character '%' (0x25) at index 4")
assert capture_error('%(a).*s', {'a': 'xyz'}) == ('TypeError', '* wants int')

# === Malformed directives ===
assert capture_error('%', ()) == ('ValueError', 'incomplete format')
assert capture_error('%s %', 'a') == ('ValueError', 'incomplete format')
assert capture_error('%s%', 'x') == ('ValueError', 'incomplete format')
assert capture_error('%%%', 1) == ('ValueError', 'incomplete format')
assert capture_error('%-', 1) == ('ValueError', 'incomplete format')
assert capture_error('%5', 1) == ('ValueError', 'incomplete format')
assert capture_error('%5.', 1) == ('ValueError', 'incomplete format')
assert capture_error('%.', 1) == ('ValueError', 'incomplete format')
assert capture_error('%l', 1) == ('ValueError', 'incomplete format')
assert capture_error('%*', (1,)) == ('ValueError', 'incomplete format')
assert capture_error('%.*', (1,)) == ('ValueError', 'incomplete format')
assert capture_error('%z', 1) == ('ValueError', "unsupported format character 'z' (0x7a) at index 1")
assert capture_error('%y', 1) == ('ValueError', "unsupported format character 'y' (0x79) at index 1")
assert capture_error('%b', 5) == ('ValueError', "unsupported format character 'b' (0x62) at index 1")
assert capture_error('%n', 5) == ('ValueError', "unsupported format character 'n' (0x6e) at index 1")
assert capture_error('%ll', 1) == ('ValueError', "unsupported format character 'l' (0x6c) at index 2")
assert capture_error('%hhd', 5) == ('ValueError', "unsupported format character 'h' (0x68) at index 2")
assert capture_error('%5%', 5) == ('ValueError', "unsupported format character '%' (0x25) at index 2")
assert capture_error('%-5%|', 5) == ('ValueError', "unsupported format character '%' (0x25) at index 3")
assert capture_error('%é', 1) == ('ValueError', "unsupported format character '?' (0xe9) at index 1")
assert capture_error('%☃', 1) == ('ValueError', "unsupported format character '?' (0x2603) at index 1")
assert capture_error('é%z', 1) == ('ValueError', "unsupported format character 'z' (0x7a) at index 2")
assert capture_error('%99999999999999999999d', 1) == ('ValueError', 'width too big')
assert capture_error('%.99999999999999999999d', 1) == ('ValueError', 'precision too big')
assert capture_error('%*d', ('a', 3)) == ('TypeError', '* wants int')
assert capture_error('%*d', (10**30, 3)) == ('OverflowError', 'Python int too large to convert to C ssize_t')
assert capture_error('%.*s', (10**30, 'x')) == ('OverflowError', 'Python int too large to convert to C int')
assert capture_error('%.*s', (2**31, 'x')) == ('OverflowError', 'Python int too large to convert to C int')
assert '%.*s' % (2**31 - 1, 'x') == 'x'
assert capture_error('%.3000000000d', 1) == ('ValueError', 'precision too big')

# === Operand type errors ===
assert capture_error('%d', 'x') == ('TypeError', '%d format: a real number is required, not str')
assert capture_error('%i', 'x') == ('TypeError', '%i format: a real number is required, not str')
assert capture_error('%d', None) == ('TypeError', '%d format: a real number is required, not NoneType')
assert capture_error('%d', Shown()) == ('TypeError', '%d format: a real number is required, not Shown')
assert capture_error('%x', 3.5) == ('TypeError', '%x format: an integer is required, not float')
assert capture_error('%o', 3.5) == ('TypeError', '%o format: an integer is required, not float')
assert capture_error('%X', 'x') == ('TypeError', '%X format: an integer is required, not str')
assert capture_error('%f', 'x') == ('TypeError', 'must be real number, not str')
assert capture_error('%f', None) == ('TypeError', 'must be real number, not NoneType')
assert capture_error('%e', [1]) == ('TypeError', 'must be real number, not list')
assert capture_error('%c', 'AB') == ('TypeError', '%c requires an int or a unicode character, not a string of length 2')
assert capture_error('%c', '') == ('TypeError', '%c requires an int or a unicode character, not a string of length 0')
assert capture_error('%c', 3.5) == ('TypeError', '%c requires an int or a unicode character, not float')
assert capture_error('%c', 0x110000) == ('OverflowError', '%c arg not in range(0x110000)')
assert capture_error('%c', -1) == ('OverflowError', '%c arg not in range(0x110000)')
assert capture_error('%c', 2**70) == ('OverflowError', '%c arg not in range(0x110000)')
assert capture_error('%d', float('inf')) == ('OverflowError', 'cannot convert float infinity to integer')
assert capture_error('%d', float('nan')) == ('ValueError', 'cannot convert float NaN to integer')
assert capture_error('%f', 10**400) == ('OverflowError', 'int too large to convert to float')
assert capture_error('%e', 2**1024) == ('OverflowError', 'int too large to convert to float')
assert capture_error('%d', 10**5000) == (
    'ValueError',
    'Exceeds the limit (4300 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit',
)

# === Other operand types stay unsupported ===
assert capture_error(5, 'x') == ('TypeError', "unsupported operand type(s) for %: 'int' and 'str'")
