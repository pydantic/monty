# timezone=Asia/Kathmandu
# `Asia/Kathmandu` covers a fractional offset and no DST.
# Every expectation is CPython 3.14 under the same `TZ`.
import time
from datetime import datetime, timedelta, timezone

# === astimezone() on an aware value follows the instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 6, 15, 18, 15, tzinfo=datetime.timezone(datetime.timedelta(seconds=20700), '+0545'))"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 1, 15, 18, 15, tzinfo=datetime.timezone(datetime.timedelta(seconds=20700), '+0545'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone(timedelta(hours=-5))).astimezone())
    == "datetime.datetime(2024, 6, 15, 23, 15, tzinfo=datetime.timezone(datetime.timedelta(seconds=20700), '+0545'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().utcoffset())
    == 'datetime.timedelta(seconds=20700)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().tzname()) == "'+0545'"
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().dst()) == 'None'

# === astimezone() reads a naive value as session-local wall time ===
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 6, 45, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 1, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 1, 15, 6, 45, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone())
    == "datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=20700), '+0545'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone(timedelta(hours=2), 'X')))
    == "datetime.datetime(2024, 6, 15, 8, 45, tzinfo=datetime.timezone(datetime.timedelta(seconds=7200), 'X'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 6, 45, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 6, 15, 13, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 7, 45, tzinfo=datetime.timezone.utc)'
)

# === timestamp() reads the same zone ===
assert repr(datetime(2024, 6, 15, 12, 30).timestamp()) == '1718433900.0'
assert repr(datetime(2024, 1, 15, 12, 30).timestamp()) == '1705301100.0'
assert repr(datetime(2024, 6, 15, 12, 30, 15, 250000).timestamp()) == '1718433915.25'
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).timestamp()) == '1718454600.0'
assert repr(datetime(1970, 1, 1).timestamp()) == '-19800.0'
assert repr(datetime(1965, 7, 4, 6, 0).timestamp()) == '-141867000.0'
assert repr(datetime(2024, 6, 15, 12, 30).timestamp()) == '1718433900.0'
assert repr(datetime(2024, 6, 15, 13, 30).timestamp()) == '1718437500.0'

# === a round trip through the zone returns the same instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone.utc)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().timestamp()) == '1705321800.0'

# === %Z, %z and %:z come from the value zone, and are empty when naive ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Y-%m-%d %H:%M %Z %z %:z'))
    == "'2024-06-15 18:15 +0545 +0545 +05:45'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Z|%z|%:z'))
    == "'+0545|+0545|+05:45'"
)
assert repr(datetime(2024, 6, 15, 12, 30).strftime('[%Z][%z][%:z]')) == "'[][][]'"
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-06-15T18:15:00+05:45'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-01-15T18:15:00+05:45'"
)

# === the time module constants come from the zone ===
assert repr(time.timezone) == '-20700'
assert repr(time.altzone) == '-20700'
assert repr(time.daylight) == '0'
assert repr(time.tzname) == "('+0545', '+0545')"

# === the first and last representable day, where CPython solves out of range ===
try:
    datetime(9999, 12, 31, 12, 0).astimezone(timezone.utc)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'year must be in 1..9999, not 10000'
try:
    datetime(1, 1, 1, 12, 0).astimezone(timezone.utc)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'year must be in 1..9999, not 0'
try:
    datetime(1, 1, 1, 12, 0).timestamp()
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'year must be in 1..9999, not 0'
