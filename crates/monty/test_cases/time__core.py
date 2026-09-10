from datetime import date, datetime, time, timedelta, timezone

# === construction / defaults ===
assert isinstance(time(), time), 'time() builds a time'
assert isinstance(time(12, 30, 45, 123456), time), 'all four components accepted'

t = time()
assert (t.hour, t.minute, t.second, t.microsecond, t.fold) == (0, 0, 0, 0, 0)
assert t.tzinfo is None

t = time(14, 30, 45, 123456)
assert (t.hour, t.minute, t.second, t.microsecond) == (14, 30, 45, 123456)
assert t.tzinfo is None

# === boundary values ===
assert (time(0, 0, 0, 0).hour, time(0, 0, 0, 0).microsecond) == (0, 0)
t_max = time(23, 59, 59, 999999)
assert (t_max.hour, t_max.minute, t_max.second, t_max.microsecond) == (23, 59, 59, 999999)

# === keyword construction ===
assert time(hour=12, minute=30) == time(12, 30)
assert time(12, minute=30, second=45) == time(12, 30, 45)
assert time(microsecond=123) == time(0, 0, 0, 123)
# `bool` is an `int` subclass, so it is accepted as a component
assert time(True) == time(1, 0)

# === repr ===
assert repr(time(0, 0)) == 'datetime.time(0, 0)'
assert repr(time(12, 30)) == 'datetime.time(12, 30)'
assert repr(time(12, 30, 45)) == 'datetime.time(12, 30, 45)'
assert repr(time(12, 30, 45, 123456)) == 'datetime.time(12, 30, 45, 123456)'
# a zero second is still printed when microseconds are present
assert repr(time(0, 0, 0, 123456)) == 'datetime.time(0, 0, 0, 123456)'

# === str / isoformat ===
assert str(time(0, 0)) == '00:00:00'
assert str(time(12, 30)) == '12:30:00'
assert str(time(12, 30, 45, 123456)) == '12:30:45.123456'
assert time(0, 0).isoformat() == '00:00:00'
assert time(12, 30, 45).isoformat() == '12:30:45'
assert time(12, 30, 45, 123456).isoformat() == '12:30:45.123456'
# f-strings with an empty spec go through str()
assert f'{time(12, 30)}' == '12:30:00'

# === isoformat(timespec) ===
# `auto` (the default) prints microseconds only when there are any, which is
# what makes isoformat() round-trip through fromisoformat().
_t = time(1, 2, 3, 4)
assert _t.isoformat('auto') == '01:02:03.000004'
assert time(1, 2, 3).isoformat('auto') == '01:02:03'
assert _t.isoformat('hours') == '01'
assert _t.isoformat('minutes') == '01:02'
assert _t.isoformat('seconds') == '01:02:03'
assert _t.isoformat('milliseconds') == '01:02:03.000'
assert _t.isoformat('microseconds') == '01:02:03.000004'
assert _t.isoformat(timespec='minutes') == '01:02'
# sub-second digits are truncated, not rounded
assert time(0, 0, 0, 999999).isoformat('milliseconds') == '00:00:00.999'
# the offset is appended whatever the precision
assert time(1, 2, 3, 4, tzinfo=timezone.utc).isoformat('minutes') == '01:02+00:00'
assert time(1, 2, tzinfo=timezone(timedelta(seconds=30))).isoformat('hours') == '01+00:00:30'

try:
    _t.isoformat('bogus')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'Unknown timespec value'

for _args, _kwargs, _msg in [
    ((5,), {}, 'isoformat() argument 1 must be str, not int'),
    ((None,), {}, 'isoformat() argument 1 must be str, not None'),
    (('auto', 'x'), {}, 'isoformat() takes at most 1 argument (2 given)'),
    ((), {'spec': 'minutes'}, "isoformat() got an unexpected keyword argument 'spec'"),
]:
    try:
        _t.isoformat(*_args, **_kwargs)
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == _msg

# === equality ===
assert time(12, 30) == time(12, 30, 0, 0)
assert time(12, 30) != time(12, 31)
assert time(12, 30, 45) != time(12, 30, 45, 1)
assert time(0, 0) != time(0, 0, 1)
# a time never equals a non-time, and says so rather than raising
assert (time(12, 30) == 5) is False
assert (time(12, 30) == 'x') is False

# === ordering (naive) ===
assert time(10, 0) < time(11, 0)
assert time(11, 0) > time(10, 0)
assert time(10, 0) <= time(10, 0)
assert time(10, 0) >= time(10, 0)
assert not (time(10, 0) > time(11, 0)), 'ordering is strict'
assert time(0, 0) < time(23, 59, 59, 999999)
assert sorted([time(3), time(1), time(2)]) == [time(1), time(2), time(3)]

# === hashing ===
assert hash(time(12, 30)) == hash(time(12, 30))
assert {time(12, 30)} == {time(12, 30)}
assert {time(12, 30): 'a'}[time(12, 30)] == 'a'

# === truthiness: every time is truthy, midnight included ===
assert bool(time(0, 0)), 'time(0, 0) is truthy unlike timedelta(0)'
assert bool(time(12, 30)), 'a non-midnight time is truthy'

# === aware times ===
utc = timezone.utc
plus1 = timezone(timedelta(hours=1), 'P1')
t_utc = time(12, 30, tzinfo=utc)
t_plus1 = time(12, 30, tzinfo=plus1)

# the caller's timezone object is kept rather than copied, so identity
# survives both construction and repeated access
assert t_utc.tzinfo is utc
assert t_plus1.tzinfo is plus1
assert t_plus1.tzinfo is t_plus1.tzinfo
assert (t_utc.hour, t_utc.minute) == (12, 30)

assert repr(t_utc) == 'datetime.time(12, 30, tzinfo=datetime.timezone.utc)'
assert repr(t_plus1) == "datetime.time(12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'P1'))"
assert str(t_utc) == '12:30:00+00:00'
assert str(t_plus1) == '12:30:00+01:00'
assert t_utc.isoformat() == '12:30:00+00:00'

# aware and naive never compare equal
assert (time(12, 30) == t_utc) is False
assert (time(12, 30) != t_utc) is True

# aware times compare on their offset-adjusted clock, so different zones can be equal
assert t_utc == time(12, 30, tzinfo=timezone(timedelta(0)))
assert time(12, 0, tzinfo=plus1) == time(11, 0, tzinfo=utc)
assert time(12, 0, tzinfo=plus1) < time(12, 0, tzinfo=utc)
assert {time(12, 0, tzinfo=utc): 1}[time(12, 0, tzinfo=timezone(timedelta(0)))] == 1

# ...but there is no wrap-around into a 24 hour day
assert time(1, 0, tzinfo=utc) != time(23, 0, tzinfo=timezone(timedelta(hours=-2)))

# sub-minute offsets keep their seconds, and are not rounded away
tz30s = timezone(timedelta(seconds=30))
assert time(12, 0, tzinfo=tz30s) != time(12, 0, tzinfo=utc)
assert time(12, 0, tzinfo=tz30s).isoformat() == '12:00:00+00:00:30'

# === fold ===
assert time(12, 0, fold=0).fold == 0
assert time(12, 0, fold=1).fold == 1
assert repr(time(12, 0, fold=1)) == 'datetime.time(12, 0, fold=1)'
assert repr(time(12, 0, fold=0)) == 'datetime.time(12, 0)'
# fold is excluded from equality and hashing, as in CPython
assert time(12, 0, fold=1) == time(12, 0, fold=0)
assert hash(time(12, 0, fold=1)) == hash(time(12, 0, fold=0))

# === component range validation ===
for args, expected in [
    ((24,), 'hour must be in 0..23, not 24'),
    ((-1,), 'hour must be in 0..23, not -1'),
    ((0, 60), 'minute must be in 0..59, not 60'),
    ((0, -1), 'minute must be in 0..59, not -1'),
    ((0, 0, 60), 'second must be in 0..59, not 60'),
    ((0, 0, 0, 1000000), 'microsecond must be in 0..999999, not 1000000'),
    ((0, 0, 0, -1), 'microsecond must be in 0..999999, not -1'),
]:
    try:
        time(*args)
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == expected

try:
    time(12, 0, fold=2)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'fold must be either 0 or 1, not 2'

# components are validated before tzinfo, so the hour wins here
try:
    time(25, tzinfo='UTC')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'hour must be in 0..23, not 25'

# === argument binding errors ===
try:
    time(1, 2, 3, 4, None, 5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'function takes at most 5 positional arguments (6 given)'

try:
    time(1, 2, 3, 4, None, 5, 6)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'function takes at most 6 arguments (7 given)'

try:
    time(1, hour=2)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "argument for function given by name ('hour') and position (1)"

try:
    time(0, foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "this function got an unexpected keyword argument 'foo'"

try:
    time(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'float' object cannot be interpreted as an integer"

# === tzinfo type validation ===
# (a `timedelta` argument is excluded: Monty names it `timedelta` where CPython
# says `datetime.timedelta` — see limitations/datetime.md)
for bad, name in [('UTC', 'str'), (5, 'int'), ([], 'list')]:
    try:
        time(0, 0, tzinfo=bad)
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == f"tzinfo argument must be None or of a tzinfo subclass, not type '{name}'"

# === replace ===
# `replace()` is keyword-only in Monty (CPython also accepts positionals — see
# limitations/datetime.md), and carries over every field the caller omits.
t_rep = time(12, 30, 45, 123456, tzinfo=plus1, fold=1)
assert repr(t_rep.replace()) == repr(t_rep)
assert t_rep.replace().tzinfo is plus1
assert t_rep.replace(hour=1) == time(1, 30, 45, 123456, tzinfo=plus1)
assert t_rep.replace(hour=1).fold == 1
assert t_rep.replace(minute=0, second=0, microsecond=0) == time(12, 0, tzinfo=plus1)
assert t_rep.replace(fold=0).fold == 0
assert time(1, 2).replace(hour=3) == time(3, 2)
assert time(1, 2).replace(hour=3).tzinfo is None

# tzinfo can be cleared or swapped
assert t_rep.replace(tzinfo=None) == time(12, 30, 45, 123456)
assert t_rep.replace(tzinfo=None).tzinfo is None
assert t_rep.replace(tzinfo=utc).tzinfo is utc

# replaced components are validated exactly like the constructor's
for kwargs, expected in [
    ({'hour': 24}, 'hour must be in 0..23, not 24'),
    ({'minute': -1}, 'minute must be in 0..59, not -1'),
    ({'second': 60}, 'second must be in 0..59, not 60'),
    ({'microsecond': 1000000}, 'microsecond must be in 0..999999, not 1000000'),
    ({'fold': 2}, 'fold must be either 0 or 1, not 2'),
]:
    try:
        t_rep.replace(**kwargs)
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == expected

try:
    t_rep.replace(foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "replace() got an unexpected keyword argument 'foo'"

try:
    t_rep.replace(tzinfo=5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "tzinfo argument must be None or of a tzinfo subclass, not type 'int'"

# === strftime ===
# A bare time has no date, so date directives render CPython's 1900-01-01 anchor.
assert time(12, 30, 45).strftime('%H:%M:%S') == '12:30:45'
assert time(13, 5).strftime('%I %p') == '01 PM'
assert time(12, 30).strftime('%Y-%m-%d') == '1900-01-01'
assert time(12, 30).strftime('') == ''
assert time(12, 30).strftime('no directives') == 'no directives'

try:
    time(1).strftime()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "strftime() missing required argument 'format' (pos 1)"

try:
    time(1).strftime('%H', '%M')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'strftime() takes at most 1 argument (2 given)'

try:
    time(1).strftime(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'strftime() argument 1 must be str, not int'

# f-string format specs go through strftime, as they do for date and datetime
assert f'{time(12, 30):%H-%M}' == '12-30'
assert f'{time(12, 30, 45):Time %H:%M:%S!}' == 'Time 12:30:45!'
_time_fmt = '%H/%M'
assert f'{time(12, 30):{_time_fmt}}' == '12/30'
# an empty spec still falls back to str()
assert f'{time(12, 30):}' == '12:30:00'

# === fromisoformat ===
assert time.fromisoformat('00:00') == time(0, 0)
assert time.fromisoformat('12:30:05') == time(12, 30, 5)
assert time.fromisoformat('12:30:05.000007') == time(12, 30, 5, 7)
assert time.fromisoformat('12:30:05.123') == time(12, 30, 5, 123000)
assert time.fromisoformat('12:30').tzinfo is None
assert time.fromisoformat('12:30+01:00') == time(12, 30, tzinfo=timezone(timedelta(hours=1)))
assert time.fromisoformat('12:30:05-03:30') == time(12, 30, 5, tzinfo=timezone(timedelta(hours=-3, minutes=-30)))
# a `Z` suffix canonicalizes to the `timezone.utc` singleton, as it does in CPython
assert repr(time.fromisoformat('12:30:05Z')) == 'datetime.time(12, 30, 5, tzinfo=datetime.timezone.utc)'
assert time.fromisoformat('12:30:05Z').tzinfo is utc
# round-tripping isoformat() is exact
assert time.fromisoformat(time(1, 2, 3, 4, tzinfo=utc).isoformat()) == time(1, 2, 3, 4, tzinfo=utc)

try:
    time.fromisoformat('not-a-time')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "Invalid isoformat string: 'not-a-time'"

try:
    time.fromisoformat(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'fromisoformat: argument must be str'

try:
    time.fromisoformat()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'time.fromisoformat() takes exactly one argument (0 given)'

# === utcoffset / tzname / dst ===
# A naive time answers None to all three; only fixed offsets exist, so `dst()`
# is always None even when aware.
assert time(12, 30).utcoffset() is None
assert time(12, 30).tzname() is None
assert time(12, 30).dst() is None
assert time(12, 30, tzinfo=utc).utcoffset() == timedelta(0)
assert time(12, 30, tzinfo=plus1).utcoffset() == timedelta(hours=1)
assert time(12, 30, tzinfo=timezone(timedelta(hours=-3, minutes=-30))).utcoffset() == timedelta(hours=-3, minutes=-30)
assert time(12, 30, tzinfo=plus1).dst() is None

# an unnamed timezone reports its offset, `timezone.utc` reports 'UTC'
assert time(12, 30, tzinfo=plus1).tzname() == 'P1'
assert time(12, 30, tzinfo=utc).tzname() == 'UTC'
assert time(12, 30, tzinfo=timezone(timedelta(hours=-3, minutes=-30))).tzname() == 'UTC-03:30'
assert time(12, 30, tzinfo=timezone(timedelta(seconds=30))).tzname() == 'UTC+00:00:30'

try:
    time(1).utcoffset(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'time.utcoffset() takes no arguments (1 given)'

try:
    time(1).tzname(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'time.tzname() takes no arguments (1 given)'

try:
    time(1).dst(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'time.dst() takes no arguments (1 given)'

# === built from a datetime ===
# `time()` drops the timezone, `timetz()` keeps the same object
dt_naive = datetime(2020, 1, 2, 3, 4, 5, 678901)
assert dt_naive.time() == time(3, 4, 5, 678901)
assert dt_naive.time().tzinfo is None
assert dt_naive.timetz() == dt_naive.time()

dt_aware = datetime(2020, 1, 2, 3, 4, tzinfo=plus1)
assert dt_aware.time() == time(3, 4)
assert dt_aware.time().tzinfo is None
assert dt_aware.timetz() == time(3, 4, tzinfo=plus1)
assert dt_aware.timetz().tzinfo is plus1
assert str(dt_aware.timetz()) == '03:04:00+01:00'
assert datetime(2020, 1, 1).time() == time(0, 0)

try:
    dt_aware.time(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'datetime.time() takes no arguments (1 given)'

try:
    dt_aware.timetz(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'datetime.timetz() takes no arguments (1 given)'

# === time is not a date subclass, unlike datetime ===
assert not isinstance(time(1), date), 'time is unrelated to date'
assert str(time) == "<class 'datetime.time'>"
