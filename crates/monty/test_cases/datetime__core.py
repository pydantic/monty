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
# TODO(datetime): restore once UnaryNeg handles timedelta values without VM-specific branching.
# assert -datetime.timedelta(days=1, seconds=30) == datetime.timedelta(days=-2, seconds=86370), (
#     'unary -timedelta should normalize like CPython'
# )
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
