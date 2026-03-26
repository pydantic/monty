# call-external
import datetime

# === now/today from deterministic OS callback ===
today = datetime.date.today()
assert isinstance(today, datetime.date), 'date.today() should return a date instance'

now_local = datetime.datetime.now()
assert isinstance(now_local, datetime.datetime), 'datetime.now() should return a datetime instance'
assert now_local.tzinfo is None, 'datetime.now() without tz should return a naive datetime'
assert str(now_local).startswith(str(today)), 'datetime.now() and date.today() should agree on the local calendar date'

now_utc = datetime.datetime.now(datetime.timezone.utc)
assert now_utc.tzinfo is datetime.timezone.utc, 'datetime.now(timezone.utc) should return an aware UTC datetime'

plus_two = datetime.timezone(datetime.timedelta(hours=2))
now_plus_two = datetime.datetime.now(plus_two)
assert now_plus_two.tzinfo == plus_two, 'datetime.now() with fixed offset should preserve the offset timezone'
named_plus_two = datetime.timezone(datetime.timedelta(hours=2), 'PLUS2')
now_named_plus_two = datetime.datetime.now(named_plus_two)
assert now_named_plus_two.tzinfo == named_plus_two, (
    'datetime.now() should preserve explicit timezone offsets on named fixed-offset tzinfo'
)
# TODO(datetime.now): preserve `tzinfo is input_tz` by threading the original tz
# object through OS-call resume instead of reconstructing from offset/name only.

# === repr/str parity ===
assert repr(datetime.date(2024, 1, 15)) == 'datetime.date(2024, 1, 15)', 'date repr should match CPython'
assert str(datetime.date(2024, 1, 15)) == '2024-01-15', 'date str should match CPython'
assert repr(datetime.datetime(2024, 1, 15, 10, 30)) == 'datetime.datetime(2024, 1, 15, 10, 30)', (
    'datetime repr should omit trailing zero fields'
)
assert str(datetime.datetime(2024, 1, 15, 10, 30)) == '2024-01-15 10:30:00', 'datetime str should include seconds'
assert repr(datetime.timedelta(days=1, seconds=3600)) == 'datetime.timedelta(days=1, seconds=3600)', (
    'timedelta repr should match CPython'
)
assert str(datetime.timedelta(days=1, seconds=3600)) == '1 day, 1:00:00', 'timedelta str should match CPython'
assert repr(datetime.timezone.utc) == 'datetime.timezone.utc', 'timezone.utc repr should match CPython'
assert datetime.timezone.utc is datetime.timezone.utc, 'timezone.utc should be a singleton identity value'
assert datetime.timezone(datetime.timedelta(0)) is datetime.timezone.utc, (
    'timezone(timedelta(0)) should return the timezone.utc singleton'
)
# TODO(timezone): add explicit regression for `timezone(timedelta(...), None)`
# raising TypeError (explicit `None` name differs from omitted name).
assert (
    repr(datetime.timezone(datetime.timedelta(seconds=3600))) == 'datetime.timezone(datetime.timedelta(seconds=3600))'
), 'timezone repr should match CPython'
assert str(datetime.timezone(datetime.timedelta(seconds=61))) == 'UTC+00:01:01', (
    'timezone str should include second-level offsets'
)
assert (
    repr(datetime.timezone(datetime.timedelta(seconds=-1)))
    == 'datetime.timezone(datetime.timedelta(days=-1, seconds=86399))'
), 'timezone repr should normalize negative second offsets like CPython'
assert (
    repr(datetime.timezone(datetime.timedelta(hours=1), 'A'))
    == "datetime.timezone(datetime.timedelta(seconds=3600), 'A')"
), 'timezone repr should use Python string quoting for custom names'
assert str(datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(seconds=61)))) == (
    '2024-01-01 00:00:00+00:01:01'
), 'datetime str should include second-level offsets'
assert repr(datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(seconds=-1)))) == (
    'datetime.datetime(2024, 1, 1, 0, 0, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=86399)))'
), 'datetime repr should use normalized negative timezone offsets'
named_tz = datetime.timezone(datetime.timedelta(hours=1), 'X')
named_dt = datetime.datetime(2024, 1, 1, tzinfo=named_tz)
assert repr(named_dt) == (
    "datetime.datetime(2024, 1, 1, 0, 0, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'X'))"
), 'datetime repr should preserve explicit timezone names'
assert repr(named_dt.tzinfo) == "datetime.timezone(datetime.timedelta(seconds=3600), 'X')", (
    'datetime.tzinfo should preserve explicit timezone names'
)

# === tzinfo identity semantics ===
identity_tz = datetime.timezone(datetime.timedelta(hours=1), 'IDENTITY')
identity_dt = datetime.datetime(2024, 1, 1, 12, 0, 0, tzinfo=identity_tz)
assert identity_dt.tzinfo is identity_tz, 'aware datetime should preserve input tzinfo identity'
assert identity_dt.tzinfo is identity_dt.tzinfo, 'datetime.tzinfo should be stable across repeated attribute access'
assert (identity_dt + datetime.timedelta(seconds=1)).tzinfo is identity_tz, (
    'datetime arithmetic should preserve aware datetime tzinfo identity'
)

# === arithmetic ===
assert datetime.date(2024, 1, 10) + datetime.timedelta(days=5) == datetime.date(2024, 1, 15), (
    'date + timedelta should add days'
)
assert datetime.date(2024, 1, 10) - datetime.timedelta(days=5) == datetime.date(2024, 1, 5), (
    'date - timedelta should subtract days'
)
assert datetime.date(2024, 1, 10) - datetime.date(2024, 1, 1) == datetime.timedelta(days=9), (
    'date - date should return timedelta'
)

base_dt = datetime.datetime(2024, 1, 10, 12, 0, 0)
assert base_dt + datetime.timedelta(hours=2) == datetime.datetime(2024, 1, 10, 14, 0, 0), (
    'datetime + timedelta should add duration'
)
assert base_dt - datetime.timedelta(hours=2) == datetime.datetime(2024, 1, 10, 10, 0, 0), (
    'datetime - timedelta should subtract duration'
)
assert datetime.datetime(2024, 1, 10, 12, 0, 0) - datetime.datetime(2024, 1, 10, 11, 0, 0) == datetime.timedelta(
    hours=1
), 'datetime - datetime should return timedelta'

assert datetime.timedelta(days=1, seconds=10) + datetime.timedelta(seconds=5) == datetime.timedelta(
    days=1, seconds=15
), 'timedelta + timedelta should add'
assert datetime.timedelta(days=1, seconds=10) - datetime.timedelta(seconds=5) == datetime.timedelta(
    days=1, seconds=5
), 'timedelta - timedelta should subtract'
assert -datetime.timedelta(days=1, seconds=30) == datetime.timedelta(days=-2, seconds=86370), (
    'unary -timedelta should normalize like CPython'
)
assert -datetime.timedelta(0) == datetime.timedelta(0), 'negation of zero timedelta'
assert -datetime.timedelta(days=-1) == datetime.timedelta(days=1), 'double negation of timedelta'
assert datetime.timedelta(hours=1, minutes=30).total_seconds() == 5400.0, (
    'timedelta.total_seconds() should match CPython'
)

# === aware/naive comparison and subtraction rules ===
aware = datetime.datetime(2024, 1, 1, 12, 0, 0, tzinfo=datetime.timezone.utc)
naive = datetime.datetime(2024, 1, 1, 12, 0, 0)

assert (aware == naive) is False, 'aware == naive should be False, not an exception'
assert (aware != naive) is True, 'aware != naive should be True, not an exception'

# TODO(datetime): restore once compare/subtract error semantics are finalized without VM-specific branching.
# try:
#     aware < naive
#     assert False, 'aware < naive should raise TypeError'
# except TypeError as e:
#     assert str(e) == "can't compare offset-naive and offset-aware datetimes", (
#         'aware/naive ordering message should match CPython'
#     )
#
# try:
#     1 > 'x'
#     assert False, 'int > str should raise TypeError'
# except TypeError as e:
#     assert str(e) == "'>' not supported between instances of 'int' and 'str'", (
#         'ordering TypeError should include the actual operator'
#     )
#
# try:
#     aware - naive
#     assert False, 'aware - naive should raise TypeError'
# except TypeError as e:
#     assert str(e) == "can't subtract offset-naive and offset-aware datetimes", (
#         'aware/naive subtraction message should match CPython'
#     )

# === timezone validations and constant ===
assert datetime.timezone.utc == datetime.timezone(datetime.timedelta(0)), (
    'timezone.utc should equal zero offset timezone'
)
# TODO(timezone): add a GC-stability regression ensuring `timezone.utc` identity
# persists after allocation/collection cycles.
assert datetime.timezone(offset=datetime.timedelta(hours=1)) == datetime.timezone(datetime.timedelta(hours=1)), (
    'timezone constructor should support the offset keyword'
)
assert datetime.timezone(datetime.timedelta(hours=1), name='A') == datetime.timezone(
    datetime.timedelta(hours=1), 'A'
), 'timezone constructor should support the name keyword'
assert datetime.timezone(datetime.timedelta(hours=1), 'A') == datetime.timezone(datetime.timedelta(hours=1), 'B'), (
    'timezone equality should depend on offset, not name'
)
assert hash(datetime.timezone(datetime.timedelta(hours=1), 'A')) == hash(
    datetime.timezone(datetime.timedelta(hours=1), 'B')
), 'timezone hash should depend on offset, not name'
assert repr(datetime.timezone(datetime.timedelta(seconds=1))) == 'datetime.timezone(datetime.timedelta(seconds=1))', (
    'timezone should allow second-level fixed offsets'
)

try:
    datetime.timezone(datetime.timedelta(hours=24))
    assert False, 'timezone offset at 24 hours should raise ValueError'
except ValueError as e:
    assert str(e) == (
        'offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), '
        'not datetime.timedelta(days=1)'
    ), 'timezone range validation message should match CPython'

# === duplicate argument bindings ===
try:
    datetime.datetime(2024, 1, 1, 1, hour=2)
    assert False, 'datetime constructor should reject positional+keyword duplicate hour'
except TypeError as e:
    assert str(e) in {
        "datetime() got multiple values for argument 'hour'",
        "datetime() got multiple values for keyword argument 'hour'",
        "argument for function given by name ('hour') and position (4)",
    }, 'datetime duplicate hour should raise CPython-style duplicate-binding TypeError'

try:
    datetime.datetime(2024, 1, 1, 0, 0, 0, 0, datetime.timezone.utc, tzinfo=datetime.timezone.utc)
    assert False, 'datetime constructor should reject positional+keyword duplicate tzinfo'
except TypeError as e:
    assert str(e) in {
        "datetime() got multiple values for argument 'tzinfo'",
        "datetime() got multiple values for keyword argument 'tzinfo'",
        "argument for function given by name ('tzinfo') and position (8)",
    }, 'datetime duplicate tzinfo should raise CPython-style duplicate-binding TypeError'

try:
    datetime.timezone(datetime.timedelta(hours=1), offset=datetime.timedelta(hours=1))
    assert False, 'timezone constructor should reject positional+keyword duplicate offset'
except TypeError as e:
    assert str(e) in {
        "timezone() got multiple values for argument 'offset'",
        "timezone() got multiple values for keyword argument 'offset'",
        "argument for timezone() given by name ('offset') and position (1)",
    }, 'timezone duplicate offset should raise duplicate-binding TypeError'

try:
    datetime.timezone(datetime.timedelta(hours=1), 'A', name='B')
    assert False, 'timezone constructor should reject 3 arguments even when name is also provided by keyword'
except TypeError as e:
    assert str(e) in {
        'timezone expected at most 2 arguments, got 3',
        'timezone() takes at most 2 arguments (3 given)',
        "timezone() got multiple values for argument 'name'",
        "timezone() got multiple values for keyword argument 'name'",
        "argument for timezone() given by name ('name') and position (2)",
    }, 'timezone constructor should reject positional+keyword duplicate name or 3-argument over-binding'

# TODO(datetime): restore once overflow paths are finalized without VM-specific binary fallback branches.
# try:
#     datetime.date(1, 1, 1) - datetime.timedelta(days=1)
#     assert False, 'date underflow should raise OverflowError'
# except OverflowError as e:
#     assert str(e) == 'date value out of range', 'date underflow should match CPython overflow message'
#
# try:
#     datetime.datetime(9999, 12, 31, 23, 59, 59, 999999) + datetime.timedelta(microseconds=1)
#     assert False, 'datetime overflow should raise OverflowError'
# except OverflowError as e:
#     assert str(e) == 'date value out of range', 'datetime overflow should match CPython overflow message'
#
# try:
#     datetime.timedelta(days=999999999) + datetime.timedelta(days=1)
#     assert False, 'timedelta addition overflow should raise OverflowError'
# except OverflowError as e:
#     assert str(e) == 'days=1000000000; must have magnitude <= 999999999', (
#         'timedelta overflow should report the overflowing days value'
#     )

# === attribute access ===

d = datetime.date(2024, 2, 29)
assert d.year == 2024, 'date.year should return year'
assert d.month == 2, 'date.month should return month'
assert d.day == 29, 'date.day should return day'

d_boundary = datetime.date(1, 1, 1)
assert d_boundary.year == 1, 'date.year at minimum boundary'
assert d_boundary.month == 1, 'date.month at minimum boundary'
assert d_boundary.day == 1, 'date.day at minimum boundary'

d_max = datetime.date(9999, 12, 31)
assert d_max.year == 9999, 'date.year at maximum boundary'
assert d_max.month == 12, 'date.month at maximum boundary'
assert d_max.day == 31, 'date.day at maximum boundary'

dt = datetime.datetime(2024, 6, 15, 14, 30, 45, 123456)
assert dt.year == 2024, 'datetime.year should return year'
assert dt.month == 6, 'datetime.month should return month'
assert dt.day == 15, 'datetime.day should return day'
assert dt.hour == 14, 'datetime.hour should return hour'
assert dt.minute == 30, 'datetime.minute should return minute'
assert dt.second == 45, 'datetime.second should return second'
assert dt.microsecond == 123456, 'datetime.microsecond should return microsecond'

dt_zero = datetime.datetime(2024, 1, 1, 0, 0, 0, 0)
assert dt_zero.hour == 0, 'datetime.hour should return 0 for midnight'
assert dt_zero.microsecond == 0, 'datetime.microsecond should return 0'

td = datetime.timedelta(days=5, seconds=3600, microseconds=500)
assert td.days == 5, 'timedelta.days should return days'
assert td.seconds == 3600, 'timedelta.seconds should return seconds'
assert td.microseconds == 500, 'timedelta.microseconds should return microseconds'

td_zero = datetime.timedelta(0)
assert td_zero.days == 0, 'zero timedelta.days'
assert td_zero.seconds == 0, 'zero timedelta.seconds'
assert td_zero.microseconds == 0, 'zero timedelta.microseconds'

td_neg = datetime.timedelta(days=-1)
assert td_neg.days == -1, 'negative timedelta.days'
assert td_neg.seconds == 0, 'negative timedelta.seconds'
assert td_neg.microseconds == 0, 'negative timedelta.microseconds'

td_mixed_neg = datetime.timedelta(seconds=-1)
assert td_mixed_neg.days == -1, 'timedelta(-1s).days should be -1 (normalized)'
assert td_mixed_neg.seconds == 86399, 'timedelta(-1s).seconds should be 86399 (normalized)'
assert td_mixed_neg.microseconds == 0, 'timedelta(-1s).microseconds should be 0'

# === edge cases: repr and str ===

assert repr(datetime.timedelta(0)) == 'datetime.timedelta(0)', 'zero timedelta repr'
assert str(datetime.timedelta(0)) == '0:00:00', 'zero timedelta str'
assert str(datetime.timedelta(days=-1)) == '-1 day, 0:00:00', 'negative day timedelta str'
assert str(datetime.timedelta(days=1)) == '1 day, 0:00:00', 'singular day timedelta str'
assert str(datetime.timedelta(days=2)) == '2 days, 0:00:00', 'plural days timedelta str'
assert repr(datetime.date(2024, 2, 29)) == 'datetime.date(2024, 2, 29)', 'leap year date repr'
assert str(datetime.date(1, 1, 1)) == '0001-01-01', 'minimum date str'
assert str(datetime.date(9999, 12, 31)) == '9999-12-31', 'maximum date str'

# === error messages should match CPython 3.14 ===

try:
    datetime.date(10000, 1, 1)
    assert False, 'year OOB should raise ValueError'
except ValueError as e:
    assert str(e) == 'year must be in 1..9999, not 10000', f'year OOB message: {e}'

try:
    datetime.date(0, 1, 1)
    assert False, 'year 0 should raise ValueError'
except ValueError as e:
    assert str(e) == 'year must be in 1..9999, not 0', f'year 0 message: {e}'

try:
    datetime.date(2024, 13, 1)
    assert False, 'month OOB should raise ValueError'
except ValueError as e:
    assert str(e) == 'month must be in 1..12, not 13', f'month OOB message: {e}'

try:
    datetime.date(2024, 0, 1)
    assert False, 'month 0 should raise ValueError'
except ValueError as e:
    assert str(e) == 'month must be in 1..12, not 0', f'month 0 message: {e}'

try:
    datetime.date(2024, 2, 30)
    assert False, 'day OOB should raise ValueError'
except ValueError as e:
    assert str(e) == 'day 30 must be in range 1..29 for month 2 in year 2024', f'day OOB message: {e}'

try:
    datetime.date(2024, 1, 0)
    assert False, 'day 0 should raise ValueError'
except ValueError as e:
    assert str(e) == 'day 0 must be in range 1..31 for month 1 in year 2024', f'day 0 message: {e}'

try:
    datetime.datetime(2024, 1, 1, 25)
    assert False, 'hour OOB should raise ValueError'
except ValueError as e:
    assert str(e) == 'hour must be in 0..23, not 25', f'hour OOB message: {e}'

try:
    datetime.datetime(2024, 1, 1, 0, 60)
    assert False, 'minute OOB should raise ValueError'
except ValueError as e:
    assert str(e) == 'minute must be in 0..59, not 60', f'minute OOB message: {e}'

try:
    datetime.datetime(2024, 1, 1, 0, 0, 60)
    assert False, 'second OOB should raise ValueError'
except ValueError as e:
    assert str(e) == 'second must be in 0..59, not 60', f'second OOB message: {e}'

try:
    datetime.datetime(2024, 1, 1, 0, 0, 0, 1000000)
    assert False, 'microsecond OOB should raise ValueError'
except ValueError as e:
    assert str(e) == 'microsecond must be in 0..999999, not 1000000', f'microsecond OOB message: {e}'

# === timedelta truthiness ===

assert not datetime.timedelta(0), 'timedelta(0) should be falsy'
assert datetime.timedelta(seconds=1), 'non-zero timedelta should be truthy'
assert datetime.timedelta(days=-1), 'negative timedelta should be truthy'

# === isinstance subclass: datetime is a subclass of date ===

assert isinstance(datetime.datetime(2024, 1, 1, 0, 0), datetime.date), (
    'datetime should be instance of date (datetime is subclass of date)'
)
assert not isinstance(datetime.date(2024, 1, 1), datetime.datetime), 'date should NOT be instance of datetime'

# === isoformat ===

assert datetime.date(2024, 1, 15).isoformat() == '2024-01-15', 'date.isoformat()'
assert datetime.datetime(2024, 1, 15, 10, 30).isoformat() == '2024-01-15T10:30:00', 'naive datetime.isoformat()'
assert datetime.datetime(2024, 1, 15, 10, 30, 0, 123456).isoformat() == '2024-01-15T10:30:00.123456', (
    'datetime.isoformat() with microseconds'
)
utc_iso = datetime.datetime(2024, 1, 15, 10, 30, tzinfo=datetime.timezone.utc)
assert utc_iso.isoformat() == '2024-01-15T10:30:00+00:00', 'aware UTC datetime.isoformat()'

# === strftime ===

assert datetime.datetime(2024, 6, 15, 10, 30, 45).strftime('%Y-%m-%d') == '2024-06-15', 'datetime.strftime date format'
assert datetime.datetime(2024, 6, 15, 10, 30, 45).strftime('%H:%M:%S') == '10:30:45', 'datetime.strftime time format'
assert datetime.date(2024, 6, 15).strftime('%Y/%m/%d') == '2024/06/15', 'date.strftime'

# === replace ===

assert datetime.date(2024, 6, 15).replace(month=1) == datetime.date(2024, 1, 15), 'date.replace(month=1)'
assert datetime.date(2024, 6, 15).replace(year=2025, day=1) == datetime.date(2025, 6, 1), 'date.replace(year, day)'
assert datetime.datetime(2024, 6, 15, 10, 30).replace(hour=0, minute=0) == datetime.datetime(2024, 6, 15, 0, 0), (
    'datetime.replace(hour, minute)'
)

# === weekday / isoweekday ===

assert datetime.date(2024, 6, 15).weekday() == 5, 'Saturday weekday() should be 5'
assert datetime.date(2024, 6, 15).isoweekday() == 6, 'Saturday isoweekday() should be 6'
assert datetime.date(2024, 6, 10).weekday() == 0, 'Monday weekday() should be 0'
assert datetime.date(2024, 6, 10).isoweekday() == 1, 'Monday isoweekday() should be 1'
assert datetime.datetime(2024, 6, 15, 12, 0).weekday() == 5, 'datetime.weekday()'

# === datetime.date() method ===

assert datetime.datetime(2024, 6, 15, 10, 30).date() == datetime.date(2024, 6, 15), 'datetime.date() extracts date'

# === datetime.timestamp() ===

assert datetime.datetime(2024, 6, 15, 10, 30, 0, tzinfo=datetime.timezone.utc).timestamp() == 1718447400.0, (
    'aware UTC datetime.timestamp()'
)

# === timedelta * int ===

assert datetime.timedelta(days=1) * 7 == datetime.timedelta(days=7), 'timedelta * int'
assert 3 * datetime.timedelta(days=1) == datetime.timedelta(days=3), 'int * timedelta'
assert datetime.timedelta(hours=2) * 0 == datetime.timedelta(0), 'timedelta * 0'

# === abs(timedelta) ===

assert abs(datetime.timedelta(days=-3)) == datetime.timedelta(days=3), 'abs(negative timedelta)'
assert abs(datetime.timedelta(0)) == datetime.timedelta(0), 'abs(zero timedelta)'
assert abs(datetime.timedelta(days=5)) == datetime.timedelta(days=5), 'abs(positive timedelta)'

# === timedelta // int and timedelta / int ===

assert datetime.timedelta(days=1) // 2 == datetime.timedelta(hours=12), 'timedelta // int'
assert datetime.timedelta(days=1) / 2 == datetime.timedelta(hours=12), 'timedelta / int'
