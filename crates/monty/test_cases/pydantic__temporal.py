from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal

from pydantic import TypeAdapter, ValidationError


def errors(adapter, value, **kwargs):
    try:
        adapter.validate_python(value, **kwargs)
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


UTC = timezone.utc
PLUS_ONE = timezone(timedelta(hours=1))

# === datetime ===
DateTime = TypeAdapter(datetime)
naive = datetime(2020, 1, 1)
assert DateTime.validate_python(naive) is naive
assert DateTime.validate_python('2020-01-02T03:04:05') == datetime(2020, 1, 2, 3, 4, 5)
assert DateTime.validate_python('2020-01-02T03:04:05Z') == datetime(2020, 1, 2, 3, 4, 5, tzinfo=UTC)
assert DateTime.validate_python('2020-01-02T03:04:05Z').utcoffset() == timedelta(0)
assert DateTime.validate_python('2020-01-02T03:04:05.123456+01:00') == datetime(
    2020, 1, 2, 3, 4, 5, 123456, tzinfo=PLUS_ONE
)
assert DateTime.validate_python('2020-01-02T03:04:05+01:00').utcoffset() == timedelta(hours=1)
assert DateTime.validate_python('2020-01-02 03:04:05') == datetime(2020, 1, 2, 3, 4, 5)
assert DateTime.validate_python('2020-01-02') == datetime(2020, 1, 2)
assert DateTime.validate_python(b'2020-01-01T00:00:00') == datetime(2020, 1, 1)
assert DateTime.validate_python(1577836800) == datetime(2020, 1, 1, tzinfo=UTC)
# a large timestamp is read as milliseconds
assert DateTime.validate_python(1577836800000) == datetime(2020, 1, 1, tzinfo=UTC)
assert DateTime.validate_python(1577836800.5) == datetime(2020, 1, 1, 0, 0, 0, 500000, tzinfo=UTC)
assert DateTime.validate_python(date(2020, 1, 1)) == datetime(2020, 1, 1)
assert errors(DateTime, 'x') == [
    {
        'type': 'datetime_from_date_parsing',
        'loc': (),
        'msg': 'Input should be a valid datetime or date, input is too short',
        'input': 'x',
        'ctx': {'error': 'input is too short'},
    }
]
assert errors(DateTime, '2020-13-01') == [
    {
        'type': 'datetime_from_date_parsing',
        'loc': (),
        'msg': 'Input should be a valid datetime or date, month value is outside expected range of 1-12',
        'input': '2020-13-01',
        'ctx': {'error': 'month value is outside expected range of 1-12'},
    }
]
assert errors(DateTime, '2020-01-02T25:00:00')[0]['type'] == 'datetime_from_date_parsing'
assert errors(DateTime, 'nan')[0]['type'] == 'datetime_from_date_parsing'
assert errors(DateTime, True) == [
    {'type': 'datetime_type', 'loc': (), 'msg': 'Input should be a valid datetime', 'input': True}
]
assert errors(DateTime, None)[0]['type'] == 'datetime_type'
assert errors(DateTime, [1])[0]['type'] == 'datetime_type'
assert errors(DateTime, float('nan'))[0]['msg'] == 'Input should be a valid datetime, NaN values not permitted'
assert errors(DateTime, '2020-01-01', strict=True)[0]['type'] == 'datetime_type'
assert errors(DateTime, date(2020, 1, 1), strict=True)[0]['type'] == 'datetime_type'
assert errors(DateTime, 1577836800, strict=True)[0]['type'] == 'datetime_type'
assert DateTime.validate_python(naive, strict=True) is naive
# a Decimal or an int beyond `i64` is a timestamp read as a float; one too large for a float is no number
assert DateTime.validate_python(Decimal('1577836800')) == datetime(2020, 1, 1, tzinfo=UTC)
assert DateTime.validate_python(Decimal('1.5')) == datetime(1970, 1, 1, 0, 0, 1, 500000, tzinfo=UTC)
assert errors(DateTime, 10**30) == [
    {
        'type': 'datetime_parsing',
        'loc': (),
        'msg': 'Input should be a valid datetime, dates after 9999 are not supported as unix timestamps',
        'input': 10**30,
        'ctx': {'error': 'dates after 9999 are not supported as unix timestamps'},
    }
]
assert errors(DateTime, -(10**30))[0]['msg'] == (
    'Input should be a valid datetime, dates before 0000 are not supported as unix timestamps'
)
assert errors(DateTime, 10**400)[0]['type'] == 'datetime_type'
assert errors(DateTime, Decimal('NaN'))[0]['msg'] == 'Input should be a valid datetime, NaN values not permitted'
assert errors(DateTime, Decimal('1'), strict=True)[0]['type'] == 'datetime_type'

# === date ===
Date = TypeAdapter(date)
today = date(2020, 1, 1)
assert Date.validate_python(today) is today
assert Date.validate_python('2020-01-02') == date(2020, 1, 2)
assert Date.validate_python(b'2020-01-02') == date(2020, 1, 2)
# a datetime at midnight is accepted as its date
assert Date.validate_python('2020-01-02T00:00:00') == date(2020, 1, 2)
assert Date.validate_python(datetime(2020, 1, 1)) == date(2020, 1, 1)
assert Date.validate_python(1577836800) == date(2020, 1, 1)
assert errors(Date, '2020-01-02T10:00:00') == [
    {
        'type': 'date_from_datetime_inexact',
        'loc': (),
        'msg': 'Datetimes provided to dates should have zero time - e.g. be exact dates',
        'input': '2020-01-02T10:00:00',
    }
]
assert errors(Date, datetime(2020, 1, 1, 1))[0]['type'] == 'date_from_datetime_inexact'
assert errors(Date, 'x') == [
    {
        'type': 'date_from_datetime_parsing',
        'loc': (),
        'msg': 'Input should be a valid date or datetime, input is too short',
        'input': 'x',
        'ctx': {'error': 'input is too short'},
    }
]
assert errors(Date, '2020-1-1')[0]['type'] == 'date_from_datetime_parsing'
assert errors(Date, None) == [{'type': 'date_type', 'loc': (), 'msg': 'Input should be a valid date', 'input': None}]
assert errors(Date, True)[0]['type'] == 'date_type'
assert errors(Date, '2020-01-02', strict=True)[0]['type'] == 'date_type'
assert errors(Date, datetime(2020, 1, 1), strict=True)[0]['type'] == 'date_type'
assert Date.validate_python(today, strict=True) is today
# a Decimal or a big int is a timestamp, through the datetime fallback
assert Date.validate_python(Decimal('1577836800')) == date(2020, 1, 1)
assert errors(Date, Decimal('1'))[0]['type'] == 'date_from_datetime_inexact'
assert errors(Date, 10**30) == [
    {
        'type': 'date_from_datetime_parsing',
        'loc': (),
        'msg': 'Input should be a valid date or datetime, dates after 9999 are not supported as unix timestamps',
        'input': 10**30,
        'ctx': {'error': 'dates after 9999 are not supported as unix timestamps'},
    }
]
assert errors(Date, 10**400)[0]['type'] == 'date_type'

# === time ===
Time = TypeAdapter(time)
noon = time(12, 0)
assert Time.validate_python(noon) is noon
assert Time.validate_python('01:02:03') == time(1, 2, 3)
assert Time.validate_python('01:02') == time(1, 2)
assert Time.validate_python('01:02:03.123456+01:00') == time(1, 2, 3, 123456, tzinfo=PLUS_ONE)
assert Time.validate_python('01:02:03Z').utcoffset() == timedelta(0)
assert Time.validate_python(b'23:59:59') == time(23, 59, 59)
assert Time.validate_python(3600) == time(1, 0, tzinfo=UTC)
assert Time.validate_python(3600.5) == time(1, 0, 0, 500000, tzinfo=UTC)
assert errors(Time, -1) == [
    {
        'type': 'time_parsing',
        'loc': (),
        'msg': 'Input should be in a valid time format, time in seconds should be positive',
        'input': -1,
        'ctx': {'error': 'time in seconds should be positive'},
    }
]
assert errors(Time, 90000)[0]['type'] == 'time_parsing'
assert errors(Time, 'x') == [
    {
        'type': 'time_parsing',
        'loc': (),
        'msg': 'Input should be in a valid time format, input is too short',
        'input': 'x',
        'ctx': {'error': 'input is too short'},
    }
]
assert errors(Time, '25:00')[0]['type'] == 'time_parsing'
assert errors(Time, True) == [{'type': 'time_type', 'loc': (), 'msg': 'Input should be a valid time', 'input': True}]
assert errors(Time, None)[0]['type'] == 'time_type'
assert errors(Time, '01:02', strict=True)[0]['type'] == 'time_type'
assert Time.validate_python(noon, strict=True) is noon
assert Time.validate_python(Decimal('1.5')) == time(0, 0, 1, 500000, tzinfo=UTC)
assert errors(Time, Decimal('-1'))[0]['msg'] == (
    'Input should be in a valid time format, time in seconds should be positive'
)
assert (
    errors(Time, 10**30)[0]['msg']
    == 'Input should be in a valid time format, numeric times may not exceed 86,399 seconds'
)
assert errors(Time, 10**400)[0]['type'] == 'time_type'
assert errors(Time, Decimal('1'), strict=True)[0]['type'] == 'time_type'

# === timedelta ===
TimeDelta = TypeAdapter(timedelta)
day = timedelta(1)
assert TimeDelta.validate_python(day) is day
assert TimeDelta.validate_python('P1DT1H') == timedelta(days=1, hours=1)
assert TimeDelta.validate_python('PT1.5S') == timedelta(seconds=1, microseconds=500000)
assert TimeDelta.validate_python('-P1D') == timedelta(days=-1)
assert TimeDelta.validate_python('1:00:00') == timedelta(hours=1)
assert TimeDelta.validate_python('1 day, 1:00:00') == timedelta(days=1, hours=1)
assert errors(TimeDelta, '90')[0]['type'] == 'time_delta_parsing'
assert TimeDelta.validate_python(b'PT2M') == timedelta(minutes=2)
assert TimeDelta.validate_python(90) == timedelta(seconds=90)
assert TimeDelta.validate_python(-90) == timedelta(seconds=-90)
assert TimeDelta.validate_python(90.5) == timedelta(seconds=90, microseconds=500000)
assert TimeDelta.validate_python(-0.5) == timedelta(microseconds=-500000)
assert errors(TimeDelta, 'x') == [
    {
        'type': 'time_delta_parsing',
        'loc': (),
        'msg': 'Input should be a valid timedelta, invalid digit in duration',
        'input': 'x',
        'ctx': {'error': 'invalid digit in duration'},
    }
]
assert TimeDelta.validate_python('1 day') == timedelta(days=1)
assert errors(TimeDelta, '')[0]['msg'] == 'Input should be a valid timedelta, input is too short'
assert errors(TimeDelta, None) == [
    {'type': 'time_delta_type', 'loc': (), 'msg': 'Input should be a valid timedelta', 'input': None}
]
assert errors(TimeDelta, [1])[0]['type'] == 'time_delta_type'
assert errors(TimeDelta, 'PT1S', strict=True)[0]['type'] == 'time_delta_type'
assert errors(TimeDelta, 90, strict=True)[0]['type'] == 'time_delta_type'
assert TimeDelta.validate_python(day, strict=True) is day
assert TimeDelta.validate_python(Decimal('1.5')) == timedelta(seconds=1, microseconds=500000)
assert TimeDelta.validate_python(Decimal('-1')) == timedelta(seconds=-1)
assert errors(TimeDelta, 10**30)[0]['msg'] == (
    'Input should be a valid timedelta, durations may not exceed 999,999,999 days'
)
assert errors(TimeDelta, 10**400)[0]['type'] == 'time_delta_type'
assert errors(TimeDelta, Decimal('1'), strict=True)[0]['type'] == 'time_delta_type'
# the extremes; a negative duration within 999,999,999 days can still fall below
# `timedelta.min`, which raises rather than reports, in every input form
max_days = 86400 * 999999999
assert TimeDelta.validate_python(-max_days) == timedelta(days=-999999999)
assert TimeDelta.validate_python(max_days + 1) == timedelta(days=999999999, seconds=1)
assert TimeDelta.validate_python('-P999999999D') == timedelta(days=-999999999)
assert errors(TimeDelta, -(max_days + 86400))[0]['type'] == 'time_delta_parsing'
for below_min in [-(max_days + 1), -(max_days + 1.5), '-P999999999DT1S', b'-P999999999DT1S', '-P999999999DT0.5S']:
    try:
        TimeDelta.validate_python(below_min)
        assert False, 'expected an OverflowError'
    except OverflowError as exc:
        assert str(exc) == 'days=-1000000000; must have magnitude <= 999999999'
for below_min in [str(-(max_days + 1)), str(-(max_days + 1.5)), '"-P999999999DT1S"']:
    try:
        TimeDelta.validate_json(below_min)
        assert False, 'expected an OverflowError'
    except OverflowError as exc:
        assert str(exc) == 'days=-1000000000; must have magnitude <= 999999999'

# === Inside containers ===
Events = TypeAdapter(dict[str, list[datetime]])
assert Events.validate_python({'a': ['2020-01-01', 1577836800]}) == {
    'a': [datetime(2020, 1, 1), datetime(2020, 1, 1, tzinfo=UTC)]
}
assert errors(Events, {'a': ['x']})[0]['loc'] == ('a', 0)
