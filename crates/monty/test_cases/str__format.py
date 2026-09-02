import re
from collections import deque
from datetime import datetime, time, timedelta


def capture_error(template, *args, **kwargs):
    try:
        template.format(*args, **kwargs)
    except Exception as exc:
        return type(exc).__name__, str(exc)
    return None


assert '{} {}'.format('one', 'two') == 'one two'
assert '{1} {0}'.format('one', 'two') == 'two one'
assert '{first} {last}'.format(first='Jean-Luc', last='Picard') == 'Jean-Luc Picard'
assert '{a-b}'.format(**{'a-b': 7}) == '7'
unicode_index = '{٠}'
assert unicode_index.format('zero') == 'zero'
mathematical_index = '{𝟘}'
assert mathematical_index.format('zero') == 'zero'
literal_only = 'unchanged'
assert literal_only.format(1, ignored=2) == 'unchanged'


class Record:
    def __init__(self, value):
        self.value = value


assert '{0.value:04d}'.format(Record(1)) == '0001'
assert capture_error('{0.missing}', Record(2001)) == (
    'AttributeError',
    "'Record' object has no attribute 'missing'",
)
person = {'first': 'Jean-Luc', 'last': 'Picard'}
assert '{p[first]} {p[last]}'.format(p=person) == 'Jean-Luc Picard'
assert capture_error('{0[missing]}', person) == ('KeyError', "'missing'")
plant = {'kinds': [{'name': 'oak'}]}
assert '{p[kinds][0][name]}'.format(p=plant) == 'oak'
assert capture_error('{0[2]}', [10, 20]) == ('IndexError', 'list index out of range')
assert '{0[１]}'.format([10, 20]) == '20'
assert '{0[𝟙]}'.format(['zero', 'one']) == 'one'
unusual_item_keys = '{0[a:b]} {0[a!b]} {0[a}b]}'
assert unusual_item_keys.format({'a:b': 1, 'a!b': 2, 'a}b': 3}) == '1 2 3'

native_datetime = datetime(2001, 2, 3, 4, 5)
assert '{0.year}-{0.hour}'.format(native_datetime) == '2001-4'
assert '{0.days}'.format(timedelta(days=2)) == '2'
assert '{0.maxlen}'.format(deque(maxlen=4)) == '4'
native_match = re.match('a', 'abc')
assert '{0.string}'.format(native_match) == 'abc'

nul = chr(0)
assert ('{0!' + nul + '}').format('a') == 'a'
assert ('{0!' + nul + ':04d}').format(1) == '0001'
assert '{0!s} {0!r} {0!a}'.format('räpr') == "räpr 'räpr' 'r\\xe4pr'"
assert '{0!a}'.format('😀') == "'\\U0001f600'"
assert '{0:>10}'.format('test') == '      test'
assert '{0:_<10.5}'.format('xylophone') == 'xylop_____'
assert '{0:+06.2f}'.format(3.14159) == '+03.14'
assert '{:５}'.format(1) == '    1'
assert '{:.２f}'.format(1.25) == '1.25'
assert '{:._}'.format(True) == '1'
assert '{:._}'.format('x') == 'x'
assert capture_error('{:.6_n}', 1.234567) == ('ValueError', "Cannot specify '_' with 'n'.")
assert '{0:.2147483647g}'.format(0.0001) == ('0.000100000000000000004792173602385929598312941379845142364501953125')
assert '{:}'.format(True) == 'True'
assert '{0!r:>6}'.format(123) == '   123'
assert '{0:{align}{width}}'.format('test', align='^', width=10) == '   test   '
assert '{0:{width}.{precision}f}'.format(2.7182, width=5, precision=2) == ' 2.72'
assert '{0:{spec[align]}}'.format('x', spec={'align': '^5'}) == '  x  '
assert '{} {:{}}'.format(1, 2, 3) == '1   2'
dt = datetime(2001, 2, 3, 4, 5)
assert '{0:%Y-%m-%d %H:%M}'.format(dt) == '2001-02-03 04:05'

assert '{{}}'.format() == '{}'
assert '{{{0}}}'.format('x') == '{x}'
assert ''.format() == ''
shared = {'x': 'value'}
assert '{0} {0!r} {0[x]}'.format(shared) == "{'x': 'value'} {'x': 'value'} value"

assert capture_error('{} {0}', 'a', 'b') == (
    'ValueError',
    'cannot switch from automatic field numbering to manual field specification',
)
assert capture_error('{0} {}', 'a', 'b') == (
    'ValueError',
    'cannot switch from manual field specification to automatic field numbering',
)
assert capture_error('{2}', 'a') == (
    'IndexError',
    'Replacement index 2 out of range for positional args tuple',
)
assert capture_error('{missing}') == ('KeyError', "'missing'")
assert capture_error('{') == ('ValueError', "Single '{' encountered in format string")
assert capture_error('}') == ('ValueError', "Single '}' encountered in format string")
assert capture_error('{0!x}', 'a') == ('ValueError', 'Unknown conversion specifier x')
assert capture_error('{0!}}', 'a') == ('ValueError', 'Unknown conversion specifier }')
assert capture_error('{0!}{', 'a') == ('ValueError', "expected ':' after conversion specifier")
assert capture_error('{0! }', 'a') == ('ValueError', 'Unknown conversion specifier \\x20')
assert capture_error('{0!é}', 'a') == ('ValueError', 'Unknown conversion specifier \\xe9')
assert capture_error('{0!😀}', 'a') == ('ValueError', 'Unknown conversion specifier \\x1f600')
assert capture_error('{0!rs}', 'a') == ('ValueError', "expected ':' after conversion specifier")
assert capture_error('{0:=5}', 'a') == ('ValueError', "'=' alignment not allowed in string format specifier")
assert capture_error('{0[foo}', {'foo': 'a'}) == ('ValueError', "expected '}' before end of string")
assert capture_error('{0!', 'a') == (
    'ValueError',
    'end of string while looking for conversion specifier',
)
assert capture_error('{0!s', 'a') == ('ValueError', "unmatched '{' in format spec")
assert capture_error('{0!}', 'a') == ('ValueError', "unmatched '{' in format spec")
assert capture_error('{missing:') == ('ValueError', "unmatched '{' in format spec")
assert capture_error('{missing!rs}') == (
    'ValueError',
    "expected ':' after conversion specifier",
)
assert capture_error('{missing!x}') == ('KeyError', "'missing'")


class BrokenRepr:
    def __repr__(self):
        raise RuntimeError('repr failed')


assert capture_error('{0!r:{missing}}', BrokenRepr()) == ('RuntimeError', 'repr failed')
assert capture_error('{0!r:q}', BrokenRepr()) == ('RuntimeError', 'repr failed')
assert capture_error('{0:.2q}', 1) == (
    'ValueError',
    "Unknown format code 'q' for object of type 'int'",
)
assert capture_error('{0:.}', 1) == ('ValueError', 'Format specifier missing precision')
assert capture_error('{0{}}', 'a') == ('ValueError', "unexpected '{' in field name")
assert capture_error('{0..x}', 'a') == ('ValueError', 'Empty attribute in format string')
assert capture_error('{0[]}', {'': 1}) == ('ValueError', 'Empty attribute in format string')
assert capture_error('{0[x]x}', {'x': 1}) == (
    'ValueError',
    "Only '.' or '[' may follow ']' in format field specifier",
)
assert capture_error('{0[x}', {'x': 1}) == ('ValueError', "expected '}' before end of string")
assert capture_error('{0:{1}', 1, 2) == ('ValueError', "unmatched '{' in format spec")
assert capture_error('{0:{spec[a}b]}}', 'x', spec={'a}b': '^5'}) == (
    'ValueError',
    "expected '}' before end of string",
)
assert capture_error('{0:{}}', 1, 2) == (
    'ValueError',
    'cannot switch from manual field specification to automatic field numbering',
)
assert capture_error('{:{0}}', 1, 2) == (
    'ValueError',
    'cannot switch from automatic field numbering to manual field specification',
)
assert capture_error('{0:{1:{2}}}', 1, 2, 3) == ('ValueError', 'Max string recursion exceeded')
assert capture_error('{999999999999999999999999999999999999}', 1) == (
    'ValueError',
    'Too many decimal digits in format string',
)
assert capture_error('{0[999999999999999999999999999999999999]}', {}) == (
    'ValueError',
    'Too many decimal digits in format string',
)

assert '{:0=10,}'.format(1234) == '00,001,234'
assert '{:0=15,.2f}'.format(1234.5) == '0,000,001,234.50'
assert '{:0=+9_g}'.format(1) == '+0_000_001'
assert '{:>010,}'.format(1234) == '000001,234'
assert '{:x=10,}'.format(1234) == 'xxxxx1,234'

assert capture_error('{0:{1:{{}}}}', 'x', 'y') == ('ValueError', 'Max string recursion exceeded')
assert capture_error('{0:{{}}<9}', 5) == (
    'ValueError',
    "Invalid format specifier '{}<9' for object of type 'int'",
)

assert '{[0]}'.format(['a']) == 'a'
assert '{.value}'.format(Record(1)) == '1'
assert capture_error('{x=}', x=1) == ('KeyError', "'x='")
assert '{:%H:%M}'.format(time(4, 5)) == '04:05'
