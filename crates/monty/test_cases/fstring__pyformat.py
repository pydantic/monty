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
assert f'{"one"} {"two"}' == 'one two', 'two positional strings'
assert f'{1} {2}' == '1 2', 'two positional ints'
# `{1} {0}` reorders args; f-strings inline the values in the wanted order.
assert f'{"two"} {"one"}' == 'two one', 'reordered positionals'

# === Value conversion (!s / !r / !a) ===
# Original uses a Data() class whose __str__/__repr__ differ. A plain string
# shows the same distinction: !s -> str(), !r -> repr() (adds quotes).
assert f'{"text"!s} {"text"!r}' == "text 'text'", '!s vs !r conversion'
# '{0!r} {0!a}' on a non-ASCII value: !r keeps it, !a escapes to ASCII.
assert f'{"räpr"!r} {"räpr"!a}' == "'räpr' 'r\\xe4pr'", '!r vs !a conversion'

# === Padding and aligning strings ===
assert f'{"test":>10}' == '      test', 'right-align in width 10'
assert f'{"test":10}' == 'test      ', 'strings left-align by default'
assert f'{"test":_<10}' == 'test______', 'left-align with _ fill'
assert f'{"test":^10}' == '   test   ', 'center-align in width 10'
assert f'{"zip":^6}' == ' zip  ', 'center-align with odd padding'

# === Truncating long strings ===
assert f'{"xylophone":.5}' == 'xylop', 'truncate to 5 chars'

# === Combining truncating and padding ===
assert f'{"xylophone":10.5}' == 'xylop     ', 'truncate to 5 then pad to 10'

# === Numbers ===
assert f'{42:d}' == '42', 'integer'
assert f'{3.141592653589793:f}' == '3.141593', 'float (default 6 dp)'

# === Padding numbers ===
assert f'{42:4d}' == '  42', 'int padded to width 4'
assert f'{3.141592653589793:06.2f}' == '003.14', 'float width 6, 2 dp, zero-pad'
assert f'{42:04d}' == '0042', 'int zero-padded to width 4'

# === Signed numbers ===
assert f'{42:+d}' == '+42', 'force + sign'
assert f'{-23: d}' == '-23', 'space sign on negative'
assert f'{42: d}' == ' 42', 'space sign on positive'
assert f'{-23:=5d}' == '-  23', 'sign-aware zero-position padding'
assert f'{23:=+5d}' == '+  23', 'sign-aware padding with forced +'

# === Named placeholders ===
# '{first} {last}'.format(first='Hodor', last='Hodor!') -> just reference vars.
first = 'Hodor'
last = 'Hodor!'
assert f'{first} {last}' == 'Hodor Hodor!', 'named placeholders'

# === Getitem and getattr ===
# '{p[first]} {p[last]}'.format(p=person) — dict subscript in the expression.
person = {'first': 'Jean-Luc', 'last': 'Picard'}
assert f'{person["first"]} {person["last"]}' == 'Jean-Luc Picard', 'dict getitem'
# '{d[4]} {d[5]}'.format(d=data) — list indexing.
data = [0, 1, 2, 3, 23, 42]
assert f'{data[4]} {data[5]}' == '23 42', 'list getitem'
# '{p.type}: {p.kinds[0][name]}' uses a custom class; demonstrate the same
# attribute-access + chained-getitem capability with built-in values.
dt = datetime(2001, 2, 3, 4, 5)
assert f'{dt.year}' == '2001', 'attribute access in f-string'
plant = {'kinds': [{'name': 'oak'}]}
assert f'tree: {plant["kinds"][0]["name"]}' == 'tree: oak', 'chained getitem'

# === Datetime ===
assert f'{dt:%Y-%m-%d %H:%M}' == '2001-02-03 04:05', 'datetime strftime spec'

# === Parametrized (nested) formats ===
# '{:{align}{width}}'.format('test', align='^', width='10')
align = '^'
width = 10
assert f'{"test":{align}{width}}' == '   test   ', 'nested align + width'
# '{:.{prec}} = {:.{prec}f}'.format('Gibberish', 2.7182, prec=3)
prec = 3
assert f'{"Gibberish":.{prec}} = {2.7182:.{prec}f}' == 'Gib = 2.718', 'nested precision (str + float)'
# '{:{width}.{prec}f}'.format(2.7182, width=5, prec=2)
w = 5
p = 2
assert f'{2.7182:{w}.{p}f}' == ' 2.72', 'nested width and precision'
# '{:{prec}} = {:{prec}}'.format('Gibberish', 2.7182, prec='.3')
prec_str = '.3'
assert f'{"Gibberish":{prec_str}} = {2.7182:{prec_str}}' == 'Gib = 2.72', 'nested whole-precision spec'
# '{:{dfmt} {tfmt}}'.format(dt, dfmt='%Y-%m-%d', tfmt='%H:%M') — nested strftime.
dfmt = '%Y-%m-%d'
tfmt = '%H:%M'
assert f'{dt:{dfmt} {tfmt}}' == '2001-02-03 04:05', 'nested datetime strftime spec'
# '{:{}{}{}.{}}'.format(2.7182818284, '>', '+', 10, 3) — all parts parametrized.
assert f'{2.7182818284:{">"}{"+"}{10}.{3}}' == '     +2.72', 'fully parametrized spec'
# '{:{}{sign}{}.{}}'.format(2.7182818284, '>', 10, 3, sign='+') — mixed.
sign = '+'
assert f'{2.7182818284:{">"}{sign}{10}.{3}}' == '     +2.72', 'mixed positional/named parametrized spec'

# === Escaping braces ===
# '{{}}'.format() -> literal braces; '{{{}}}'.format('x') -> '{x}'.
assert f'{{}}' == '{}', 'escaped empty braces'
assert f'{{{"x"}}}' == '{x}', 'escaped braces around a value'

# === Custom objects (not representable in Monty) ===
# pyformat.info also shows '{:%Y}'.format(custom) and
# '{:open-the-pod-bay-doors}'.format(HAL9000()), which dispatch to a
# user-defined __format__. Monty has no `class` statement and no general
# __format__ protocol (only date/datetime get strftime handling — covered
# above), so these have no f-string equivalent and are intentionally omitted.
