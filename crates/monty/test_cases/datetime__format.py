import datetime

# === strftime ===

assert datetime.datetime(2024, 6, 15, 10, 30, 45).strftime('%Y-%m-%d') == '2024-06-15'
assert datetime.datetime(2024, 6, 15, 10, 30, 45).strftime('%H:%M:%S') == '10:30:45'
assert datetime.date(2024, 6, 15).strftime('%Y/%m/%d') == '2024/06/15'
assert datetime.date(2024, 6, 15).strftime(format='%Y/%m/%d') == '2024/06/15'
assert datetime.datetime.strptime('2024-06-15 10:30:45.1', '%Y-%m-%d %H:%M:%S.%f') == datetime.datetime(
    2024, 6, 15, 10, 30, 45, 100000
)

try:
    datetime.date(2024, 6, 15).strftime()
    assert False, 'expected strftime() with no args to fail'
except TypeError as exc:
    assert str(exc) == "strftime() missing required argument 'format' (pos 1)", f'strftime() no-args: {exc}'

try:
    datetime.date(2024, 6, 15).strftime('%Y', '%m')
    assert False, 'expected strftime() with extra positional to fail'
except TypeError as exc:
    assert str(exc) == 'strftime() takes at most 1 argument (2 given)', f'strftime() extra positional: {exc}'

try:
    datetime.date(2024, 6, 15).strftime('%Y', extra='nope')
    assert False, 'expected strftime() with unexpected kwarg to fail'
except TypeError as exc:
    assert str(exc) == 'strftime() takes at most 1 argument (2 given)', f'strftime() unexpected kwarg: {exc}'

# Wrong-type `format` matches CPython's `_PyArg_BadArgument` wording, including
# the special "not None" case (vs. the type name "NoneType").
for bad, expected_type in (
    (42, 'int'),
    (None, 'None'),
    (b'%Y', 'bytes'),
    (1.5, 'float'),
    (True, 'bool'),
    ([1, 2], 'list'),
    ({1: 2}, 'dict'),
    ((1, 2), 'tuple'),
):
    try:
        datetime.date(2024, 6, 15).strftime(bad)
        assert False, f'expected strftime({bad!r}) to fail'
    except TypeError as exc:
        assert str(exc) == f'strftime() argument 1 must be str, not {expected_type}', (
            f'strftime({bad!r}) wrong type: {exc}'
        )
    # Same wording when passed as a kwarg.
    try:
        datetime.date(2024, 6, 15).strftime(format=bad)
        assert False, f'expected strftime(format={bad!r}) to fail'
    except TypeError as exc:
        assert str(exc) == f'strftime() argument 1 must be str, not {expected_type}', (
            f'strftime(format={bad!r}) wrong type: {exc}'
        )

# Same error wording on `datetime.strftime`.
try:
    datetime.datetime(2024, 6, 15).strftime(42)
    assert False, 'expected datetime.strftime(42) to fail'
except TypeError as exc:
    assert str(exc) == 'strftime() argument 1 must be str, not int', f'datetime.strftime wrong type: {exc}'

# NOTE: an *unrecognised* directive (e.g. `%Q`) can't be asserted here. Monty
# matches glibc/Linux CPython — it passes the directive through verbatim
# (`'%Q'`) — but macOS CPython instead drops the `%` (`'Q'`), and this file is
# checked against whatever CPython the host runs. That glibc-matching behaviour
# lives in `tests/datetime_format.rs`. See limitations/datetime.md.

# === %f renders microseconds, not chrono's nanoseconds ===
assert datetime.datetime(2020, 1, 2, 3, 4, 5, 123456).strftime('%f') == '123456'
assert datetime.datetime(2020, 1, 2, 3, 4, 5, 7).strftime('%f') == '000007'
assert datetime.datetime(2020, 1, 2, 3, 4, 5, 123456).strftime('%H:%M:%S.%f') == '03:04:05.123456'
assert datetime.date(2020, 1, 2).strftime('%f') == '000000'
assert datetime.time(1, 2, 3, 4).strftime('%f') == '000004'
assert f'{datetime.datetime(2020, 1, 2, 0, 0, 0, 99):%f}' == '000099'
# an escaped percent is a literal, so the `f` after it is not a directive
assert datetime.datetime(2020, 1, 2).strftime('%%f') == '%f'
assert datetime.datetime(2020, 1, 2).strftime('100%% %f') == '100% 000000'
assert datetime.datetime(2020, 1, 2).strftime('%%%f') == '%000000'

# === time directives on a bare date read as midnight ===
_d_mid = datetime.date(2024, 6, 15)
assert _d_mid.strftime('%H:%M:%S') == '00:00:00'
assert _d_mid.strftime('%I %p') == '12 AM'
assert _d_mid.strftime('%Y-%m-%d %H:%M') == '2024-06-15 00:00'
assert f'{_d_mid:%d/%m/%Y %H:%M}' == '15/06/2024 00:00'

# === f-string / format() of date & datetime (strftime via __format__) ===
_d = datetime.date(2024, 6, 15)
_dt = datetime.datetime(2024, 6, 15, 10, 30, 45)
# literal strftime spec
assert f'{_d:%Y-%m-%d}' == '2024-06-15'
assert f'{_dt:%Y-%m-%d %H:%M:%S}' == '2024-06-15 10:30:45'
assert f'{_dt:%Y-%m-%dT%H:%M}' == '2024-06-15T10:30'
assert f'{_d:Year %Y!}' == 'Year 2024!'
# no spec falls back to str()
assert f'{_d}' == '2024-06-15'
assert f'{_dt}' == '2024-06-15 10:30:45'
# dynamic spec (nested interpolation) carries the strftime string
_fmt = '%Y/%m/%d'
assert f'{_d:{_fmt}}' == '2024/06/15'
# empty dynamic spec behaves like str()
_empty = ''
assert f'{_dt:{_empty}}' == '2024-06-15 10:30:45'
# a conversion flag converts to a string first, so the spec formats that string
assert f'{_d!s:>12}' == '  2024-06-15'
# the format() builtin takes the same path
assert format(_d, '%Y/%m/%d') == '2024/06/15'
assert format(_dt, '%H:%M') == '10:30'
assert format(datetime.time(10, 30, 45), '%H-%M-%S') == '10-30-45'
assert format(_d) == '2024-06-15'
assert format(_dt, '') == '2024-06-15 10:30:45'
