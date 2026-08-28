# An aware `time` owns a reference to its tzinfo object, so dropping the time
# must hand that reference back. Regression test: the owned id has to be listed
# in `py_dec_ref_ids_for_data`, otherwise every aware time leaks its timezone.
from datetime import datetime, time, timedelta, timezone

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


def via_tzinfo_methods():
    # `utcoffset()` reads the retained zone without taking a reference to it
    assert time(12, 0, tzinfo=tz).utcoffset() == timedelta(hours=1)
    return 12


for _ in range(3):
    assert make_and_drop() == 12
    assert via_datetime() == 12
    assert via_replace() == 12
    assert via_fromisoformat() == 12
    assert via_tzinfo_methods() == 12
# ref-counts={'tz': 1}
