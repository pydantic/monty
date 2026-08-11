# Every example from https://pyformat.info/ , converted to f-strings.
#
# pyformat.info presents each example in old `%` style and new `str.format()`
# style. Monty implements the format mini-language ONLY through f-strings (no
# `str.format()` / `%` / general `__format__` protocol — see
# limitations/format.md), so each `.format()` example is written here as the
# equivalent f-string. Examples that rely on a user-defined class (`Data`,
# `Plant`, `HAL9000`) can't be represented verbatim — Monty has no `class`
# statement — so they are adapted to built-in values that exercise the same
# formatting feature, with a note.

from datetime import datetime

# === Basic formatting ===
# '{} {}'.format('one', 'two') / '{} {}'.format(1, 2) / '{1} {0}'.format(...)
assert f'{"one"} {"two"}' == 'one two'
assert f'{1} {2}' == '1 2'
# `{1} {0}` reorders args; f-strings inline the values in the wanted order.
assert f'{"two"} {"one"}' == 'two one'

# === Value conversion (!s / !r / !a) ===
# Original uses a Data() class whose __str__/__repr__ differ. A plain string
# shows the same distinction: !s -> str(), !r -> repr() (adds quotes).
assert f'{"text"!s} {"text"!r}' == "text 'text'"
# '{0!r} {0!a}' on a non-ASCII value: !r keeps it, !a escapes to ASCII.
assert f'{"räpr"!r} {"räpr"!a}' == "'räpr' 'r\\xe4pr'"

# === Padding and aligning strings ===
assert f'{"test":>10}' == '      test'
assert f'{"test":10}' == 'test      '
assert f'{"test":_<10}' == 'test______'
assert f'{"test":^10}' == '   test   '
assert f'{"zip":^6}' == ' zip  '

# === Truncating long strings ===
assert f'{"xylophone":.5}' == 'xylop'

# === Combining truncating and padding ===
assert f'{"xylophone":10.5}' == 'xylop     '

# === Numbers ===
assert f'{42:d}' == '42'
assert f'{3.141592653589793:f}' == '3.141593'

# === Padding numbers ===
assert f'{42:4d}' == '  42'
assert f'{3.141592653589793:06.2f}' == '003.14'
assert f'{42:04d}' == '0042'

# === Signed numbers ===
assert f'{42:+d}' == '+42'
assert f'{-23: d}' == '-23'
assert f'{42: d}' == ' 42'
assert f'{-23:=5d}' == '-  23'
assert f'{23:=+5d}' == '+  23'

# === Named placeholders ===
# '{first} {last}'.format(first='Hodor', last='Hodor!') -> just reference vars.
first = 'Hodor'
last = 'Hodor!'
assert f'{first} {last}' == 'Hodor Hodor!'

# === Getitem and getattr ===
# '{p[first]} {p[last]}'.format(p=person) — dict subscript in the expression.
person = {'first': 'Jean-Luc', 'last': 'Picard'}
assert f'{person["first"]} {person["last"]}' == 'Jean-Luc Picard'
# '{d[4]} {d[5]}'.format(d=data) — list indexing.
data = [0, 1, 2, 3, 23, 42]
assert f'{data[4]} {data[5]}' == '23 42'
# '{p.type}: {p.kinds[0][name]}' uses a custom class; demonstrate the same
# attribute-access + chained-getitem capability with built-in values.
dt = datetime(2001, 2, 3, 4, 5)
assert f'{dt.year}' == '2001'
plant = {'kinds': [{'name': 'oak'}]}
assert f'tree: {plant["kinds"][0]["name"]}' == 'tree: oak'

# === Datetime ===
assert f'{dt:%Y-%m-%d %H:%M}' == '2001-02-03 04:05'

# === Parametrized (nested) formats ===
# '{:{align}{width}}'.format('test', align='^', width='10')
align = '^'
width = 10
assert f'{"test":{align}{width}}' == '   test   '
# '{:.{prec}} = {:.{prec}f}'.format('Gibberish', 2.7182, prec=3)
prec = 3
assert f'{"Gibberish":.{prec}} = {2.7182:.{prec}f}' == 'Gib = 2.718'
# '{:{width}.{prec}f}'.format(2.7182, width=5, prec=2)
w = 5
p = 2
assert f'{2.7182:{w}.{p}f}' == ' 2.72'
# '{:{prec}} = {:{prec}}'.format('Gibberish', 2.7182, prec='.3')
prec_str = '.3'
assert f'{"Gibberish":{prec_str}} = {2.7182:{prec_str}}' == 'Gib = 2.72'
# '{:{dfmt} {tfmt}}'.format(dt, dfmt='%Y-%m-%d', tfmt='%H:%M') — nested strftime.
dfmt = '%Y-%m-%d'
tfmt = '%H:%M'
assert f'{dt:{dfmt} {tfmt}}' == '2001-02-03 04:05'
# '{:{}{}{}.{}}'.format(2.7182818284, '>', '+', 10, 3) — all parts parametrized.
assert f'{2.7182818284:{">"}{"+"}{10}.{3}}' == '     +2.72'
# '{:{}{sign}{}.{}}'.format(2.7182818284, '>', 10, 3, sign='+') — mixed.
sign = '+'
assert f'{2.7182818284:{">"}{sign}{10}.{3}}' == '     +2.72'

# === Escaping braces ===
# '{{}}'.format() -> literal braces; '{{{}}}'.format('x') -> '{x}'.
assert f'{{}}' == '{}'
assert f'{{{"x"}}}' == '{x}'

# === Custom objects (not representable in Monty) ===
# pyformat.info also shows '{:%Y}'.format(custom) and
# '{:open-the-pod-bay-doors}'.format(HAL9000()), which dispatch to a
# user-defined __format__. Monty has no `class` statement and no general
# __format__ protocol (only date/datetime get strftime handling — covered
# above), so these have no f-string equivalent and are intentionally omitted.
