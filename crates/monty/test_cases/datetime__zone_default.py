# /// monty
# skip-cpython-windows = true
# ///
# A case with no `timezone` header key: Monty's default session zone is UTC, and the
# harness puts the CPython side in UTC to match. Windows has no `time.tzset`, so the
# harness cannot move CPython there and only the Monty side of this case runs.
# Every expectation is CPython 3.14 under `TZ=UTC`.
import time
from datetime import date, datetime, timedelta, timezone

# === the zone is UTC, named ===
_local = datetime(2024, 6, 15, 12, 30).astimezone()
assert repr(_local) == "datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(0), 'UTC'))"
assert _local.utcoffset() == timedelta(0)
assert _local.tzname() == 'UTC'
assert _local.dst() is None

# === a naive value is already UTC, so converting moves nothing ===
assert repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc)) == (
    'datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone.utc)'
)
assert repr(datetime(2024, 1, 15, 12, 30).astimezone(timezone.utc)) == (
    'datetime.datetime(2024, 1, 15, 12, 30, tzinfo=datetime.timezone.utc)'
)
assert repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone(timedelta(hours=2), 'X'))) == (
    "datetime.datetime(2024, 6, 15, 14, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=7200), 'X'))"
)
# an aware value follows the instant back to UTC
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone(timedelta(hours=-5))).astimezone()) == (
    "datetime.datetime(2024, 6, 15, 17, 30, tzinfo=datetime.timezone(datetime.timedelta(0), 'UTC'))"
)

# === a naive timestamp() reads the same zone, so there is no offset to apply ===
assert repr(datetime(2024, 6, 15, 12, 30).timestamp()) == '1718454600.0'
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).timestamp()) == '1718454600.0'
assert repr(datetime(2024, 6, 15, 12, 30, 15, 250000).timestamp()) == '1718454615.25'
assert repr(datetime(1970, 1, 1).timestamp()) == '0.0'
assert repr(datetime(1965, 7, 4, 6, 0).timestamp()) == '-141847200.0'

# === %Z, %z and %:z come from the value zone, and are empty when naive ===
assert repr(_local.strftime('%Y-%m-%d %H:%M %Z %z %:z')) == "'2024-06-15 12:30 UTC +0000 +00:00'"
assert repr(datetime(2024, 6, 15, 12, 30).strftime('[%Z][%z][%:z]')) == "'[][][]'"
assert repr(_local.isoformat()) == "'2024-06-15T12:30:00+00:00'"

# === the time module constants come from the zone ===
assert repr(time.timezone) == '0'
assert repr(time.altzone) == '0'
assert repr(time.daylight) == '0'
assert repr(time.tzname) == "('UTC', 'UTC')"

# === a naive fromtimestamp() is the zone's wall clock ===
assert datetime.fromtimestamp(1700000000) == datetime(2023, 11, 14, 22, 13, 20)
assert datetime.fromtimestamp(1700000000.123456789) == datetime(2023, 11, 14, 22, 13, 20, 123457)
assert datetime.fromtimestamp(True) == datetime(1970, 1, 1, 0, 0, 1)
assert date.fromtimestamp(1700000000) == date(2023, 11, 14)
assert date.fromtimestamp(1700000000.9) == date(2023, 11, 14)
# `date` floors the fraction where `datetime` rounds it
assert date.fromtimestamp(86399.9999996) == date(1970, 1, 1)
assert datetime.fromtimestamp(86399.9999996) == datetime(1970, 1, 2)


class _Index:
    def __index__(self):
        return 86400


assert datetime.fromtimestamp(_Index()) == datetime(1970, 1, 2)
assert date.fromtimestamp(_Index()) == date(1970, 1, 2)

# === instants that Windows CPython's localtime() rejects ===
assert datetime.fromtimestamp(-1.5) == datetime(1969, 12, 31, 23, 59, 58, 500000)
assert datetime.fromtimestamp(-0.0000005) == datetime(1970, 1, 1)
assert datetime.fromtimestamp(253402300799) == datetime(9999, 12, 31, 23, 59, 59)
assert datetime.fromtimestamp(-62135596800 + 86400) == datetime(1, 1, 2)
assert date.fromtimestamp(-62135596800) == date(1, 1, 1)
assert date.fromtimestamp(253402300799) == date(9999, 12, 31)
# `date` floors the fraction where `datetime` rounds it
assert date.fromtimestamp(-5e-07) == date(1969, 12, 31)
assert date.fromtimestamp(-0.5) == date(1969, 12, 31)
assert datetime.fromtimestamp(-5e-07) == datetime(1970, 1, 1)

# the UTC instant is range-checked before a fixed offset shifts it
_plus_one = timezone(timedelta(hours=1))
_minus_one = timezone(timedelta(hours=-1))
for _bad, _exc, _msg in [
    ((253402300800,), ValueError, 'year must be in 1..9999, not 10000'),
    ((-62135596800,), ValueError, 'year must be in 1..9999, not 0'),
    ((-62135598600, _plus_one), ValueError, 'year must be in 1..9999, not 0'),
    ((253402300799, _plus_one), OverflowError, 'date value out of range'),
    ((-62135596800, _minus_one), OverflowError, 'date value out of range'),
    ((253402302599, _minus_one), ValueError, 'year must be in 1..9999, not 10000'),
]:
    try:
        datetime.fromtimestamp(*_bad)
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg
assert datetime.fromtimestamp(-62135596800, _plus_one) == datetime(1, 1, 1, 1, tzinfo=_plus_one)

try:
    date.fromtimestamp(-62135596801)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'year must be in 1..9999, not 0'
