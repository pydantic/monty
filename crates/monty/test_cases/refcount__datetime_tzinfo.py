# An aware `datetime` owns a reference to its tzinfo object, so every path that
# attaches, carries or clears one has to balance. Regression test: `replace()`
# released the argument before `from_components` took its own reference, so a
# zone built in the call — `replace(tzinfo=timezone(...))` — was freed under it.
from datetime import date, datetime, time, timedelta, timezone

tz = timezone(timedelta(hours=1), 'P1')


def make_and_drop():
    d = datetime(2020, 1, 1, 12, tzinfo=tz)
    # reading `.tzinfo` hands out a new reference, which must be released too
    assert d.tzinfo is tz
    return d.hour


def via_replace():
    # keeping, swapping and clearing the zone all have to balance out
    d = datetime(2020, 1, 1, 12, tzinfo=tz)
    assert d.replace(minute=30).tzinfo is tz
    assert d.replace(tzinfo=None).tzinfo is None
    # the swap is the regression: only the call holds the new zone
    swapped = d.replace(tzinfo=timezone(timedelta(hours=2)))
    assert swapped.tzinfo is not tz
    assert swapped.utcoffset() == timedelta(hours=2)
    return d.hour


def via_replace_errors():
    # the kwargs are all bound before any component is validated, so a rejected
    # component must still release the timezone the call owns
    d = datetime(2020, 1, 1, 12)
    for bad in ('nope', 99):
        try:
            d.replace(month=bad, tzinfo=timezone(timedelta(hours=2)))
            assert False, 'expected replace() to reject the month'
        except (TypeError, ValueError):
            pass
    return d.hour


def via_naive_temporary():
    # a temporary zone also crosses `astimezone` and the constructor
    assert datetime(2020, 1, 1, 12, tzinfo=timezone(timedelta(hours=2))).utcoffset() == timedelta(hours=2)
    assert datetime(2020, 1, 1, 12, tzinfo=tz).astimezone(timezone(timedelta(hours=3))).hour == 14
    return 12


def via_timetz_and_combine():
    # `timetz()` re-attaches the datetime's zone to a fresh time, and `combine`
    # carries it the other way
    assert datetime(2020, 1, 1, 12, tzinfo=tz).timetz().tzinfo is tz
    assert datetime.combine(date(2020, 1, 1), time(12, 0), tz).tzinfo is tz
    return 12


def via_fromisoformat():
    # an offset in the string allocates a timezone the datetime then owns
    d = datetime.fromisoformat('2020-01-01T12:00+01:00')
    assert d.tzinfo is not tz
    assert datetime.fromisoformat('2020-01-01T12:00').tzinfo is None
    return d.hour


def via_tzinfo_methods():
    # `utcoffset()` / `tzname()` / `timestamp()` each read the owned zone
    d = datetime(2020, 1, 1, 12, tzinfo=tz)
    assert d.utcoffset() == timedelta(hours=1)
    assert d.tzname() == 'P1'
    assert d.timestamp() == 1577876400.0
    return d.hour


for _ in range(3):
    assert make_and_drop() == 12
    assert via_replace() == 12
    assert via_replace_errors() == 12
    assert via_naive_temporary() == 12
    assert via_timetz_and_combine() == 12
    assert via_fromisoformat() == 12
    assert via_tzinfo_methods() == 12
# ref-counts={'tz': 1}
