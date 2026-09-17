# An aware `time` owns a reference to its tzinfo object, so dropping the time
# must hand that reference back. Regression test: the owned id has to be listed
# in `py_dec_ref_ids_for_data`, otherwise every aware time leaks its timezone.
from datetime import date, datetime, time, timedelta, timezone

tz = timezone(timedelta(hours=1), 'P1')


def make_and_drop():
    t = time(12, 0, tzinfo=tz)
    # reading `.tzinfo` hands out a new reference, which must be released too
    assert t.tzinfo is tz
    return t.hour


def via_datetime():
    # `timetz()` re-attaches the datetime's timezone to a fresh time
    t = datetime(2020, 1, 1, 12, tzinfo=tz).timetz()
    assert t.tzinfo is tz
    return t.hour


def via_replace():
    # keeping, swapping and clearing the zone all have to balance out
    t = time(12, 0, tzinfo=tz)
    assert t.replace(minute=30).tzinfo is tz
    assert t.replace(tzinfo=timezone(timedelta(hours=2))).tzinfo is not tz
    assert t.replace(tzinfo=None).tzinfo is None
    return t.hour


def via_fromisoformat():
    # an offset in the string allocates a timezone the time then owns
    t = time.fromisoformat('12:00+01:00')
    assert t.tzinfo is not tz
    assert time.fromisoformat('12:00').tzinfo is None
    return t.hour


def via_combine():
    # `combine` inherits the time's zone, and an explicit third argument
    # replaces it — including with a temporary that only the call holds
    assert datetime.combine(date(2020, 1, 1), time(12, 0, tzinfo=tz)).tzinfo is tz
    assert datetime.combine(date(2020, 1, 1), time(12, 0, tzinfo=tz), timezone(timedelta(hours=2))).tzinfo is not tz
    assert datetime.combine(date(2020, 1, 1), time(12, 0, tzinfo=tz), None).tzinfo is None
    return 12


def via_combine_errors():
    # the third argument is bound before either type check runs, so a bad
    # argument 1 or 2 must still release the timezone the call owns
    for bad in ('x', 5):
        try:
            datetime.combine(bad, time(12, 0), timezone(timedelta(hours=2)))
            assert False, 'expected combine() argument 1 to be rejected'
        except TypeError:
            pass
        try:
            datetime.combine(date(2020, 1, 1), bad, timezone(timedelta(hours=2)))
            assert False, 'expected combine() argument 2 to be rejected'
        except TypeError:
            pass
    return 12


def via_tzinfo_methods():
    # `timezone.utcoffset(dt)` owns the datetime it is handed
    assert tz.utcoffset(datetime(2020, 1, 1)) == timedelta(hours=1)
    assert time(12, 0, tzinfo=tz).utcoffset() == timedelta(hours=1)
    return 12


for _ in range(3):
    assert make_and_drop() == 12
    assert via_datetime() == 12
    assert via_replace() == 12
    assert via_fromisoformat() == 12
    assert via_combine() == 12
    assert via_combine_errors() == 12
    assert via_tzinfo_methods() == 12
# ref-counts={'tz': 1}
