# /// monty
# skip-cpython-windows = true
# ///
# CPython on Windows needs the `tzdata` package for IANA zones; both sides run in UTC.
import zoneinfo
from datetime import date, datetime, time, timedelta, timezone
from zoneinfo import ZoneInfo

# === ZoneInfo objects ===
_london = ZoneInfo('Europe/London')
assert repr(_london) == "zoneinfo.ZoneInfo(key='Europe/London')"
assert str(_london) == 'Europe/London'
assert _london.key == 'Europe/London'
assert _london == ZoneInfo('Europe/London')
assert _london != ZoneInfo('UTC')
assert _london != 'Europe/London'
assert hash(_london) == hash(ZoneInfo('Europe/London'))
assert isinstance(_london, ZoneInfo)
assert type(_london).__name__ == 'ZoneInfo'
assert repr(ZoneInfo('UTC')) == "zoneinfo.ZoneInfo(key='UTC')"
assert repr(ZoneInfo('Etc/UTC')) == "zoneinfo.ZoneInfo(key='Etc/UTC')"
assert repr(ZoneInfo(key='Asia/Tokyo')) == "zoneinfo.ZoneInfo(key='Asia/Tokyo')"
assert repr(ZoneInfo.no_cache('UTC')) == "zoneinfo.ZoneInfo(key='UTC')"
assert ZoneInfo.clear_cache() is None
assert ZoneInfo.clear_cache(only_keys=['UTC']) is None

# ZoneInfoNotFoundError is raised for unknown keys; it is a KeyError
try:
    ZoneInfo('Nope/Nope')
    assert False, 'expected ZoneInfoNotFoundError'
except zoneinfo.ZoneInfoNotFoundError as e:
    assert str(e) == "'No time zone found with key Nope/Nope'"
    assert isinstance(e, KeyError)
try:
    ZoneInfo('Nope/Nope')
    assert False, 'expected KeyError'
except KeyError as e:
    assert str(e) == "'No time zone found with key Nope/Nope'"

for _call, _exc, _msg in [
    (lambda: ZoneInfo(5), TypeError, 'expected str, bytes or os.PathLike object, not int'),
    (lambda: ZoneInfo(None), TypeError, 'expected str, bytes or os.PathLike object, not NoneType'),
    (
        lambda: ZoneInfo('../etc/passwd'),
        ValueError,
        'ZoneInfo keys must refer to subdirectories of TZPATH, got: ../etc/passwd',
    ),
    (lambda: ZoneInfo('/etc/localtime'), ValueError, 'ZoneInfo keys may not be absolute paths, got: /etc/localtime'),
    (
        lambda: ZoneInfo('Europe//London'),
        ValueError,
        'ZoneInfo keys must be normalized relative paths, got: Europe//London',
    ),
    (lambda: ZoneInfo(''), ValueError, 'ZoneInfo keys must be normalized relative paths, got: '),
    (lambda: ZoneInfo(), TypeError, "ZoneInfo() missing required argument 'key' (pos 1)"),
    (lambda: ZoneInfo('UTC', 1), TypeError, 'ZoneInfo() takes at most 1 argument (2 given)'),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === tzinfo methods ===
assert _london.utcoffset(None) is None
assert _london.tzname(None) is None
assert _london.dst(None) is None
assert _london.utcoffset(datetime(2024, 7, 1, 12)) == timedelta(hours=1)
assert _london.utcoffset(datetime(2024, 1, 1, 12)) == timedelta(0)
assert _london.tzname(datetime(2024, 7, 1, 12)) == 'BST'
assert _london.tzname(datetime(2024, 1, 1, 12)) == 'GMT'
assert _london.dst(datetime(2024, 7, 1, 12)) == timedelta(hours=1)
assert _london.dst(datetime(2024, 1, 1, 12)) == timedelta(0)
assert ZoneInfo('UTC').dst(datetime(2024, 1, 1)) == timedelta(0)
assert ZoneInfo('Asia/Kolkata').utcoffset(datetime(2024, 1, 1)) == timedelta(hours=5, minutes=30)
assert ZoneInfo('Asia/Kolkata').tzname(datetime(2024, 1, 1)) == 'IST'
assert ZoneInfo('America/New_York').tzname(datetime(2024, 7, 1)) == 'EDT'
assert ZoneInfo('America/New_York').utcoffset(datetime(2024, 7, 1)) == timedelta(hours=-4)
assert ZoneInfo('Australia/Sydney').utcoffset(datetime(2024, 1, 1)) == timedelta(hours=11)
# the argument's tzinfo is ignored: only its wall clock counts
assert _london.utcoffset(datetime(2024, 7, 1, 12, tzinfo=timezone.utc)) == timedelta(hours=1)
# a DST gap takes the offset from before the transition
assert _london.utcoffset(datetime(2024, 3, 31, 1, 30)) == timedelta(0)
assert _london.tzname(datetime(2024, 3, 31, 1, 30)) == 'GMT'
# historical local mean time is in the database too
assert ZoneInfo('Asia/Tokyo').utcoffset(datetime(1, 1, 1)) == timedelta(seconds=33539)
assert ZoneInfo('America/New_York').utcoffset(datetime(1800, 1, 1)) == timedelta(days=-1, seconds=68638)
assert repr(_london.fromutc(datetime(2024, 7, 1, 12, tzinfo=_london))) == (
    "datetime.datetime(2024, 7, 1, 13, 0, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
)

for _call, _exc, _msg in [
    (lambda: _london.utcoffset(5), AttributeError, "'int' object has no attribute 'toordinal'"),
    (lambda: _london.tzname('x'), AttributeError, "'str' object has no attribute 'toordinal'"),
    (lambda: _london.dst(date(2024, 1, 1)), AttributeError, "'datetime.date' object has no attribute 'hour'"),
    (lambda: _london.utcoffset(), TypeError, 'utcoffset() takes exactly 1 positional argument (0 given)'),
    (lambda: _london.utcoffset(None, None), TypeError, 'utcoffset() takes at most 1 argument (2 given)'),
    (lambda: _london.fromutc(datetime(2024, 7, 1, 12)), ValueError, 'fromutc: dt.tzinfo is not self'),
    (
        lambda: _london.fromutc(datetime(2024, 7, 1, 12, tzinfo=timezone.utc)),
        ValueError,
        'fromutc: dt.tzinfo is not self',
    ),
    (lambda: _london.fromutc(5), TypeError, 'fromutc: argument must be a datetime'),
    (lambda: _london.nope, AttributeError, "'zoneinfo.ZoneInfo' object has no attribute 'nope'"),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === datetime with a ZoneInfo tzinfo ===
_summer = datetime(2024, 7, 1, 12, 30, tzinfo=_london)
assert repr(_summer) == "datetime.datetime(2024, 7, 1, 12, 30, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
assert str(_summer) == '2024-07-01 12:30:00+01:00'
assert _summer.isoformat() == '2024-07-01T12:30:00+01:00'
assert _summer.utcoffset() == timedelta(hours=1)
assert _summer.tzname() == 'BST'
assert _summer.dst() == timedelta(hours=1)
assert _summer.tzinfo is _london
assert _summer.timestamp() == 1719833400.0
assert _summer == datetime(2024, 7, 1, 11, 30, tzinfo=timezone.utc)
assert hash(_summer) == hash(datetime(2024, 7, 1, 11, 30, tzinfo=timezone.utc))
assert _summer < datetime(2024, 7, 1, 11, 31, tzinfo=timezone.utc)
assert _summer.strftime('%Y-%m-%d %H:%M %Z %z') == '2024-07-01 12:30 BST +0100'
assert f'{_summer:%Z}' == 'BST'
assert {_summer: 1}[datetime(2024, 7, 1, 11, 30, tzinfo=timezone.utc)] == 1

_winter = datetime(2024, 1, 1, 12, 30, tzinfo=_london)
assert (_winter.utcoffset(), _winter.tzname(), _winter.dst()) == (timedelta(0), 'GMT', timedelta(0))
# one shared tzinfo object subtracts and compares by wall clock
assert _summer - _winter == timedelta(days=182)
assert datetime(2024, 7, 1, 12, tzinfo=_london) == datetime(2024, 7, 1, 12, tzinfo=ZoneInfo('Europe/London'))
assert datetime(2024, 7, 1, 12, tzinfo=_london) - datetime(2024, 1, 1, 12, tzinfo=timezone.utc) == timedelta(
    days=181, hours=23
)

# arithmetic moves the wall clock and re-resolves the offset
_moved = _summer + timedelta(days=180)
assert repr(_moved) == "datetime.datetime(2024, 12, 28, 12, 30, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
assert _moved.utcoffset() == timedelta(0)
assert _moved.tzinfo is _london
assert (_summer - timedelta(days=100)).tzname() == 'GMT'
_gap = datetime(2024, 3, 31, 0, 30, tzinfo=_london) + timedelta(hours=1)
assert repr(_gap) == "datetime.datetime(2024, 3, 31, 1, 30, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
assert _gap.utcoffset() == timedelta(0)
assert datetime(2024, 3, 31, 1, 30, tzinfo=_london).utcoffset() == timedelta(0)
# an ambiguous wall clock takes its first occurrence
assert datetime(2024, 10, 27, 1, 30, tzinfo=_london).utcoffset() == timedelta(hours=1)
assert datetime(2024, 10, 27, 1, 30, tzinfo=_london).timestamp() == 1729989000.0

# conversions
_ny = _summer.astimezone(ZoneInfo('America/New_York'))
assert repr(_ny) == "datetime.datetime(2024, 7, 1, 7, 30, tzinfo=zoneinfo.ZoneInfo(key='America/New_York'))"
assert _ny.tzname() == 'EDT'
assert repr(_summer.astimezone(timezone.utc)) == 'datetime.datetime(2024, 7, 1, 11, 30, tzinfo=datetime.timezone.utc)'
assert repr(datetime(2024, 7, 1, 12, tzinfo=timezone.utc).astimezone(_london)) == (
    "datetime.datetime(2024, 7, 1, 13, 0, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
)
assert repr(datetime(2024, 7, 1, 12).astimezone(_london)) == (
    "datetime.datetime(2024, 7, 1, 13, 0, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
)
assert repr(datetime.fromisoformat('2024-07-01T12:00:00+01:00').astimezone(_london)) == (
    "datetime.datetime(2024, 7, 1, 12, 0, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
)
assert repr(datetime(9999, 12, 31, 23, tzinfo=ZoneInfo('Asia/Tokyo')).astimezone(timezone.utc)) == (
    'datetime.datetime(9999, 12, 31, 14, 0, tzinfo=datetime.timezone.utc)'
)
assert repr(_summer.replace(tzinfo=ZoneInfo('Asia/Tokyo'))) == (
    "datetime.datetime(2024, 7, 1, 12, 30, tzinfo=zoneinfo.ZoneInfo(key='Asia/Tokyo'))"
)
assert _summer.replace(month=1).utcoffset() == timedelta(0)
assert repr(_summer.replace(tzinfo=None)) == 'datetime.datetime(2024, 7, 1, 12, 30)'
assert repr(datetime.combine(date(2024, 1, 1), time(12, 0), _london)) == (
    "datetime.datetime(2024, 1, 1, 12, 0, tzinfo=zoneinfo.ZoneInfo(key='Europe/London'))"
)
assert datetime.combine(date(2024, 1, 1), time(12, 0), _london).utcoffset() == timedelta(0)
assert datetime.now(_london).tzinfo is _london
assert datetime.now(tz=_london).utcoffset() in (timedelta(0), timedelta(hours=1))
assert type(datetime.now(_london)).__name__ == 'datetime'
assert datetime(1, 1, 1, tzinfo=ZoneInfo('Asia/Tokyo')).utcoffset() == timedelta(seconds=33539)
assert datetime(1900, 1, 1, tzinfo=_london).utcoffset() == timedelta(0)
assert sorted([datetime(2024, 7, 1, 12, tzinfo=_london), datetime(2024, 7, 1, 12, tzinfo=timezone.utc)]) == [
    datetime(2024, 7, 1, 12, tzinfo=_london),
    datetime(2024, 7, 1, 12, tzinfo=timezone.utc),
]
assert repr(_summer.time()) == 'datetime.time(12, 30)'

# === available_timezones ===
_zones = zoneinfo.available_timezones()
assert type(_zones).__name__ == 'set'
assert 'Europe/London' in _zones
assert 'UTC' in _zones
assert len(_zones) > 300

# === equal wall clocks of a repeated hour hash alike ===
_first = datetime(2024, 10, 27, 0, 30, tzinfo=timezone.utc).astimezone(_london)
_second = datetime(2024, 10, 27, 1, 30, tzinfo=timezone.utc).astimezone(_london)
assert _first == _second
assert hash(_first) == hash(_second)
assert hash(_first) == hash(datetime(2024, 10, 27, 1, 30, tzinfo=_london))
assert hash(_first) == hash(datetime(2024, 10, 27, 0, 30, tzinfo=timezone.utc))
assert len({_first, _second, datetime(2024, 10, 27, 1, 30, tzinfo=_london)}) == 1

# === a wall clock in a DST gap reports the saving of the period before it ===
assert datetime(2024, 3, 31, 1, 30, tzinfo=_london).dst() == timedelta(0)
assert _london.dst(datetime(2024, 3, 31, 1, 30)) == timedelta(0)
assert datetime(2024, 3, 31, 2, 30, tzinfo=_london).dst() == timedelta(hours=1)
