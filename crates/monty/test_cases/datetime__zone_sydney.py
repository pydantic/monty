# timezone=Australia/Sydney
# `Australia/Sydney` covers the southern hemisphere, so the `time` halves swap.
# Every expectation is CPython 3.14 under the same `TZ`.
import time
from datetime import datetime, timedelta, timezone

# === astimezone() on an aware value follows the instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 6, 15, 22, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=36000), 'AEST'))"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone())
    == "datetime.datetime(2024, 1, 15, 23, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=39600), 'AEDT'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone(timedelta(hours=-5))).astimezone())
    == "datetime.datetime(2024, 6, 16, 3, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=36000), 'AEST'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().utcoffset())
    == 'datetime.timedelta(seconds=36000)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().tzname()) == "'AEDT'"
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().dst()) == 'None'

# === astimezone() reads a naive value as session-local wall time ===
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 2, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 1, 15, 12, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 1, 15, 1, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone())
    == "datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=36000), 'AEST'))"
)
assert (
    repr(datetime(2024, 6, 15, 12, 30).astimezone(timezone(timedelta(hours=2), 'X')))
    == "datetime.datetime(2024, 6, 15, 4, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=7200), 'X'))"
)
assert (
    repr(datetime(2024, 4, 7, 2, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 4, 6, 15, 30, tzinfo=datetime.timezone.utc)'
)
assert (
    repr(datetime(2024, 10, 6, 2, 30).astimezone(timezone.utc))
    == 'datetime.datetime(2024, 10, 5, 16, 30, tzinfo=datetime.timezone.utc)'
)

# === timestamp() reads the same zone ===
assert repr(datetime(2024, 6, 15, 12, 30).timestamp()) == '1718418600.0'
assert repr(datetime(2024, 1, 15, 12, 30).timestamp()) == '1705282200.0'
assert repr(datetime(2024, 6, 15, 12, 30, 15, 250000).timestamp()) == '1718418615.25'
assert repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).timestamp()) == '1718454600.0'
assert repr(datetime(1970, 1, 1).timestamp()) == '-36000.0'
assert repr(datetime(1965, 7, 4, 6, 0).timestamp()) == '-141883200.0'
assert repr(datetime(2024, 4, 7, 2, 30).timestamp()) == '1712417400.0'
assert repr(datetime(2024, 10, 6, 2, 30).timestamp()) == '1728145800.0'

# === a round trip through the zone returns the same instant ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().astimezone(timezone.utc))
    == 'datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone.utc)'
)
assert repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().timestamp()) == '1705321800.0'

# === %Z, %z and %:z come from the value zone, and are empty when naive ===
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Y-%m-%d %H:%M %Z %z %:z'))
    == "'2024-06-15 22:30 AEST +1000 +10:00'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().strftime('%Z|%z|%:z')) == "'AEDT|+1100|+11:00'"
)
assert repr(datetime(2024, 6, 15, 12, 30).strftime('[%Z][%z][%:z]')) == "'[][][]'"
assert (
    repr(datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-06-15T22:30:00+10:00'"
)
assert (
    repr(datetime(2024, 1, 15, 12, 30, tzinfo=timezone.utc).astimezone().isoformat()) == "'2024-01-15T23:30:00+11:00'"
)

# === the time module constants come from the zone ===
assert repr(time.timezone) == '-36000'
assert repr(time.altzone) == '-39600'
assert repr(time.daylight) == '1'
assert repr(time.tzname) == "('AEST', 'AEDT')"

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
