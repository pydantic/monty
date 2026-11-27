from collections import defaultdict, deque
from datetime import date, datetime, time, timedelta, timezone
from decimal import Context, Inexact, ROUND_DOWN
import re

dd = defaultdict(list)
factory = defaultdict.default_factory
assert factory.__get__(dd) is list
factory.__set__(dd, 3)
assert dd.default_factory == 3
try:
    dd['missing']
except TypeError:
    pass
else:
    assert False, 'expected a non-callable default factory to fail at the call'
dd.default_factory = None
assert factory.__get__(dd) is None

ctx = Context()
for name, value in [('prec', 8), ('rounding', ROUND_DOWN), ('Emin', -10), ('Emax', 10), ('capitals', 0), ('clamp', 1)]:
    descriptor = getattr(Context, name)
    descriptor.__set__(ctx, value)
    assert descriptor.__get__(ctx) == getattr(ctx, name) == value

flags = ctx.flags
new_flags = {signal: flags[signal] for signal in flags}
new_flags[Inexact] = True
ctx.flags = new_flags
assert flags[Inexact]
flags[Inexact] = False
assert not ctx.flags[Inexact]
traps = ctx.traps
new_traps = {signal: traps[signal] for signal in traps}
new_traps[Inexact] = True
ctx.traps = new_traps
assert traps[Inexact] and ctx.traps[Inexact]
assert not hasattr(Context, 'flags') and not hasattr(Context, 'traps')
try:
    ctx.prec = 0
except ValueError:
    assert ctx.prec == 8
else:
    assert False, 'expected precision validation'

items = deque([], 3)
assert deque.maxlen.__get__(items) == items.maxlen == 3
z = 2 + 3j
assert complex.real.__get__(z) == z.real == 2
assert complex.imag.__get__(z) == z.imag == 3
part = slice(None, 8, 2)
assert slice.start.__get__(part) is None
assert slice.stop.__get__(part) == 8
assert slice.step.__get__(part) == 2
try:
    slice.step.__set__(part, 1)
except AttributeError:
    assert part.step == 2
else:
    assert False, 'expected slice.step to be read-only'

pattern = re.compile('[a-z]+')
assert type(pattern).pattern.__get__(pattern) == pattern.pattern
assert type(pattern).flags.__get__(pattern) == pattern.flags
subject = 'a long lowercase string to retain in the match'
match = pattern.match(subject)
assert type(match).string.__get__(match) is subject
assert match.string is subject

tz = timezone.utc
day = date(2024, 2, 29)
stamp = datetime(2024, 2, 29, 1, 2, 3, 4, tzinfo=tz)
for name, value in [('year', 2024), ('month', 2), ('day', 29)]:
    descriptor = getattr(date, name)
    assert descriptor.__get__(day) == value
    assert descriptor.__get__(stamp) == getattr(stamp, name) == value
    assert getattr(datetime, name).__objclass__ is date

clock = time(1, 2, 3, 4, tzinfo=tz, fold=1)
for name, value in [('hour', 1), ('minute', 2), ('second', 3), ('microsecond', 4), ('tzinfo', tz)]:
    assert getattr(datetime, name).__get__(stamp) == value
    assert getattr(time, name).__get__(clock) == value
assert time.fold.__get__(clock) == 1
assert datetime.tzinfo.__get__(stamp) is tz
assert time.tzinfo.__get__(clock) is tz
period = timedelta(days=-1, seconds=2, microseconds=3)
assert timedelta.days.__get__(period) == -1
assert timedelta.seconds.__get__(period) == 2
assert timedelta.microseconds.__get__(period) == 3
