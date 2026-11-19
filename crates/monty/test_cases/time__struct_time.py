# The `time` module's conversion functions, with explicit times so nothing
# depends on the clock. The zone-dependent halves live in datetime__zone_*.py.
import sys
import time


def check(fn, expected):
    try:
        fn()
        raise AssertionError('expected failure')
    except Exception as exc:
        got = f'{type(exc).__name__}: {exc}'
        assert got == expected, f'{got!r} != {expected!r}'


# === gmtime ===
epoch = time.gmtime(0)
assert (epoch.tm_year, epoch.tm_mon, epoch.tm_mday) == (1970, 1, 1)
assert (epoch.tm_hour, epoch.tm_min, epoch.tm_sec) == (0, 0, 0)
assert (epoch.tm_wday, epoch.tm_yday, epoch.tm_isdst) == (3, 1, 0)
mid = time.gmtime(1718451000)
assert (mid.tm_year, mid.tm_mon, mid.tm_mday) == (2024, 6, 15)
assert (mid.tm_hour, mid.tm_min, mid.tm_sec) == (11, 30, 0)
assert (mid.tm_wday, mid.tm_yday) == (5, 167)
# fractional and negative seconds floor rather than round
assert time.gmtime(1.9).tm_sec == 1
assert time.gmtime(-1.9).tm_sec == 58
assert time.gmtime(-1.9).tm_year == 1969
assert time.gmtime(True).tm_sec == 1


class Index:
    def __index__(self) -> int:
        return 5


assert time.gmtime(Index()).tm_sec == 5

# === asctime / ctime ===
# the fixed 24-character form, with a space-padded day
assert time.asctime(epoch) == 'Thu Jan  1 00:00:00 1970'
assert time.asctime(mid) == 'Sat Jun 15 11:30:00 2024'
assert time.ctime(0) == time.asctime(time.localtime(0))
assert time.asctime((1, 1, 1, 0, 0, 0, 0, 1, 0)) == 'Mon Jan  1 00:00:00 1'

# === strftime ===
assert time.strftime('%Y-%m-%d %H:%M:%S', epoch) == '1970-01-01 00:00:00'
assert time.strftime('', epoch) == ''
assert time.strftime('%%Y literal', epoch) == '%Y literal'
# gmtime's offset is always zero; its zone *name* is platform-dependent on CPython
# (glibc says GMT), so that lives in datetime_format.rs
assert time.strftime('%z', epoch) == '+0000'
# tm_wday and tm_yday are printed as given, not recomputed from the date --
# this tuple claims Monday, day 1 of the year, for a date that is a Saturday
claimed = (2024, 6, 15, 12, 30, 0, 0, 1, 0)
assert time.strftime('%a|%A|%d|%m|%j|%w|%u|%U|%W', claimed) == 'Mon|Monday|15|06|001|1|1|00|01'
assert time.strftime('%a|%A|%j|%w|%u|%U|%W', mid) == 'Sat|Saturday|167|6|6|23|24'
assert time.asctime(claimed) == 'Mon Jun 15 12:30:00 2024'

# === strptime ===
parsed = time.strptime('2026-01-02', '%Y-%m-%d')
assert (parsed.tm_year, parsed.tm_mon, parsed.tm_mday) == (2026, 1, 2)
assert (parsed.tm_wday, parsed.tm_yday) == (4, 2)
# a format that sets no zone leaves tm_isdst unknown
assert parsed.tm_isdst == -1
# fields the format does not set come from 1900-01-01
timed = time.strptime('12:30', '%H:%M')
assert (timed.tm_year, timed.tm_mon, timed.tm_mday) == (1900, 1, 1)
assert (timed.tm_hour, timed.tm_min) == (12, 30)
assert time.strptime('45', '%M')[:6] == (1900, 1, 1, 0, 45, 0)
assert time.strptime('2024-06', '%Y-%m')[:6] == (2024, 6, 1, 0, 0, 0)
assert time.strptime('120', '%H20')[:6] == (1900, 1, 1, 1, 0, 0)
assert time.strptime('120', '%I20')[:6] == (1900, 1, 1, 1, 0, 0)
assert time.strptime('120', '%S20')[:6] == (1900, 1, 1, 0, 0, 1)
assert time.strptime('1120', '%H%I20')[:6] == (1900, 1, 1, 1, 0, 0)
assert time.strptime(' 1', '%H').tm_hour == 1
assert time.strptime(' 1', '%I').tm_hour == 1
# the default format is asctime's own, so the two round-trip
assert time.strptime(time.asctime(epoch))[:6] == epoch[:6]

# === strptime preserves leap seconds and uses Python's hour rules ===
for second in [60, 61]:
    leap = time.strptime(f'2024-01-01 23:59:{second}', '%Y-%m-%d %H:%M:%S')
    assert leap[:6] == (2024, 1, 1, 23, 59, second)
    assert (leap.tm_wday, leap.tm_yday) == (0, 1)
    assert time.asctime(leap) == f'Mon Jan  1 23:59:{second} 2024'
    # Windows CPython rejects leap seconds in strftime; Monty reports 'monty'.
    if sys.platform != 'win32':
        assert time.strftime('%Y-%m-%d %H:%M:%S', leap) == f'2024-01-01 23:59:{second}'
        assert time.strftime('%X', leap) == f'23:59:{second}'
    assert time.strptime(f'23:59:{second}.1', '%H:%M:%S.%f')[:6] == (1900, 1, 1, 23, 59, second)
    assert time.mktime(leap) == time.mktime((2024, 1, 2, 0, 0, second - 60, 1, 2, -1))
assert time.strptime('13 AM', '%H %p').tm_hour == 13
assert time.strptime('01 PM', '%H %p').tm_hour == 1
assert time.strptime('12', '%I').tm_hour == 0
assert time.strptime('PM 01', '%p %I').tm_hour == 13
assert time.strptime('2024-01-01 UTC,', '%Y-%m-%d %Z,')[:6] == (2024, 1, 1, 0, 0, 0)
check(
    lambda: time.strptime('2024-01-01 ARBITRARY', '%Y-%m-%d %Z'),
    "ValueError: time data '2024-01-01 ARBITRARY' does not match format '%Y-%m-%d %Z'",
)
check(
    lambda: time.strptime('2024-01-01 UTCextra', '%Y-%m-%d %Z'),
    'ValueError: unconverted data remains: extra',
)

# === round trips ===
assert time.mktime(time.localtime(1718451000)) == 1718451000.0
assert time.mktime(time.localtime(0)) == 0.0
assert time.strftime('%Y-%m-%d', time.strptime('2024-06-15', '%Y-%m-%d')) == '2024-06-15'

# === argument errors ===
check(lambda: time.gmtime(0, 1), 'TypeError: gmtime() takes at most 1 argument (2 given)')
check(lambda: time.gmtime(seconds=0), 'TypeError: gmtime() takes no keyword arguments')
check(lambda: time.localtime(0, 1), 'TypeError: localtime() takes at most 1 argument (2 given)')
check(lambda: time.ctime(0, 1), 'TypeError: ctime() takes at most 1 argument (2 given)')
check(lambda: time.asctime(epoch, epoch), 'TypeError: asctime expected at most 1 argument, got 2')
check(lambda: time.mktime(), 'TypeError: time.mktime() takes exactly one argument (0 given)')
check(lambda: time.strftime(), 'TypeError: strftime() takes at least 1 argument (0 given)')
check(lambda: time.strftime('%Y', epoch, 3), 'TypeError: strftime() takes at most 2 arguments (3 given)')
check(
    lambda: time.strptime('a', 'b', 'c'),
    'TypeError: _strptime_time() takes from 1 to 2 positional arguments but 3 were given',
)
check(lambda: time.strftime(5), 'TypeError: strftime() argument 1 must be str, not int')
check(lambda: time.gmtime('x'), "TypeError: 'str' object cannot be interpreted as an integer")
check(lambda: time.gmtime(float('nan')), 'ValueError: Invalid value NaN (not a number)')
check(lambda: time.gmtime(1e20), 'OverflowError: timestamp out of range for platform time_t')
check(lambda: time.localtime(1e20), 'OverflowError: timestamp out of range for platform time_t')

# === bad time tuples ===
check(lambda: time.asctime(None), 'TypeError: Tuple or struct_time argument required')
check(
    lambda: time.strftime('%Y', [2024, 6, 15, 12, 30, 0, 0, 1, 0]), 'TypeError: Tuple or struct_time argument required'
)
check(lambda: time.mktime((1, 2)), 'TypeError: mktime(): illegal time tuple argument')
check(
    lambda: time.strftime('%Y', (2024, 6, 15, 12, 30, 0, 0, 1, 0, 0)),
    'TypeError: strftime(): illegal time tuple argument',
)
check(lambda: time.strftime('%Y', (2024, 13, 15, 12, 30, 0, 0, 1, 0)), 'ValueError: month out of range')
check(lambda: time.asctime((2024, 13, 15, 12, 30, 0, 0, 1, 0)), 'ValueError: month out of range')
# tm_wday folds mod 7 from -1 up; tm_yday is 0..=366 with 0 printed as day 1.
# mktime reads only the wall clock and skips these checks entirely
assert time.strftime('%a', (2024, 1, 1, 0, 0, 0, -1, 1, -1)) == 'Sun'
assert time.strftime('%a', (2024, 1, 1, 0, 0, 0, 100, 1, -1)) == 'Wed'
assert time.strftime('%j', (2024, 1, 1, 0, 0, 0, 0, 0, -1)) == '001'
assert time.strftime('%j', (2024, 1, 1, 0, 0, 0, 0, 366, -1)) == '366'
check(lambda: time.strftime('%a', (2024, 1, 1, 0, 0, 0, -2, 1, -1)), 'ValueError: day of week out of range')
check(lambda: time.asctime((2024, 1, 1, 0, 0, 0, -2, 1, -1)), 'ValueError: day of week out of range')
check(lambda: time.strftime('%j', (2024, 1, 1, 0, 0, 0, 0, 367, -1)), 'ValueError: day of year out of range')
check(lambda: time.strftime('%Y', (2024, 1, 1, 0, 0, 0, 0, -5, -1)), 'ValueError: day of year out of range')
assert time.mktime((2024, 1, 1, 0, 0, 0, -2, 367, -1)) == time.mktime((2024, 1, 1, 0, 0, 0, 0, 1, -1))
check(lambda: time.strptime('nope', '%Y'), "ValueError: time data 'nope' does not match format '%Y'")
check(lambda: time.strptime('12:30 extra', '%H:%M'), 'ValueError: unconverted data remains:  extra')
check(lambda: time.strptime('120x', '%H20'), 'ValueError: unconverted data remains: x')
check(lambda: time.strptime('122020', '%H20'), 'ValueError: unconverted data remains: 20')
check(lambda: time.strptime(' 12', '%H'), 'ValueError: unconverted data remains: 2')
check(lambda: time.strptime(' 1', '%S'), "ValueError: time data ' 1' does not match format '%S'")
check(
    lambda: time.strptime('2024-06-15 +0100 extra', '%Y-%m-%d %z'),
    'ValueError: unconverted data remains:  extra',
)

# === strptime %j rolls into the next year ===
assert time.strptime('2023 366', '%Y %j')[:3] == (2024, 1, 1)

# === strftime leap seconds ===
# Windows CPython has no `%-S` or `%s`
if sys.platform != 'win32':
    # flagged forms of a leap second keep the 60
    assert time.strftime('%-S|%_S|%0S|%S', (2024, 1, 1, 23, 59, 60, 0, 1, 0)) == '60|60|60|60'
    assert time.strftime('%-S|%-M', (2024, 1, 1, 23, 5, 7, 0, 1, 0)) == '7|5'
    # and `%s` counts a leap second into the next minute, as `mktime` does
    assert time.strftime('%s|%-s|%0s', (2024, 1, 1, 23, 59, 60, 0, 1, 0)) == '1704153600|1704153600|1704153600'

# === a time tuple's year is a C int ===
# Windows CPython's 32-bit C `long` overflows first, with a different message
if sys.platform != 'win32':
    for _year, _message in [(2**31, 'greater than maximum'), (-(2**31) - 1, 'less than minimum')]:
        _tuple = (_year, 1, 1, 0, 0, 0, 0, 1, -1)
        check(lambda: time.mktime(_tuple), 'OverflowError: signed integer is ' + _message)
        check(lambda: time.asctime(_tuple), 'OverflowError: signed integer is ' + _message)
        check(lambda: time.strftime('%Y', _tuple), 'OverflowError: signed integer is ' + _message)
