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
assert datetime.datetime(2024, 1, 1, 12, 0, tzinfo=datetime.timezone.utc) == datetime.datetime(
    2024, 1, 1, 13, 0, tzinfo=datetime.timezone(datetime.timedelta(hours=1))
), 'aware datetime equality should compare UTC instants, not local fields'

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
    assert str(e) == "argument for function given by name ('hour') and position (4)", (
        'datetime duplicate hour should raise CPython-style duplicate-binding TypeError'
    )

try:
    datetime.datetime(2024, 1, 1, 0, 0, 0, 0, datetime.timezone.utc, tzinfo=datetime.timezone.utc)
    assert False, 'datetime constructor should reject positional+keyword duplicate tzinfo'
except TypeError as e:
    assert str(e) == "argument for function given by name ('tzinfo') and position (8)", (
        'datetime duplicate tzinfo should raise CPython-style duplicate-binding TypeError'
    )

try:
    datetime.timezone(datetime.timedelta(hours=1), offset=datetime.timedelta(hours=1))
    assert False, 'timezone constructor should reject positional+keyword duplicate offset'
except TypeError as e:
    assert str(e) == "argument for timezone() given by name ('offset') and position (1)", (
        'timezone duplicate offset should raise duplicate-binding TypeError'
    )

try:
    datetime.timezone(datetime.timedelta(hours=1), 'A', name='B')
    assert False, 'timezone constructor should reject 3 arguments even when name is also provided by keyword'
except TypeError as e:
    assert str(e) == 'timezone() takes at most 2 arguments (3 given)', f'timezone 3-arg error: {e}'

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
assert datetime.datetime.strptime('2024-06-15 10:30:45.1', '%Y-%m-%d %H:%M:%S.%f') == datetime.datetime(
    2024, 6, 15, 10, 30, 45, 100000
), 'strptime %f should accept 1 digit and right-pad to microseconds'

# === replace ===

assert datetime.date(2024, 6, 15).replace(month=1) == datetime.date(2024, 1, 15), 'date.replace(month=1)'
assert datetime.date(2024, 6, 15).replace(year=2025, day=1) == datetime.date(2025, 6, 1), 'date.replace(year, day)'
assert datetime.datetime(2024, 6, 15, 10, 30).replace(hour=0, minute=0) == datetime.datetime(2024, 6, 15, 0, 0), (
    'datetime.replace(hour, minute)'
)
assert datetime.datetime(2024, 6, 15, 10, 30).replace(tzinfo=datetime.timezone.utc) == datetime.datetime(
    2024, 6, 15, 10, 30, tzinfo=datetime.timezone.utc
), 'datetime.replace(tzinfo=...) should replace the timezone'

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
assert datetime.timedelta(microseconds=3) / 2 == datetime.timedelta(microseconds=2), (
    'timedelta / int should round to nearest microsecond with ties-to-even'
)

# === date.fromisoformat ===

assert datetime.date.fromisoformat('2024-06-15') == datetime.date(2024, 6, 15), 'date.fromisoformat YYYY-MM-DD'

try:
    datetime.date.fromisoformat('not-a-date')
    assert False, 'date.fromisoformat should reject invalid strings'
except ValueError as e:
    assert str(e) == "Invalid isoformat string: 'not-a-date'", f'date.fromisoformat error message: {e}'

# === datetime.fromisoformat ===

assert datetime.datetime.fromisoformat('2024-06-15') == datetime.datetime(2024, 6, 15, 0, 0), (
    'datetime.fromisoformat date only'
)
assert datetime.datetime.fromisoformat('2024-06-15T10:30:00') == datetime.datetime(2024, 6, 15, 10, 30), (
    'datetime.fromisoformat with T separator'
)
assert datetime.datetime.fromisoformat('2024-06-15 10:30:00') == datetime.datetime(2024, 6, 15, 10, 30), (
    'datetime.fromisoformat with space separator'
)
assert datetime.datetime.fromisoformat('2024-06-15T10:30') == datetime.datetime(2024, 6, 15, 10, 30), (
    'datetime.fromisoformat without seconds'
)
assert datetime.datetime.fromisoformat('2024-06-15T10:30:00.123456') == datetime.datetime(
    2024, 6, 15, 10, 30, 0, 123456
), 'datetime.fromisoformat with microseconds'

iso_utc = datetime.datetime.fromisoformat('2024-06-15T10:30:00+00:00')
assert iso_utc == datetime.datetime(2024, 6, 15, 10, 30, tzinfo=datetime.timezone.utc), (
    'datetime.fromisoformat with UTC offset'
)

# === datetime.strptime ===

assert datetime.datetime.strptime('2024-06-15', '%Y-%m-%d') == datetime.datetime(2024, 6, 15, 0, 0), (
    'strptime date-only format'
)
assert datetime.datetime.strptime('2024-06-15 10:30:45', '%Y-%m-%d %H:%M:%S') == datetime.datetime(
    2024, 6, 15, 10, 30, 45
), 'strptime datetime format'
assert datetime.datetime.strptime('15/06/2024', '%d/%m/%Y') == datetime.datetime(2024, 6, 15, 0, 0), (
    'strptime custom date format'
)

try:
    datetime.datetime.strptime('2024-06-15', '%d/%m/%Y')
    assert False, 'strptime should reject mismatched format'
except ValueError as e:
    assert str(e) == "time data '2024-06-15' does not match format '%d/%m/%Y'", f'strptime error message: {e}'

# === keyword-only construction for date ===

assert datetime.date(year=2024, month=6, day=15) == datetime.date(2024, 6, 15), 'date keyword construction'
assert datetime.date(2024, month=6, day=15) == datetime.date(2024, 6, 15), 'date mixed positional/keyword construction'

try:
    datetime.date(2024, 1, 1, 1)
    assert False, 'date should reject too many positional args'
except TypeError as e:
    assert str(e) == 'function takes at most 3 arguments (4 given)', f'date too many args message: {e}'

try:
    datetime.date(2024, 1, 1, foo=1)
    assert False, 'date should reject unknown keyword arg'
except TypeError as e:
    assert str(e) == 'function takes at most 3 arguments (4 given)', f'date unknown kwarg message: {e}'

# === missing positional arguments for date ===

try:
    datetime.date()
    assert False, 'date() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'year' (pos 1)", f'date() no args message: {e}'

try:
    datetime.date(2024)
    assert False, 'date() with 1 arg should raise TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'month' (pos 2)", f'date(year) message: {e}'

try:
    datetime.date(2024, 1)
    assert False, 'date() with 2 args should raise TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'day' (pos 3)", f'date(year, month) message: {e}'

# === keyword-only construction for datetime ===

assert datetime.datetime(year=2024, month=1, day=1, hour=12) == datetime.datetime(2024, 1, 1, 12), (
    'datetime keyword construction'
)

try:
    datetime.datetime(2024, 1, 1, foo=1)
    assert False, 'datetime should reject unknown keyword arg'
except TypeError as e:
    assert str(e) == "this function got an unexpected keyword argument 'foo'", f'datetime unknown kwarg message: {e}'

# === missing positional arguments for datetime ===

try:
    datetime.datetime()
    assert False, 'datetime() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'year' (pos 1)", f'datetime() no args message: {e}'

try:
    datetime.datetime(2024)
    assert False, 'datetime() with 1 arg should raise TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'month' (pos 2)", f'datetime(year) message: {e}'

try:
    datetime.datetime(2024, 1)
    assert False, 'datetime() with 2 args should raise TypeError'
except TypeError as e:
    assert str(e) == "function missing required argument 'day' (pos 3)", f'datetime(year, month) message: {e}'

# === aware datetime arithmetic ===

utc = datetime.timezone.utc
aware_base = datetime.datetime(2024, 6, 15, 12, 0, 0, tzinfo=utc)
td_2h = datetime.timedelta(hours=2)

# aware datetime + timedelta
aware_add = aware_base + td_2h
assert aware_add == datetime.datetime(2024, 6, 15, 14, 0, 0, tzinfo=utc), 'aware datetime + timedelta'
assert aware_add.tzinfo is utc, 'aware datetime + timedelta preserves tzinfo'

# aware datetime - timedelta
aware_sub = aware_base - td_2h
assert aware_sub == datetime.datetime(2024, 6, 15, 10, 0, 0, tzinfo=utc), 'aware datetime - timedelta'
assert aware_sub.tzinfo is utc, 'aware datetime - timedelta preserves tzinfo'

# aware datetime - aware datetime
aware_diff = aware_base - datetime.datetime(2024, 6, 15, 10, 0, 0, tzinfo=utc)
assert aware_diff == datetime.timedelta(hours=2), 'aware datetime - aware datetime'

# aware datetime subtraction with different offsets
plus5 = datetime.timezone(datetime.timedelta(hours=5))
aware_plus5 = datetime.datetime(2024, 6, 15, 17, 0, 0, tzinfo=plus5)
diff_tz = aware_base - aware_plus5
assert diff_tz == datetime.timedelta(0), 'aware datetimes at same UTC instant should have zero diff'

# === aware datetime comparison ===

aware_a = datetime.datetime(2024, 1, 1, 12, 0, 0, tzinfo=utc)
aware_b = datetime.datetime(2024, 1, 1, 14, 0, 0, tzinfo=utc)
assert aware_a < aware_b, 'aware datetime < comparison'
assert aware_b > aware_a, 'aware datetime > comparison'
assert aware_a <= aware_a, 'aware datetime <= equal'
assert aware_a >= aware_a, 'aware datetime >= equal'
assert not (aware_a > aware_b), 'aware datetime not >'

# === naive datetime comparison ===

naive_a = datetime.datetime(2024, 1, 1, 10, 0, 0)
naive_b = datetime.datetime(2024, 1, 1, 12, 0, 0)
assert naive_a < naive_b, 'naive datetime < comparison'
assert naive_b > naive_a, 'naive datetime > comparison'
assert naive_a == naive_a, 'naive datetime equality'
assert not (naive_a == naive_b), 'naive datetime inequality'

# === timedelta comparison ===

td_a = datetime.timedelta(days=1)
td_b = datetime.timedelta(days=2)
assert td_a < td_b, 'timedelta < comparison'
assert td_b > td_a, 'timedelta > comparison'
assert td_a <= td_a, 'timedelta <= equal'
assert td_a >= td_a, 'timedelta >= equal'
assert td_a == td_a, 'timedelta equality'
assert not (td_a == td_b), 'timedelta inequality'

# === timedelta repr with microseconds ===

assert repr(datetime.timedelta(microseconds=500)) == 'datetime.timedelta(microseconds=500)', (
    'timedelta repr with microseconds only'
)
assert repr(datetime.timedelta(seconds=1, microseconds=500)) == ('datetime.timedelta(seconds=1, microseconds=500)'), (
    'timedelta repr with seconds and microseconds'
)

# === datetime repr with seconds and microseconds ===

assert repr(datetime.datetime(2024, 1, 1, 0, 0, 30)) == 'datetime.datetime(2024, 1, 1, 0, 0, 30)', (
    'datetime repr with seconds'
)
assert repr(datetime.datetime(2024, 1, 1, 0, 0, 0, 123456)) == 'datetime.datetime(2024, 1, 1, 0, 0, 0, 123456)', (
    'datetime repr with microseconds'
)
assert repr(datetime.datetime(2024, 1, 1, 0, 0, 30, 123456)) == ('datetime.datetime(2024, 1, 1, 0, 0, 30, 123456)'), (
    'datetime repr with seconds and microseconds'
)

# === datetime str with microseconds ===

assert str(datetime.datetime(2024, 1, 1, 10, 30, 0, 123456)) == '2024-01-01 10:30:00.123456', (
    'datetime str with microseconds'
)

# === datetime repr with UTC timezone ===

assert repr(datetime.datetime(2024, 1, 1, 0, 0, tzinfo=datetime.timezone.utc)) == (
    'datetime.datetime(2024, 1, 1, 0, 0, tzinfo=datetime.timezone.utc)'
), 'datetime repr with UTC timezone'

# === datetime.replace with second/microsecond ===

dt_rep = datetime.datetime(2024, 6, 15, 10, 30, 45, 123456)
assert dt_rep.replace(second=0) == datetime.datetime(2024, 6, 15, 10, 30, 0, 123456), 'datetime.replace(second=0)'
assert dt_rep.replace(microsecond=0) == datetime.datetime(2024, 6, 15, 10, 30, 45, 0), 'datetime.replace(microsecond=0)'
assert dt_rep.replace(year=2025, month=1, day=1, hour=0, minute=0, second=0, microsecond=0) == (
    datetime.datetime(2025, 1, 1, 0, 0)
), 'datetime.replace all fields'

# === date.replace ===

d_rep = datetime.date(2024, 6, 15)
assert d_rep.replace(year=2025) == datetime.date(2025, 6, 15), 'date.replace(year)'
assert d_rep.replace(month=1) == datetime.date(2024, 1, 15), 'date.replace(month)'
assert d_rep.replace(day=1) == datetime.date(2024, 6, 1), 'date.replace(day)'

# === date.isoformat ===

assert datetime.date(1, 1, 1).isoformat() == '0001-01-01', 'isoformat with minimum date'
assert datetime.date(9999, 12, 31).isoformat() == '9999-12-31', 'isoformat with maximum date'

# === datetime.isoformat with timezone ===

plus1 = datetime.timezone(datetime.timedelta(hours=1))
assert datetime.datetime(2024, 1, 15, 10, 30, tzinfo=plus1).isoformat() == '2024-01-15T10:30:00+01:00', (
    'aware datetime.isoformat with +01:00'
)

minus5 = datetime.timezone(datetime.timedelta(hours=-5))
assert datetime.datetime(2024, 1, 15, 10, 30, tzinfo=minus5).isoformat() == '2024-01-15T10:30:00-05:00', (
    'aware datetime.isoformat with -05:00'
)

# === datetime.isoformat with seconds and microseconds ===

assert datetime.datetime(2024, 1, 15, 10, 30, 45).isoformat() == '2024-01-15T10:30:45', (
    'datetime.isoformat with seconds'
)

# === date arithmetic ===

assert datetime.date(2024, 3, 1) - datetime.date(2024, 2, 1) == datetime.timedelta(days=29), (
    'date subtraction across leap year February'
)

# === date comparison ===

d_a = datetime.date(2024, 1, 1)
d_b = datetime.date(2024, 12, 31)
assert d_a < d_b, 'date < comparison'
assert d_b > d_a, 'date > comparison'
assert d_a <= d_a, 'date <= equal'
assert d_a >= d_a, 'date >= equal'

# === date attribute access ===

d_attr = datetime.date(2024, 6, 15)
assert d_attr.year == 2024, 'date.year attribute'
assert d_attr.month == 6, 'date.month attribute'
assert d_attr.day == 15, 'date.day attribute'

# === timedelta with milliseconds, minutes, hours, weeks ===

assert datetime.timedelta(milliseconds=1500) == datetime.timedelta(seconds=1, microseconds=500000), (
    'timedelta with milliseconds'
)
assert datetime.timedelta(minutes=90) == datetime.timedelta(seconds=5400), 'timedelta with minutes'
assert datetime.timedelta(hours=2) == datetime.timedelta(seconds=7200), 'timedelta with hours'
assert datetime.timedelta(weeks=1) == datetime.timedelta(days=7), 'timedelta with weeks'

# === timedelta attributes ===

td_attrs = datetime.timedelta(days=3, seconds=7200, microseconds=500)
assert td_attrs.days == 3, 'timedelta.days attribute'
assert td_attrs.seconds == 7200, 'timedelta.seconds attribute'
assert td_attrs.microseconds == 500, 'timedelta.microseconds attribute'

# === timezone constructor edge cases ===

try:
    datetime.timezone(datetime.timedelta(hours=-24))
    assert False, 'timezone offset at -24 hours should raise ValueError'
except ValueError as e:
    assert str(e) == (
        'offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), '
        'not datetime.timedelta(days=-1)'
    ), f'timezone -24h range validation: {e}'

try:
    datetime.timezone()
    assert False, 'timezone() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == "timezone() missing required argument 'offset' (pos 1)", f'timezone() no args message: {e}'

# === timezone repr and str ===

assert str(datetime.timezone.utc) == 'UTC', 'timezone.utc str should be UTC'
assert str(datetime.timezone(datetime.timedelta(hours=5))) == 'UTC+05:00', 'positive offset timezone str'
assert str(datetime.timezone(datetime.timedelta(hours=-5))) == 'UTC-05:00', 'negative offset timezone str'
assert str(datetime.timezone(datetime.timedelta(hours=5, minutes=30))) == 'UTC+05:30', 'offset with minutes'
assert str(datetime.timezone(datetime.timedelta(hours=0), 'MyTZ')) == 'MyTZ', 'named timezone str uses name'

# === datetime.now with tz keyword arg ===

now_kw = datetime.datetime.now(tz=datetime.timezone.utc)
assert now_kw.tzinfo is not None, 'datetime.now(tz=...) should return aware datetime'

# === hash ===

assert hash(datetime.date(2024, 1, 1)) == hash(datetime.date(2024, 1, 1)), 'date hash consistency'
assert hash(datetime.datetime(2024, 1, 1, 12, 0)) == hash(datetime.datetime(2024, 1, 1, 12, 0)), (
    'datetime hash consistency'
)
assert hash(datetime.timedelta(days=1)) == hash(datetime.timedelta(days=1)), 'timedelta hash consistency'

# === datetime.timestamp() for naive datetime ===

# naive datetimes use local time for timestamp, just check it returns a float
ts_naive = datetime.datetime(2024, 6, 15, 12, 0, 0).timestamp()
assert isinstance(ts_naive, float), 'naive datetime.timestamp() should return float'

# === aware datetime timestamp ===

ts_aware = datetime.datetime(1970, 1, 1, 0, 0, 0, tzinfo=datetime.timezone.utc).timestamp()
assert ts_aware == 0.0, f'epoch aware timestamp should be 0.0, got {ts_aware}'

# === datetime constructor error: too many positional args (line 231) ===

try:
    datetime.datetime(2024, 1, 1, 0, 0, 0, 0, datetime.timezone.utc, 'extra')
    assert False, 'datetime with 9 positional args should raise TypeError'
except TypeError as e:
    assert str(e) == 'function takes at most 8 positional arguments (9 given)', f'datetime too many args: {e}'

# === datetime constructor error: duplicate keyword args (lines 244-286) ===

try:
    datetime.datetime(2024, 1, 1, year=2024)
    assert False, 'positional+keyword year should raise TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('year') and position (1)", f'dup year: {e}'

try:
    datetime.datetime(2024, 1, 1, day=1)
    assert False, 'positional+keyword day should raise TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('day') and position (3)", f'dup day: {e}'

try:
    datetime.datetime(2024, 1, 1, 0, 30, minute=30)
    assert False, 'positional+keyword minute should raise TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('minute') and position (5)", f'dup minute: {e}'

try:
    datetime.datetime(2024, 1, 1, 0, 0, 30, second=30)
    assert False, 'positional+keyword second should raise TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('second') and position (6)", f'dup second: {e}'

try:
    datetime.datetime(2024, 1, 1, 0, 0, 0, 500, microsecond=500)
    assert False, 'positional+keyword microsecond should raise TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('microsecond') and position (7)", f'dup microsecond: {e}'

try:
    datetime.datetime(2024, 1, 1, 0, 0, 0, 0, datetime.timezone.utc, tzinfo=datetime.timezone.utc)
    assert False, 'positional+keyword tzinfo should raise TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('tzinfo') and position (8)", f'dup tzinfo: {e}'

# === datetime.now() error: too many positional args (lines 350-357) ===

try:
    datetime.datetime.now(datetime.timezone.utc, datetime.timezone.utc)
    assert False, 'datetime.now() with 2 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'now() takes at most 1 argument (2 given)', f'datetime.now too many args: {e}'

# === datetime.now() error: bad keyword argument (lines 370-374) ===

try:
    datetime.datetime.now(badkw=1)
    assert False, 'datetime.now(badkw=1) should raise TypeError'
except TypeError as e:
    assert str(e) == "now() got an unexpected keyword argument 'badkw'", f'datetime.now bad kwarg: {e}'

# === datetime.now() error: bad tz type (lines 380-382) ===

try:
    datetime.datetime.now(tz=123)
    assert False, 'datetime.now(tz=123) should raise TypeError'
except TypeError as e:
    assert str(e) == "tzinfo argument must be None or of a tzinfo subclass, not type 'int'", f'now bad tz: {e}'

# === datetime.now() error: duplicate tz (lines 376-378) ===

try:
    datetime.datetime.now(datetime.timezone.utc, tz=datetime.timezone.utc)
    assert False, 'datetime.now(utc, tz=utc) should raise TypeError for duplicate tz'
except TypeError as e:
    assert str(e) == 'now() takes at most 1 argument (2 given)', f'datetime.now dup tz message: {e}'

# === datetime.now() with tz=None ===

now_tz_none = datetime.datetime.now(tz=None)
assert now_tz_none.tzinfo is None, 'now(tz=None) should be naive'

# === aware datetime arithmetic: add timedelta (lines 523-542) ===

utc = datetime.timezone.utc
aware_dt = datetime.datetime(2024, 6, 15, 12, 0, tzinfo=utc)
td = datetime.timedelta(hours=5)
result_add = aware_dt + td
assert repr(result_add) == 'datetime.datetime(2024, 6, 15, 17, 0, tzinfo=datetime.timezone.utc)', (
    f'aware datetime + timedelta: {result_add!r}'
)

# === aware datetime arithmetic: sub timedelta (lines 553-572) ===

result_sub = aware_dt - td
assert repr(result_sub) == 'datetime.datetime(2024, 6, 15, 7, 0, tzinfo=datetime.timezone.utc)', (
    f'aware datetime - timedelta: {result_sub!r}'
)

# === aware datetime - aware datetime (lines 583-602) ===

aware_a = datetime.datetime(2024, 6, 15, 12, 0, tzinfo=utc)
aware_b = datetime.datetime(2024, 6, 14, 10, 0, tzinfo=utc)
diff_aware = aware_a - aware_b
assert repr(diff_aware) == 'datetime.timedelta(days=1, seconds=7200)', f'aware sub: {diff_aware!r}'

# === naive datetime - naive datetime ===

naive_a = datetime.datetime(2024, 6, 15, 12, 0)
naive_b = datetime.datetime(2024, 6, 14, 10, 0)
diff_naive = naive_a - naive_b
assert repr(diff_naive) == 'datetime.timedelta(days=1, seconds=7200)', f'naive sub: {diff_naive!r}'

# === aware vs naive equality returns False (lines 911-921) ===

naive_dt = datetime.datetime(2024, 1, 1, 12, 0)
aware_dt2 = datetime.datetime(2024, 1, 1, 12, 0, tzinfo=utc)
assert not (naive_dt == aware_dt2), 'naive != aware should be False'
assert not (aware_dt2 == naive_dt), 'aware != naive should be False'

# === aware datetime comparison (lines 923-936) ===

aware_early = datetime.datetime(2024, 1, 1, 10, 0, tzinfo=utc)
aware_late = datetime.datetime(2024, 1, 1, 14, 0, tzinfo=utc)
assert aware_early < aware_late, 'earlier aware < later aware'
assert aware_late > aware_early, 'later aware > earlier aware'
assert aware_early <= aware_late, 'earlier aware <= later aware'
assert aware_late >= aware_early, 'later aware >= earlier aware'
assert aware_early <= aware_early, 'aware <= self'
assert aware_early >= aware_early, 'aware >= self'

# === aware datetime hash consistency (line 52) ===

hash_a = hash(datetime.datetime(2024, 1, 1, 12, 0, tzinfo=utc))
hash_b = hash(datetime.datetime(2024, 1, 1, 12, 0, tzinfo=utc))
assert hash_a == hash_b, 'aware datetime hash should be consistent'

# === datetime.isoweekday (lines 1027-1030) ===

monday_dt = datetime.datetime(2024, 1, 1, 12, 0)  # 2024-01-01 is a Monday
assert monday_dt.isoweekday() == 1, f'Monday isoweekday should be 1, got {monday_dt.isoweekday()}'

saturday_dt = datetime.datetime(2024, 1, 6, 12, 0)  # 2024-01-06 is a Saturday
assert saturday_dt.isoweekday() == 6, f'Saturday isoweekday should be 6, got {saturday_dt.isoweekday()}'

sunday_dt = datetime.datetime(2024, 1, 7, 12, 0)  # 2024-01-07 is a Sunday
assert sunday_dt.isoweekday() == 7, f'Sunday isoweekday should be 7, got {sunday_dt.isoweekday()}'

# === datetime.date() method (line 1038) ===

dt_with_time = datetime.datetime(2024, 6, 15, 12, 30, 45, 123456)
d = dt_with_time.date()
assert repr(d) == 'datetime.date(2024, 6, 15)', f'datetime.date() method: {d!r}'
assert isinstance(d, datetime.date), 'datetime.date() should return a date instance'

# === datetime unknown attribute (line 1046) ===

try:
    datetime.datetime(2024, 1, 1).nosuchattr
    assert False, 'accessing unknown attribute should raise AttributeError'
except AttributeError as e:
    assert str(e) == "'datetime.datetime' object has no attribute 'nosuchattr'", f'datetime attr error: {e}'

# === datetime.replace with keyword args (lines 841-885) ===

dt_rep = datetime.datetime(2024, 6, 15, 12, 30, 45, 123456)
r_all = dt_rep.replace(year=2025, month=3, day=20, hour=8, minute=15, second=30, microsecond=999)
assert repr(r_all) == 'datetime.datetime(2025, 3, 20, 8, 15, 30, 999)', f'replace all fields: {r_all!r}'

# === datetime.replace error: unexpected keyword (lines 865-868) ===

try:
    dt_rep.replace(badkw=1)
    assert False, 'datetime.replace(badkw=1) should raise TypeError'
except TypeError as e:
    assert str(e) == "replace() got an unexpected keyword argument 'badkw'", f'datetime.replace bad kwarg: {e}'

# === datetime.strptime date-only format (line 431) ===

dt_strptime_date = datetime.datetime.strptime('2024-01-15', '%Y-%m-%d')
assert repr(dt_strptime_date) == 'datetime.datetime(2024, 1, 15, 0, 0)', f'strptime date-only: {dt_strptime_date!r}'

# === datetime.strptime with microseconds ===

dt_strptime_micro = datetime.datetime.strptime('2024-01-15 10:30:45.123456', '%Y-%m-%d %H:%M:%S.%f')
assert repr(dt_strptime_micro) == 'datetime.datetime(2024, 1, 15, 10, 30, 45, 123456)', (
    f'strptime with microseconds: {dt_strptime_micro!r}'
)

# === datetime.strptime error: bad format ===

try:
    datetime.datetime.strptime('not-a-date', '%Y-%m-%d')
    assert False, 'strptime with non-matching format should raise ValueError'
except ValueError as e:
    assert str(e) == "time data 'not-a-date' does not match format '%Y-%m-%d'", f'strptime bad format: {e}'

# === datetime bool is always True (lines 939-941) ===

assert bool(datetime.datetime(2024, 1, 1)), 'datetime bool should always be True'
assert bool(datetime.datetime(1, 1, 1, 0, 0, 0, 0)), 'min datetime bool should be True'

# === datetime isoformat (line 783, 1002-1007) ===

iso_plain = datetime.datetime(2024, 1, 1, 12, 30, 45, 123456).isoformat()
assert iso_plain == '2024-01-01T12:30:45.123456', f'isoformat plain: {iso_plain}'

iso_no_micro = datetime.datetime(2024, 1, 1, 12, 30, 45).isoformat()
assert iso_no_micro == '2024-01-01T12:30:45', f'isoformat no micro: {iso_no_micro}'

iso_aware = datetime.datetime(2024, 1, 1, 12, 0, tzinfo=utc).isoformat()
assert iso_aware == '2024-01-01T12:00:00+00:00', f'isoformat aware: {iso_aware}'

# === datetime strftime (lines 1009-1014) ===

dt_fmt = datetime.datetime(2024, 6, 15, 12, 30)
formatted = dt_fmt.strftime('%Y/%m/%d %H:%M')
assert formatted == '2024/06/15 12:30', f'strftime: {formatted}'

# === aware datetime replace preserves tzinfo (line 884) ===

aware_rep = datetime.datetime(2024, 6, 15, 12, 30, tzinfo=utc)
aware_rep_result = aware_rep.replace(year=2025)
assert aware_rep_result.tzinfo is not None, 'replace on aware should preserve tzinfo'
assert repr(aware_rep_result) == 'datetime.datetime(2025, 6, 15, 12, 30, tzinfo=datetime.timezone.utc)', (
    f'aware replace: {aware_rep_result!r}'
)

# === date bool is always True (date.rs py_bool) ===

assert bool(datetime.date(2024, 1, 1)), 'date bool should always be True'
assert bool(datetime.date(1, 1, 1)), 'min date bool should be True'

# === date ordering comparisons (date.rs py_cmp) ===

assert datetime.date(2024, 1, 1) < datetime.date(2024, 1, 2), 'date < should work'
assert datetime.date(2024, 1, 2) > datetime.date(2024, 1, 1), 'date > should work'
assert datetime.date(2024, 1, 1) <= datetime.date(2024, 1, 1), 'date <= equal should work'
assert datetime.date(2024, 1, 1) >= datetime.date(2024, 1, 1), 'date >= equal should work'
assert datetime.date(2024, 1, 1) <= datetime.date(2024, 1, 2), 'date <= less should work'
assert datetime.date(2024, 6, 15) >= datetime.date(2024, 6, 14), 'date >= greater should work'

# === date constructor: negative day (date.rs from_ymd) ===

try:
    datetime.date(2024, 1, -1)
    assert False, 'date(day=-1) should raise ValueError'
except ValueError as e:
    assert str(e) == 'day -1 must be in range 1..31 for month 1 in year 2024', f'date neg day: {e}'

# === date constructor: too many args (date.rs init) ===

try:
    datetime.date(2024, 1, 1, foo=1)
    assert False, 'date with extra kwarg should raise TypeError'
except TypeError as e:
    assert str(e) == 'function takes at most 3 arguments (4 given)', f'date too many args: {e}'

# === date constructor: duplicate year kwarg (date.rs init) ===

try:
    datetime.date(2024, year=2024)
    assert False, 'date with duplicate year should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

# === date constructor: duplicate month kwarg (date.rs init) ===

try:
    datetime.date(2024, 1, month=1)
    assert False, 'date with duplicate month should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

# === date.replace() unexpected keyword (date.rs extract_date_replace_kwargs) ===

try:
    datetime.date(2024, 1, 1).replace(foo=1)
    assert False, 'date.replace(foo=1) should raise TypeError'
except TypeError as e:
    assert str(e) == "replace() got an unexpected keyword argument 'foo'", f'date.replace bad kwarg: {e}'

# === timedelta overflow (timedelta.rs new) ===

try:
    datetime.timedelta(days=1000000000)
    assert False, 'timedelta with 1e9 days should raise OverflowError'
except OverflowError as e:
    assert str(e) == 'days=1000000000; must have magnitude <= 999999999', f'td overflow: {e}'

try:
    datetime.timedelta(days=-1000000000)
    assert False, 'timedelta with -1e9 days should raise OverflowError'
except OverflowError as e:
    assert str(e) == 'days=-1000000000; must have magnitude <= 999999999', f'td neg overflow: {e}'

# === timedelta constructor: duplicate kwargs (timedelta.rs init) ===

try:
    datetime.timedelta(1, days=1)
    assert False, 'timedelta with duplicate days should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

try:
    datetime.timedelta(1, 2, seconds=2)
    assert False, 'timedelta with duplicate seconds should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

try:
    datetime.timedelta(1, 2, 3, microseconds=3)
    assert False, 'timedelta with duplicate microseconds should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

# === timedelta constructor: unexpected keyword (timedelta.rs init) ===

try:
    datetime.timedelta(foo=1)
    assert False, 'timedelta with unexpected kwarg should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

# === timedelta str with microseconds (timedelta.rs py_str) ===

assert str(datetime.timedelta(seconds=1, microseconds=500)) == '0:00:01.000500', (
    'timedelta str should include 6-digit microsecond padding'
)
assert str(datetime.timedelta(microseconds=1)) == '0:00:00.000001', 'timedelta str should show single microsecond'
assert str(datetime.timedelta(days=1, microseconds=123456)) == '1 day, 0:00:00.123456', (
    'timedelta str with days and microseconds'
)

# === timedelta ordering comparisons (timedelta.rs py_cmp) ===

assert datetime.timedelta(days=1) < datetime.timedelta(days=2), 'timedelta < should work'
assert datetime.timedelta(days=2) > datetime.timedelta(days=1), 'timedelta > should work'
assert datetime.timedelta(days=1) <= datetime.timedelta(days=1), 'timedelta <= equal should work'
assert datetime.timedelta(days=1) >= datetime.timedelta(days=1), 'timedelta >= equal should work'
assert datetime.timedelta(seconds=30) < datetime.timedelta(seconds=60), 'timedelta < seconds'
assert datetime.timedelta(days=1) >= datetime.timedelta(seconds=86399), 'timedelta >= cross-unit'

# === timezone constructor: too many args (timezone.rs init) ===

try:
    datetime.timezone(datetime.timedelta(0), 'UTC', 'extra')
    assert False, 'timezone with 3 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'timezone() takes at most 2 arguments (3 given)', f'tz too many: {e}'

# === timezone constructor: unexpected keyword (timezone.rs init) ===

try:
    datetime.timezone(datetime.timedelta(0), foo='bar')
    assert False, 'timezone with unexpected kwarg should raise TypeError'
except TypeError as e:
    assert str(e) == "timezone() got an unexpected keyword argument 'foo'", f'tz bad kwarg: {e}'

# === timezone constructor: non-timedelta offset (timezone.rs extract_offset_seconds) ===

try:
    datetime.timezone(3600)
    assert False, 'timezone(int) should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

# === timezone constructor: non-string name (timezone.rs extract_name) ===

try:
    datetime.timezone(datetime.timedelta(0), 123)
    assert False, 'timezone(td, int) should raise TypeError'
except TypeError:
    pass  # message differs between CPython and Monty

# === timezone constructor: offset out of range (timezone.rs extract_offset_seconds) ===

try:
    datetime.timezone(datetime.timedelta(hours=25))
    assert False, 'timezone with 25h offset should raise ValueError'
except ValueError as e:
    assert 'strictly between' in str(e), f'tz out of range: {e}'

try:
    datetime.timezone(datetime.timedelta(hours=-25))
    assert False, 'timezone with -25h offset should raise ValueError'
except ValueError as e:
    assert 'strictly between' in str(e), f'tz neg out of range: {e}'

# === timezone equality (timezone.rs PartialEq) ===

tz_five = datetime.timezone(datetime.timedelta(hours=5))
tz_five_b = datetime.timezone(datetime.timedelta(hours=5))
tz_six = datetime.timezone(datetime.timedelta(hours=6))
assert tz_five == tz_five_b, 'same offset timezones should be equal'
assert tz_five != tz_six, 'different offset timezones should not be equal'
assert not (tz_five != tz_five_b), 'same offset timezones should not be not-equal'

# === datetime constructor: duplicate month (datetime.rs init) ===

try:
    datetime.datetime(2024, 1, 1, month=1)
    assert False, 'datetime with duplicate month should raise TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('month') and position (2)", f'dt dup month: {e}'
