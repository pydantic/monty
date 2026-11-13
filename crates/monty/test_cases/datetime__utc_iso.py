# Every expectation is CPython 3.14. Its DeprecationWarning for utcfromtimestamp() and
# utcnow() has no Monty counterpart: Monty emits no warnings.
from datetime import date, datetime, timedelta, timezone


def _raises(func, *args, **kwargs):
    try:
        func(*args, **kwargs)
    except Exception as e:
        return type(e).__name__ + ': ' + str(e)
    return 'no error'


# === utcfromtimestamp() ===
assert repr(datetime.utcfromtimestamp(0)) == 'datetime.datetime(1970, 1, 1, 0, 0)'
assert repr(datetime.utcfromtimestamp(-1)) == 'datetime.datetime(1969, 12, 31, 23, 59, 59)'
assert repr(datetime.utcfromtimestamp(-1.5)) == 'datetime.datetime(1969, 12, 31, 23, 59, 58, 500000)'
assert repr(datetime.utcfromtimestamp(1.5)) == 'datetime.datetime(1970, 1, 1, 0, 0, 1, 500000)'
assert datetime.utcfromtimestamp(0).tzinfo is None
assert repr(datetime.utcfromtimestamp(True)) == 'datetime.datetime(1970, 1, 1, 0, 0, 1)'

# epoch milliseconds from an API, divided down
assert repr(datetime.utcfromtimestamp(1762546800123 / 1000)) == 'datetime.datetime(2025, 11, 7, 20, 20, 0, 123000)'
assert repr(datetime.utcfromtimestamp(1762546800000 / 1000)) == 'datetime.datetime(2025, 11, 7, 20, 20)'
assert repr(datetime.utcfromtimestamp(1e12 / 1000)) == 'datetime.datetime(2001, 9, 9, 1, 46, 40)'

# microseconds round half to even
assert datetime.utcfromtimestamp(0.0000005).microsecond == 0
assert datetime.utcfromtimestamp(0.0000015).microsecond == 2
assert datetime.utcfromtimestamp(0.0000025).microsecond == 2
assert datetime.utcfromtimestamp(-0.0000005) == datetime(1970, 1, 1)
assert datetime.utcfromtimestamp(1762546800.1234565).microsecond == 123456
assert datetime.utcfromtimestamp(1762546800.1234575).microsecond == 123457
assert datetime.utcfromtimestamp(86399.9999996) == datetime(1970, 1, 2)


class _Index:
    def __index__(self):
        return 86400


assert datetime.utcfromtimestamp(_Index()) == datetime(1970, 1, 2)


# usable through an instance, as a classmethod is
assert datetime(2020, 1, 1).utcfromtimestamp(5) == datetime(1970, 1, 1, 0, 0, 5)

# arguments
assert (
    _raises(lambda: datetime.utcfromtimestamp()) == 'TypeError: utcfromtimestamp() takes exactly 1 argument (0 given)'
)
assert (
    _raises(lambda *a, **k: datetime.utcfromtimestamp(*a, **k), 1, 2)
    == 'TypeError: utcfromtimestamp() takes exactly 1 argument (2 given)'
)
assert (
    _raises(lambda *a, **k: datetime.utcfromtimestamp(*a, **k), timestamp=1)
    == 'TypeError: utcfromtimestamp() takes no keyword arguments'
)

# === utcnow() ===
_now = datetime.utcnow()
assert _now.tzinfo is None
assert _now.year >= 2024
assert abs(datetime.now(timezone.utc).replace(tzinfo=None) - _now) < timedelta(minutes=5)
assert _raises(lambda *a: datetime.utcnow(*a), 1) == 'TypeError: datetime.utcnow() takes no arguments (1 given)'

# === isocalendar() ===
_iso = date(2026, 11, 7).isocalendar()
assert repr(_iso) == 'datetime.IsoCalendarDate(year=2026, week=45, weekday=6)'
assert _iso == (2026, 45, 6)
assert tuple(_iso) == (2026, 45, 6)
assert len(_iso) == 3
assert (_iso.year, _iso.week, _iso.weekday) == (2026, 45, 6)
assert _iso[0] == 2026 and _iso[-1] == 6
year, week, weekday = _iso
assert (year, week, weekday) == (2026, 45, 6)
assert _raises(lambda: date(2026, 11, 7).isocalendar(1)) == 'TypeError: date.isocalendar() takes no arguments (1 given)'

# year boundaries
for _day, _expected in [
    (date(2026, 12, 31), (2026, 53, 4)),
    (date(2027, 1, 1), (2026, 53, 5)),
    (date(2027, 1, 3), (2026, 53, 7)),
    (date(2027, 1, 4), (2027, 1, 1)),
    (date(2020, 12, 31), (2020, 53, 4)),
    (date(2021, 1, 3), (2020, 53, 7)),
    (date(2008, 12, 29), (2009, 1, 1)),
    (date(2018, 12, 31), (2019, 1, 1)),
    (date(2016, 1, 1), (2015, 53, 5)),
    (date(1, 1, 1), (1, 1, 1)),
    (date(9999, 12, 31), (9999, 52, 5)),
]:
    assert tuple(_day.isocalendar()) == _expected
    assert tuple(datetime(_day.year, _day.month, _day.day, 23, 59).isocalendar()) == _expected
assert repr(datetime(2026, 12, 31, 5, tzinfo=timezone.utc).isocalendar()) == (
    'datetime.IsoCalendarDate(year=2026, week=53, weekday=4)'
)
assert (
    _raises(lambda: datetime(2026, 1, 1).isocalendar(1)) == 'TypeError: date.isocalendar() takes no arguments (1 given)'
)

# === fromisocalendar() ===
assert repr(date.fromisocalendar(2026, 45, 6)) == 'datetime.date(2026, 11, 7)'
assert repr(datetime.fromisocalendar(2026, 45, 6)) == 'datetime.datetime(2026, 11, 7, 0, 0)'
assert date.fromisocalendar(2020, 53, 7) == date(2021, 1, 3)
assert date.fromisocalendar(2026, 53, 4) == date(2026, 12, 31)
assert date.fromisocalendar(2009, 1, 1) == date(2008, 12, 29)
assert date.fromisocalendar(1, 1, 1) == date(1, 1, 1)
assert date.fromisocalendar(9999, 52, 5) == date(9999, 12, 31)
assert date.fromisocalendar(year=2026, week=1, day=1) == date(2025, 12, 29)
assert date.fromisocalendar(2026, 1, day=1) == date(2025, 12, 29)
assert date.fromisocalendar(True, 1, 1) == date(1, 1, 1)
assert date(2020, 1, 1).fromisocalendar(2026, 45, 6) == date(2026, 11, 7)
for _day in [date(2026, 12, 31), date(2027, 1, 1), date(2020, 12, 31), date(2021, 1, 3), date(2008, 12, 29)]:
    assert date.fromisocalendar(*_day.isocalendar()) == _day

for _args, _msg in [
    ((2021, 53, 1), 'ValueError: Invalid week: 53'),
    ((2026, 54, 1), 'ValueError: Invalid week: 54'),
    ((2026, 0, 1), 'ValueError: Invalid week: 0'),
    ((2026, -1, 1), 'ValueError: Invalid week: -1'),
    ((2026, 1, 0), 'ValueError: Invalid weekday: 0 (range is [1, 7])'),
    ((2026, 1, 8), 'ValueError: Invalid weekday: 8 (range is [1, 7])'),
    ((2026, 0, 0), 'ValueError: Invalid week: 0'),
    ((0, 1, 1), 'ValueError: year must be in 1..9999, not 0'),
    ((-1, 1, 1), 'ValueError: year must be in 1..9999, not -1'),
    ((10000, 1, 1), 'ValueError: year must be in 1..9999, not 10000'),
    ((9999, 53, 1), 'ValueError: Invalid week: 53'),
    ((9999, 52, 7), 'ValueError: year must be in 1..9999, not 10000'),
    ((2**40, 1, 1), 'ValueError: ISO calendar component out of range'),
    ((2026, 2**40, 1), 'ValueError: ISO calendar component out of range'),
    ((2026, 1, 2**40), 'ValueError: ISO calendar component out of range'),
    ((1.5, 1, 1), "TypeError: 'float' object cannot be interpreted as an integer"),
    ((2026, '1', 1), "TypeError: 'str' object cannot be interpreted as an integer"),
    ((2026, 1), "TypeError: fromisocalendar() missing required argument 'day' (pos 3)"),
    ((2026, 1, 1, 1), 'TypeError: fromisocalendar() takes at most 3 arguments (4 given)'),
]:
    assert _raises(lambda *a: date.fromisocalendar(*a), *_args) == _msg
    assert _raises(lambda *a: datetime.fromisocalendar(*a), *_args) == _msg

# === timetuple() ===
_tt = date(2026, 11, 7).timetuple()
assert (_tt.tm_year, _tt.tm_mon, _tt.tm_mday, _tt.tm_hour, _tt.tm_min, _tt.tm_sec) == (2026, 11, 7, 0, 0, 0)
assert (_tt.tm_wday, _tt.tm_yday, _tt.tm_isdst) == (5, 311, -1)
_tt = datetime(2024, 12, 31, 1, 2, 3, 5, tzinfo=timezone.utc).timetuple()
assert (_tt.tm_year, _tt.tm_mon, _tt.tm_mday, _tt.tm_hour, _tt.tm_min, _tt.tm_sec) == (2024, 12, 31, 1, 2, 3)
assert (_tt.tm_wday, _tt.tm_yday, _tt.tm_isdst) == (1, 366, -1)
assert datetime(2026, 11, 7, 1, 2, 3).timetuple().tm_isdst == -1
assert tuple(date(2026, 11, 7).timetuple())[:9] == (2026, 11, 7, 0, 0, 0, 5, 311, -1)
assert _raises(lambda: date(2026, 1, 1).timetuple(1)) == 'TypeError: date.timetuple() takes no arguments (1 given)'
