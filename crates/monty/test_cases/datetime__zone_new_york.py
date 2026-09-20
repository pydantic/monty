# timezone=America/New_York
# `America/New_York` covers west of UTC, EST/EDT.
# Every expectation is CPython 3.14 under the same `TZ`.
import time
from datetime import datetime, timedelta, timezone

# === astimezone() on an aware value follows the instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 6, 15, 8, 30, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=72000), 'EDT'))"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 1, 15, 7, 30, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=68400), 'EST'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone(timedelta(hours=-5))).astimezone())
    == "datetime.datetime(2024, 6, 15, 13, 30, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=72000), 'EDT'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().utcoffset())
    == 'datetime.timedelta(days=-1, seconds=72000)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().tzname()) == "'EST'"
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().dst()) == 'None'

# === astimezone() reads a naive value as session-local wall time ===
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 16, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 1, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 1, 15, 17, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone())
    == "datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=72000), 'EDT'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone(timedelta(hours=2), 'X')))
    == "datetime.datetime(2024, 6, 15, 18, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=7200), 'X'))"
)
assert (
    repr(datetime(2024, 11, 3, 1, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 11, 3, 5, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 3, 10, 2, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 3, 10, 7, 30, tzinfo=datetime.timezone.utc)'
)

# === timestamp() reads the same zone ===
assert repr(datetime(2024, 6, 15, 12, 30).timestamp()) == '1718469000.0'
assert repr(datetime(2024, 1, 15, 12, 30).timestamp()) == '1705339800.0'
assert repr(datetime(2024, 6, 15, 12, 30, 15, 250000).timestamp()) == '1718469015.25'
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).timestamp()) == '1718454600.0'
assert repr(datetime(1970, 1, 1).timestamp()) == '18000.0'
assert repr(datetime(1965, 7, 4, 6, 0).timestamp()) == '-141832800.0'
assert repr(datetime(2024, 11, 3, 1, 30).timestamp()) == '1730611800.0'
assert repr(datetime(2024, 3, 10, 2, 30).timestamp()) == '1710055800.0'

# === a round trip through the zone returns the same instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone.utc)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().timestamp()) == '1705321800.0'

# === %Z, %z and %:z come from the value zone, and are empty when naive ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Y-%m-%d %H:%M %Z %z %:z'))
    == "'2024-06-15 08:30 EDT -0400 -04:00'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Z|%z|%:z')) == "'EST|-0500|-05:00'"
)
assert repr(datetime(2024, 6, 15, 12, 30).strftime('[%Z][%z][%:z]')) == "'[][][]'"
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-06-15T08:30:00-04:00'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-01-15T07:30:00-05:00'"
)

# === the time module constants come from the zone ===
assert repr(time.timezone) == '18000'
assert repr(time.altzone) == '14400'
assert repr(time.daylight) == '1'
assert repr(time.tzname) == "('EST', 'EDT')"

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
