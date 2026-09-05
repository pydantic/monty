# Every example from https://pyformat.info/ in each form the site shows:
# old-style `%`, new-style `.format()`, and the f-string equivalent.

from datetime import datetime


class Data:
    def __str__(self):
        return 'str'

    def __repr__(self):
        return 'repr'


class Umlaut:
    def __repr__(self):
        return 'räpr'


class Plant:
    type = 'tree'
    kinds = [{'name': 'oak'}, {'name': 'maple'}]


# === Basic formatting ===
assert '%s %s' % ('one', 'two') == 'one two'
assert '{} {}'.format('one', 'two') == 'one two'
assert f'{"one"} {"two"}' == 'one two'
assert '%d %d' % (1, 2) == '1 2'
assert '{} {}'.format(1, 2) == '1 2'
assert f'{1} {2}' == '1 2'
assert '{1} {0}'.format('one', 'two') == 'two one'
assert f'{"two"} {"one"}' == 'two one'

# === Value conversion ===
assert '%s %r' % (Data(), Data()) == 'str repr'
assert '{0!s} {0!r}'.format(Data()) == 'str repr'
assert f'{Data()!s} {Data()!r}' == 'str repr'
assert '%r %a' % (Umlaut(), Umlaut()) == 'räpr r\\xe4pr'
assert '{0!r} {0!a}'.format(Umlaut()) == 'räpr r\\xe4pr'
assert f'{Umlaut()!r} {Umlaut()!a}' == 'räpr r\\xe4pr'

# === Padding and aligning strings ===
assert '%10s' % ('test',) == '      test'
assert '{:>10}'.format('test') == '      test'
assert f'{"test":>10}' == '      test'
assert '%-10s' % ('test',) == 'test      '
assert '{:10}'.format('test') == 'test      '
assert f'{"test":10}' == 'test      '
assert '{:_<10}'.format('test') == 'test______'
assert f'{"test":_<10}' == 'test______'
assert '{:^10}'.format('test') == '   test   '
assert f'{"test":^10}' == '   test   '
assert '{:^6}'.format('zip') == ' zip  '
assert f'{"zip":^6}' == ' zip  '

# === Truncating long strings ===
assert '%.5s' % ('xylophone',) == 'xylop'
assert '{:.5}'.format('xylophone') == 'xylop'
assert f'{"xylophone":.5}' == 'xylop'

# === Combining truncating and padding ===
assert '%-10.5s' % ('xylophone',) == 'xylop     '
assert '{:10.5}'.format('xylophone') == 'xylop     '
assert f'{"xylophone":10.5}' == 'xylop     '

# === Numbers ===
assert '%d' % (42,) == '42'
assert '{:d}'.format(42) == '42'
assert f'{42:d}' == '42'
assert '%f' % (3.141592653589793,) == '3.141593'
assert '{:f}'.format(3.141592653589793) == '3.141593'
assert f'{3.141592653589793:f}' == '3.141593'

# === Padding numbers ===
assert '%4d' % (42,) == '  42'
assert '{:4d}'.format(42) == '  42'
assert f'{42:4d}' == '  42'
assert '%06.2f' % (3.141592653589793,) == '003.14'
assert '{:06.2f}'.format(3.141592653589793) == '003.14'
assert f'{3.141592653589793:06.2f}' == '003.14'
assert '%04d' % (42,) == '0042'
assert '{:04d}'.format(42) == '0042'
assert f'{42:04d}' == '0042'

# === Signed numbers ===
assert '%+d' % (42,) == '+42'
assert '{:+d}'.format(42) == '+42'
assert f'{42:+d}' == '+42'
assert '% d' % ((-23),) == '-23'
assert '{: d}'.format((-23)) == '-23'
assert f'{-23: d}' == '-23'
assert '% d' % (42,) == ' 42'
assert '{: d}'.format(42) == ' 42'
assert f'{42: d}' == ' 42'
assert '{:=5d}'.format((-23)) == '-  23'
assert f'{-23:=5d}' == '-  23'
assert '{:=+5d}'.format(23) == '+  23'
assert f'{23:=+5d}' == '+  23'

# === Named placeholders ===
data = {'first': 'Hodor', 'last': 'Hodor!'}
assert '%(first)s %(last)s' % data == 'Hodor Hodor!'
assert '{first} {last}'.format(**data) == 'Hodor Hodor!'
assert '{first} {last}'.format(first='Hodor', last='Hodor!') == 'Hodor Hodor!'
first = 'Hodor'
last = 'Hodor!'
assert f'{first} {last}' == 'Hodor Hodor!'

# === Getitem and Getattr ===
person = {'first': 'Jean-Luc', 'last': 'Picard'}
assert '{p[first]} {p[last]}'.format(p=person) == 'Jean-Luc Picard'
assert f'{person["first"]} {person["last"]}' == 'Jean-Luc Picard'
data = [4, 8, 15, 16, 23, 42]
assert '{d[4]} {d[5]}'.format(d=data) == '23 42'
assert f'{data[4]} {data[5]}' == '23 42'
assert '{p.type}'.format(p=Plant()) == 'tree'
assert f'{Plant().type}' == 'tree'
assert '{p.type}: {p.kinds[0][name]}'.format(p=Plant()) == 'tree: oak'
assert f'{Plant().type}: {Plant().kinds[0]["name"]}' == 'tree: oak'

# === Datetime ===
assert '{:%Y-%m-%d %H:%M}'.format(datetime(2001, 2, 3, 4, 5)) == '2001-02-03 04:05'
assert f'{datetime(2001, 2, 3, 4, 5):%Y-%m-%d %H:%M}' == '2001-02-03 04:05'

# === Parametrized formats ===
assert '{:{align}{width}}'.format('test', align='^', width='10') == '   test   '
align = '^'
width = 10
assert f'{"test":{align}{width}}' == '   test   '
assert '%.*s = %.*f' % (3, 'Gibberish', 3, 2.7182) == 'Gib = 2.718'
assert '{:.{prec}} = {:.{prec}f}'.format('Gibberish', 2.7182, prec=3) == 'Gib = 2.718'
prec = 3
assert f'{"Gibberish":.{prec}} = {2.7182:.{prec}f}' == 'Gib = 2.718'
assert '%*.*f' % (5, 2, 2.7182) == ' 2.72'
assert '{:{width}.{prec}f}'.format(2.7182, width=5, prec=2) == ' 2.72'
w = 5
p = 2
assert f'{2.7182:{w}.{p}f}' == ' 2.72'
assert '{:{prec}} = {:{prec}}'.format('Gibberish', 2.7182, prec='.3') == 'Gib = 2.72'
prec_str = '.3'
assert f'{"Gibberish":{prec_str}} = {2.7182:{prec_str}}' == 'Gib = 2.72'
dt = datetime(2001, 2, 3, 4, 5)
assert '{:{dfmt} {tfmt}}'.format(dt, dfmt='%Y-%m-%d', tfmt='%H:%M') == '2001-02-03 04:05'
dfmt = '%Y-%m-%d'
tfmt = '%H:%M'
assert f'{dt:{dfmt} {tfmt}}' == '2001-02-03 04:05'
assert '{:{}{}{}.{}}'.format(2.7182818284, '>', '+', 10, 3) == '     +2.72'
assert f'{2.7182818284:{">"}{"+"}{10}.{3}}' == '     +2.72'
assert '{:{}{sign}{}.{}}'.format(2.7182818284, '>', 10, 3, sign='+') == '     +2.72'
sign = '+'
assert f'{2.7182818284:{">"}{sign}{10}.{3}}' == '     +2.72'

# === Escaping braces ===
assert '{{}}'.format() == '{}'
assert f'{{}}' == '{}'
assert '{{{}}}'.format('x') == '{x}'
assert f'{{{"x"}}}' == '{x}'

# === Custom objects ===
# pyformat.info's last example, '{:open-the-pod-bay-doors}'.format(HAL9000()),
# dispatches to a user-defined __format__. Monty has no general __format__
# protocol (see limitations/format.md), so it is intentionally omitted.
