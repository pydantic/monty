# skip-cpython-windows
# A case with no `# timezone=` marker: Monty's default session zone is UTC, and the
# harness puts the CPython side in UTC to match. Windows has no `time.tzset`, so the
# harness cannot move CPython there and only the Monty side of this case runs.
# Every expectation is CPython 3.14 under `TZ=UTC`.
import time
from datetime import datetime, timedelta, timezone

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
