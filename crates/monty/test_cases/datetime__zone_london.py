# timezone=Europe/London
# `Europe/London` covers northern DST, a near-zero offset and GMT/BST.
# Every expectation is CPython 3.14 under the same `TZ`.
import time
from datetime import datetime, timedelta, timezone

# === astimezone() on an aware value follows the instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 6, 15, 13, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'BST'))"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 1, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(0), 'GMT'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone(timedelta(hours=-5))).astimezone())
    == "datetime.datetime(2024, 6, 15, 18, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'BST'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().utcoffset())
    == 'datetime.timedelta(seconds=3600)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().tzname()) == "'GMT'"
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().dst()) == 'None'

# === astimezone() reads a naive value as session-local wall time ===
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 11, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 1, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 1, 15, 12, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone())
    == "datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'BST'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone(timedelta(hours=2), 'X')))
    == "datetime.datetime(2024, 6, 15, 13, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=7200), 'X'))"
)
assert (
    repr(datetime(2024, 10, 27, 1, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 10, 27, 0, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 3, 31, 1, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 3, 31, 1, 30, tzinfo=datetime.timezone.utc)'
)

# === timestamp() reads the same zone ===
assert repr(datetime(2024, 6, 15, 12, 30).timestamp()) == '1718451000.0'
assert repr(datetime(2024, 1, 15, 12, 30).timestamp()) == '1705321800.0'
assert repr(datetime(2024, 6, 15, 12, 30, 15, 250000).timestamp()) == '1718451015.25'
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).timestamp()) == '1718454600.0'
assert repr(datetime(1970, 1, 1).timestamp()) == '-3600.0'
assert repr(datetime(1965, 7, 4, 6, 0).timestamp()) == '-141850800.0'
assert repr(datetime(2024, 10, 27, 1, 30).timestamp()) == '1729989000.0'
assert repr(datetime(2024, 3, 31, 1, 30).timestamp()) == '1711848600.0'

# === a round trip through the zone returns the same instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone.utc)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().timestamp()) == '1705321800.0'

# === %Z, %z and %:z come from the value zone, and are empty when naive ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Y-%m-%d %H:%M %Z %z %:z'))
    == "'2024-06-15 13:30 BST +0100 +01:00'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Z|%z|%:z')) == "'GMT|+0000|+00:00'"
)
assert repr(datetime(2024, 6, 15, 12, 30).strftime('[%Z][%z][%:z]')) == "'[][][]'"
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-06-15T13:30:00+01:00'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-01-15T12:30:00+00:00'"
)

# === the time module constants come from the zone ===
assert repr(time.timezone) == '0'
assert repr(time.altzone) == '-3600'
assert repr(time.daylight) == '1'
assert repr(time.tzname) == "('GMT', 'BST')"

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

# === localtime() and mktime() read the same zone ===
summer = time.localtime(1718451000)
winter = time.localtime(1705321800)
assert (summer.tm_hour, summer.tm_isdst, summer.tm_zone, summer.tm_gmtoff) == (12, 1, 'BST', 3600)
assert (winter.tm_hour, winter.tm_isdst, winter.tm_zone, winter.tm_gmtoff) == (12, 0, 'GMT', 0)
assert time.strftime('%H:%M %Z %z', summer) == '12:30 BST +0100'
assert time.strftime('%H:%M %Z %z', winter) == '12:30 GMT +0000'
assert time.mktime(summer) == 1718451000.0
assert time.mktime(winter) == 1705321800.0
assert time.ctime(1718451000) == 'Sat Jun 15 12:30:00 2024'
# gmtime ignores the zone entirely
assert time.gmtime(1718451000).tm_hour == 11
assert time.strftime('%Z %z', time.gmtime(1718451000)) == 'UTC +0000'
