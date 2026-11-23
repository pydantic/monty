import datetime

# === strftime ===

assert datetime.datetime(2024, 6, 15, 10, 30, 45).strftime('%Y-%m-%d') == '2024-06-15'
assert datetime.datetime(2024, 6, 15, 10, 30, 45).strftime('%H:%M:%S') == '10:30:45'
assert datetime.date(2024, 6, 15).strftime('%Y/%m/%d') == '2024/06/15'
assert datetime.date(2024, 6, 15).strftime(format='%Y/%m/%d') == '2024/06/15'
assert datetime.date(2024, 6, 5).strftime('%B|%b|%A|%a') == 'June|Jun|Wednesday|Wed'
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

# === %f always renders six microsecond digits ===
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

# === %z, %:z and %Z come from utcoffset() and tzname(); empty when naive ===
assert datetime.datetime(2024, 1, 1).strftime('%z|%:z|%Z') == '||'
assert datetime.date(2024, 1, 1).strftime('%z|%Z') == '|'
assert datetime.time(1, 2).strftime('%z|%Z') == '|'
_eet = datetime.timezone(datetime.timedelta(hours=2), 'EET')
_aware = datetime.datetime(2024, 6, 15, 12, 30, tzinfo=_eet)
assert _aware.strftime('%z|%:z|%Z') == '+0200|+02:00|EET'
assert _aware.strftime('%Y-%m-%d %H:%M %Z (%z)') == '2024-06-15 12:30 EET (+0200)'
assert f'{_aware:%Z %z}' == 'EET +0200'
assert '{:%Z}'.format(_aware) == 'EET'
# an unnamed zone reports CPython's UTC±HH:MM name; a sub-minute offset adds seconds
assert datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(hours=-5))).strftime('%z %Z') == (
    '-0500 UTC-05:00'
)
_odd = datetime.timezone(datetime.timedelta(hours=2, minutes=30, seconds=15))
assert datetime.datetime(2024, 1, 1, tzinfo=_odd).strftime('%z|%:z|%Z') == '+023015|+02:30:15|UTC+02:30:15'
assert datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone.utc).strftime('%z %Z') == '+0000 UTC'
assert datetime.time(1, 2, tzinfo=_eet).strftime('%z|%Z') == '+0200|EET'
assert f'{datetime.time(1, 2, tzinfo=_eet):%Z}' == 'EET'
# an escaped percent is a literal, and a percent inside the name stays one
assert _aware.strftime('%%z|%%Z|%%%z') == '%z|%Z|%+0200'
assert datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(0), '50%')).strftime('%Z|%%') == (
    '50%|%'
)

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

# === strptime %z: a sign, hours and minutes with an optional colon, or a bare Z ===
assert repr(datetime.datetime.strptime('2024-06-15 12:30 +0100', '%Y-%m-%d %H:%M %z')) == (
    'datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600)))'
)
assert datetime.datetime.strptime('2024-06-15 12:30 +01:00', '%Y-%m-%d %H:%M %z').utcoffset() == datetime.timedelta(
    hours=1
)
assert datetime.datetime.strptime('2024-06-15 12:30 -0530', '%Y-%m-%d %H:%M %z').utcoffset() == datetime.timedelta(
    hours=-5, minutes=-30
)
# seconds are allowed too, with or without their own colon
assert datetime.datetime.strptime('2024-06-15 12:30 +010203', '%Y-%m-%d %H:%M %z').utcoffset() == datetime.timedelta(
    seconds=3723
)
assert datetime.datetime.strptime('2024-06-15 12:30 +01:02:03', '%Y-%m-%d %H:%M %z').utcoffset() == datetime.timedelta(
    seconds=3723
)
# Optional offset seconds are greedy unless needed by the remaining format.
for _text, _format, _seconds in [
    ('+010003', '%z03', 3600),
    ('+01:00:03', '%z:03', 3600),
    ('+01:0003', '%z03', 3600),
    ('+01000303', '%z03', 3603),
]:
    assert datetime.datetime.strptime(_text, _format).utcoffset() == datetime.timedelta(seconds=_seconds)

# +0000, -0000 and Z all give the UTC singleton
assert datetime.datetime.strptime('2024-06-15 12:30 +0000', '%Y-%m-%d %H:%M %z').tzinfo is datetime.timezone.utc
assert datetime.datetime.strptime('2024-06-15 12:30 -0000', '%Y-%m-%d %H:%M %z').tzinfo is datetime.timezone.utc
assert datetime.datetime.strptime('2024-06-15 12:30 Z', '%Y-%m-%d %H:%M %z').tzinfo is datetime.timezone.utc
# a format without %z stays naive, and %%z is a literal
assert datetime.datetime.strptime('2024-06-15 12:30', '%Y-%m-%d %H:%M').tzinfo is None
assert datetime.datetime.strptime('2024-06-15 %z', '%Y-%m-%d %%z').tzinfo is None
# %Z validates a name without attaching tzinfo
assert datetime.datetime.strptime('2024-06-15 12:30 UTC', '%Y-%m-%d %H:%M %Z').tzinfo is None
# strftime and strptime round trip through the offset
_rt = datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(hours=-5)))
assert datetime.datetime.strptime(_rt.strftime('%Y-%m-%d %H:%M %z'), '%Y-%m-%d %H:%M %z') == _rt

# === strptime numeric fields retry shorter matches when the suffix requires it ===
for _text, _format, _expected in [
    ('120', '%H20', datetime.datetime(1900, 1, 1, 1)),
    ('120', '%I20', datetime.datetime(1900, 1, 1, 1)),
    ('120', '%S20', datetime.datetime(1900, 1, 1, 0, 0, 1)),
    ('1120', '%H%I20', datetime.datetime(1900, 1, 1, 1)),
    ('0120', '%H120', datetime.datetime(1900, 1, 1)),
    ('12PM', '%I2%p', datetime.datetime(1900, 1, 1, 13)),
    ('123', '%H%M', datetime.datetime(1900, 1, 1, 12, 3)),
    (' 0', '%H', datetime.datetime(1900, 1, 1)),
    (' 1', '%H', datetime.datetime(1900, 1, 1, 1)),
    (' 1', '%I', datetime.datetime(1900, 1, 1, 1)),
    (' 9', '%I', datetime.datetime(1900, 1, 1, 9)),
    (' 120', '%H20', datetime.datetime(1900, 1, 1, 1)),
    (' 120', '%I20', datetime.datetime(1900, 1, 1, 1)),
    (' 1 PM', '%I %p', datetime.datetime(1900, 1, 1, 13)),
    ('PM 1', '%p%I', datetime.datetime(1900, 1, 1, 13)),
]:
    assert datetime.datetime.strptime(_text, _format) == _expected
for _text, _format in [('  1', '%H'), (' 0', '%I'), (' 1', '%S')]:
    try:
        datetime.datetime.strptime(_text, _format)
        assert False, 'expected ValueError for invalid clock padding'
    except ValueError as exc:
        assert str(exc) == f'time data {_text!r} does not match format {_format!r}'

# === strptime meridiem affects only the 12-hour clock ===
for _text, _format, _hour in [
    ('13 AM', '%H %p', 13),
    ('01 PM', '%H %p', 1),
    ('AM 13', '%p %H', 13),
    ('PM 01', '%p %H', 1),
    ('12', '%I', 0),
    ('01', '%I', 1),
    ('12 AM', '%I %p', 0),
    ('12 PM', '%I %p', 12),
    ('01 pm', '%I %p', 13),
    ('PM 01', '%p %I', 13),
    ('PM', '%p', 0),
    ('01 12 PM', '%H %I %p', 12),
    ('12 PM 01', '%I %p %H', 1),
    ('%H 12 %p', '%%H %I %%p', 0),
]:
    assert datetime.datetime.strptime('2024-01-01 ' + _text, '%Y-%m-%d ' + _format).hour == _hour

# === strptime rejects leap seconds rather than clamping them ===
for _text, _format, _second in [
    ('23:59:60', '%H:%M:%S', 60),
    ('23:59:61', '%H:%M:%S', 61),
    ('23:59:60.1', '%H:%M:%S.%f', 60),
    ('23:59:61.123456', '%H:%M:%S.%f', 61),
    ('23:59:60', '%X', 60),
    ('60', '%S', 60),
    ('6020', '%S20', 60),
]:
    try:
        datetime.datetime.strptime('2024-01-01 ' + _text, '%Y-%m-%d ' + _format)
        assert False, 'expected ValueError for a leap second'
    except ValueError as exc:
        assert str(exc) == f'second must be in 0..59, not {_second}'

# === strptime zone names are validated and leave following literals intact ===
for _text, _format in [
    ('2024-01-01 UTC', '%Y-%m-%d %Z'),
    ('2024-01-01 gmt', '%Y-%m-%d %Z'),
    ('2024-01-01 UTC,', '%Y-%m-%d %Z,'),
    ('UTC2024-01-01', '%Z%Y-%m-%d'),
    ('2024-01-01 %Z', '%Y-%m-%d %%Z'),
]:
    assert datetime.datetime.strptime(_text, _format) == datetime.datetime(2024, 1, 1)
assert datetime.datetime.strptime('2024-01-01 UTC+0100', '%Y-%m-%d %Z%z') == datetime.datetime(
    2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(hours=1))
)
for _zone in ['ARBITRARY', 'UT', '']:
    try:
        datetime.datetime.strptime('2024-01-01 ' + _zone, '%Y-%m-%d %Z')
        assert False, 'expected ValueError for an unsupported zone name'
    except ValueError as exc:
        assert str(exc) == f"time data '2024-01-01 {_zone}' does not match format '%Y-%m-%d %Z'"

# === strptime missing fields default to 1900-01-01 at midnight ===
for _text, _format, _expected in [
    ('12:30', '%H:%M', datetime.datetime(1900, 1, 1, 12, 30)),
    ('2024', '%Y', datetime.datetime(2024, 1, 1)),
    ('02', '%m', datetime.datetime(1900, 2, 1)),
    ('15', '%d', datetime.datetime(1900, 1, 15)),
    ('45', '%M', datetime.datetime(1900, 1, 1, 0, 45)),
    ('12', '%S', datetime.datetime(1900, 1, 1, 0, 0, 12)),
    ('000123', '%f', datetime.datetime(1900, 1, 1, 0, 0, 0, 123)),
    ('', '', datetime.datetime(1900, 1, 1)),
    ('literal', 'literal', datetime.datetime(1900, 1, 1)),
    ('060', '%j', datetime.datetime(1900, 3, 1)),
    ('01 1', '%W %w', datetime.datetime(1900, 1, 1)),
    ('2020 01 1', '%G %V %u', datetime.datetime(2019, 12, 30)),
    ('2024 060', '%Y %j', datetime.datetime(2024, 2, 29)),
    (
        '12:30 +0100',
        '%H:%M %z',
        datetime.datetime(1900, 1, 1, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(hours=1))),
    ),
]:
    assert datetime.datetime.strptime(_text, _format) == _expected

try:
    datetime.datetime.strptime('02/29', '%m/%d')
    assert False, 'expected ValueError for February 29 in the default year'
except ValueError as exc:
    assert str(exc) == 'day 29 must be in range 1..28 for month 2 in year 1900'

# === strptime trailing data is reported before component and offset validation ===
for _text, _format, _remaining in [
    ('12:30 extra', '%H:%M', ' extra'),
    ('2024-02-30 extra', '%Y-%m-%d', ' extra'),
    ('2024-06-15 12:30+01:00', '%Y-%m-%d %H:%M', '+01:00'),
    ('+0100 2024-06-15 extra', '%z %Y-%m-%d', ' extra'),
    ('2024-06-15 12:30 +0100 extra', '%Y-%m-%d %H:%M %z', ' extra'),
    ('2024-06-15 +0100 12:30 extra', '%Y-%m-%d %z %H:%M', ' extra'),
    ('2024-06-15 +01:0203 extra', '%Y-%m-%d %z', ' extra'),
    ('2024-06-15 +2400 extra', '%Y-%m-%d %z', ' extra'),
    ('2024-06-15 UTC extra', '%Y-%m-%d %Z', ' extra'),
    ('2024-06-15 UTC 12:30 extra', '%Y-%m-%d %Z %H:%M', ' extra'),
    ('2024-06-15 UTC +0100 extra', '%Y-%m-%d %Z %z', ' extra'),
    ('2024-06-15 +0100 UTC extra', '%Y-%m-%d %z %Z', ' extra'),
    ('2024-06-15 extra日', '%Y-%m-%d', ' extra日'),
    ('2024-06-15 ', '%Y-%m-%d', ' '),
    ('extra', '', 'extra'),
    ('120x', '%H20', 'x'),
    ('122020', '%H20', '20'),
    (' 12', '%H', '2'),
    ('2024-01-01 UTCextra', '%Y-%m-%d %Z', 'extra'),
    ('2024-01-01 UTC,extra', '%Y-%m-%d %Z,', 'extra'),
    ('2024-01-01 23:59:60 extra', '%Y-%m-%d %H:%M:%S', ' extra'),
]:
    try:
        datetime.datetime.strptime(_text, _format)
        assert False, 'expected ValueError for trailing input'
    except ValueError as exc:
        assert str(exc) == 'unconverted data remains: ' + _remaining

# === strptime fractions, locale expansions, and offsets between civil fields ===
for _fraction in ['1', '12', '123', '1234', '12345', '123456']:
    _parsed = datetime.datetime.strptime('2024-06-15 10:30:45.' + _fraction, '%Y-%m-%d %H:%M:%S.%f')
    assert _parsed.microsecond == int(_fraction.ljust(6, '0'))
assert datetime.datetime.strptime('2024-06-15 +01:02:03 12:30', '%Y-%m-%d %z %H:%M') == datetime.datetime(
    2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3723))
)
assert datetime.datetime.strptime('12Z2024', '%f%z%Y') == datetime.datetime(
    2024, 1, 1, microsecond=120000, tzinfo=datetime.timezone.utc
)
assert datetime.datetime.strptime('06/15/24 10:30:45', '%x %X') == datetime.datetime(2024, 6, 15, 10, 30, 45)
assert datetime.datetime.strptime('Sat Jun 15 10:30:45 2024', '%c') == datetime.datetime(2024, 6, 15, 10, 30, 45)

# === strptime %z rejections ===
# lowercase z, a missing sign, a bare hour and a minute above 59 are all no match
for _bad in ['z', '0100', '+01', '+0160']:
    try:
        datetime.datetime.strptime('2024-06-15 ' + _bad, '%Y-%m-%d %z')
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == f"time data '2024-06-15 {_bad}' does not match format '%Y-%m-%d %z'"
# the minute and second separators have to agree: a mixed pair matches, then is refused
# (the mirrored '+0102:03' is rejected by both, with different wording — see limitations/datetime.md)
for _mixed in ['+01:0203', '-01:0203']:
    try:
        datetime.datetime.strptime('2024-06-15 ' + _mixed, '%Y-%m-%d %z')
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == f'Inconsistent use of : in {_mixed}'
# the hour is only bounded by the timezone range, so 23:59 parses and 24:00 does not
assert datetime.datetime.strptime('2024-06-15 +2359', '%Y-%m-%d %z').utcoffset() == datetime.timedelta(
    hours=23, minutes=59
)
try:
    datetime.datetime.strptime('2024-06-15 +2400', '%Y-%m-%d %z')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == (
        'offset must be a timedelta strictly between -timedelta(hours=24) and '
        'timedelta(hours=24), not datetime.timedelta(days=1)'
    )
# the leading directives decide where %z can start, so a long run of offset-shaped
# text costs one parse rather than one per position it could have begun at
_long = '+0000' * 2000
try:
    datetime.datetime.strptime(_long, '%Y-%m-%d %z')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == f"time data '{_long}' does not match format '%Y-%m-%d %z'"
# %:z formats but does not parse, so the colon reads as a directive of its own
try:
    datetime.datetime.strptime('2024-06-15 +01:00', '%Y-%m-%d %:z')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == "':' is a bad directive in format '%Y-%m-%d %:z'"

# === strptime rejects directives outside CPython's table ===
for _format, _bad in [
    ('%s', 's'),
    ('%F', 'F'),
    ('%D', 'D'),
    ('%C', 'C'),
    ('%h', 'h'),
    ('%Q', 'Q'),
    ('%q', 'q'),
    ('%N', 'N'),
    ('%.f', '.'),
    ('%E', 'E'),
    ('%Ez', 'Ez'),
    ('%Oz', 'Oz'),
    ('%-s', 's'),
    ('%é', 'é'),
]:
    try:
        datetime.datetime.strptime('2024', _format)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == f"'{_bad}' is a bad directive in format '{_format}'"
# a % with nothing, or only whitespace, after it is stray
for _format in ['%', '%Y%', '% ', "%Y'%"]:
    try:
        datetime.datetime.strptime('2024', _format)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == "stray % in format '" + _format + "'"
# flags and widths are ignored, so `%-f` keeps its six-digit limit and `%-c` still expands
assert datetime.datetime.strptime('5', '%-d') == datetime.datetime(1900, 1, 5)
assert datetime.datetime.strptime('7', '%-H') == datetime.datetime(1900, 1, 1, 7)
assert datetime.datetime.strptime('Mon Jun  3 07:03:04 2024', '%-c') == datetime.datetime(2024, 6, 3, 7, 3, 4)
try:
    datetime.datetime.strptime('1234567', '%-f')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'unconverted data remains: 7'

# === strptime %f takes at most six digits ===
for _text, _remaining in [('1234567', '7'), ('123456789', '789')]:
    try:
        datetime.datetime.strptime('10:30:45.' + _text, '%H:%M:%S.%f')
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'unconverted data remains: ' + _remaining
assert datetime.datetime.strptime('000001', '%f').microsecond == 1
# %f gives digits back to a following numeric field
assert datetime.datetime.strptime('123', '%f%S') == datetime.datetime(1900, 1, 1, 0, 0, 3, 120000)
assert datetime.datetime.strptime('12345', '%f%H') == datetime.datetime(1900, 1, 1, 5, 0, 0, 123400)
assert datetime.datetime.strptime('45.5Z', '%S.%f%z') == datetime.datetime(
    1900, 1, 1, 0, 0, 45, 500000, tzinfo=datetime.timezone.utc
)

# === strptime %j wins over month and day, and runs on past the year ===
assert datetime.datetime.strptime('2024-03-01 060', '%Y-%m-%d %j') == datetime.datetime(2024, 2, 29)
assert datetime.datetime.strptime('2023 366', '%Y %j') == datetime.datetime(2024, 1, 1)
assert datetime.datetime.strptime('2024 366', '%Y %j') == datetime.datetime(2024, 12, 31)
assert datetime.datetime.strptime('2023 W01 1 010', '%Y W%W %w %j') == datetime.datetime(2023, 1, 10)
