# The `time` module's conversion functions, with explicit times so nothing
# depends on the clock. The zone-dependent halves live in datetime__zone_*.py.
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
# the default format is asctime's own, so the two round-trip
assert time.strptime(time.asctime(epoch))[:6] == epoch[:6]

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
