from datetime import time, timedelta, timezone

# === import / basic construction ===
assert isinstance(time(), time), 'time() should construct a time instance'
assert isinstance(time(12), time), 'time(hour) should construct a time instance'
assert isinstance(time(12, 30), time), 'time(hour, minute) should construct a time instance'
assert isinstance(time(12, 30, 45), time), 'time(hour, minute, second) should construct a time instance'
assert isinstance(time(12, 30, 45, 123456), time), (
    'time(hour, minute, second, microsecond) should construct a time instance'
)

# === attribute access ===
t = time(14, 30, 45, 123456)
assert t.hour == 14, 'time.hour should return hour'
assert t.minute == 30, 'time.minute should return minute'
assert t.second == 45, 'time.second should return second'
assert t.microsecond == 123456, 'time.microsecond should return microsecond'
assert t.tzinfo is None, 'naive time.tzinfo should be None'
assert t.fold == 0, 'default time.fold should be 0'

# === default construction ===
t_default = time()
assert t_default.hour == 0, 'default time.hour should be 0'
assert t_default.minute == 0, 'default time.minute should be 0'
assert t_default.second == 0, 'default time.second should be 0'
assert t_default.microsecond == 0, 'default time.microsecond should be 0'
assert t_default.tzinfo is None, 'default time.tzinfo should be None'
assert t_default.fold == 0, 'default time.fold should be 0'

# === boundary values ===
t_min = time(0, 0, 0, 0)
assert t_min.hour == 0, 'time(0, 0, 0, 0).hour'
assert t_min.microsecond == 0, 'time(0, 0, 0, 0).microsecond'

t_max = time(23, 59, 59, 999999)
assert t_max.hour == 23, 'time(23, 59, 59, 999999).hour'
assert t_max.minute == 59, 'time(23, 59, 59, 999999).minute'
assert t_max.second == 59, 'time(23, 59, 59, 999999).second'
assert t_max.microsecond == 999999, 'time(23, 59, 59, 999999).microsecond'

# === keyword construction ===
assert time(hour=12, minute=30) == time(12, 30), 'time keyword construction'
assert time(12, minute=30, second=45) == time(12, 30, 45), 'time mixed positional/keyword construction'
assert time(microsecond=123) == time(0, 0, 0, 123), 'time(microsecond=...) keyword-only style'

# === repr/str ===
assert repr(time(0, 0)) == 'datetime.time(0, 0)', 'repr of midnight time'
assert repr(time(12, 30)) == 'datetime.time(12, 30)', 'repr of time without seconds'
assert repr(time(12, 30, 45)) == 'datetime.time(12, 30, 45)', 'repr of time with seconds'
assert repr(time(12, 30, 45, 123456)) == 'datetime.time(12, 30, 45, 123456)', 'repr of time with microseconds'
assert repr(time(0, 0, 0, 123456)) == 'datetime.time(0, 0, 0, 123456)', 'repr of time with only microseconds'

assert str(time(0, 0)) == '00:00:00', 'str of midnight time'
assert str(time(12, 30)) == '12:30:00', 'str of time without seconds'
assert str(time(12, 30, 45)) == '12:30:45', 'str of time with seconds'
assert str(time(12, 30, 45, 123456)) == '12:30:45.123456', 'str of time with microseconds'

# === isoformat ===
assert time(0, 0).isoformat() == '00:00:00', 'isoformat of midnight'
assert time(12, 30, 45).isoformat() == '12:30:45', 'isoformat with seconds'
assert time(12, 30, 45, 123456).isoformat() == '12:30:45.123456', 'isoformat with microseconds'

# === equality ===
assert time(12, 30) == time(12, 30), 'time equality, same values'
assert time(12, 30) == time(12, 30, 0, 0), 'time equality, implicit defaults'
assert time(12, 30) != time(12, 31), 'time inequality'
assert time(12, 30, 45) != time(12, 30, 45, 1), 'time microseconds matter'
assert time(0, 0) != time(0, 0, 1), 'time seconds matter'

# === ordering comparisons (naive times) ===
assert time(10, 0) < time(11, 0), 'time < comparison'
assert time(11, 0) > time(10, 0), 'time > comparison'
assert time(10, 0) <= time(10, 0), 'time <= equal'
assert time(10, 0) >= time(10, 0), 'time >= equal'
assert not (time(10, 0) > time(11, 0)), 'time not >'
assert time(0, 0) < time(23, 59, 59, 999999), 'min < max'

# === hashability ===
assert hash(time(12, 30)) == hash(time(12, 30)), 'equal times should hash equal'
assert {time(12, 30)} == {time(12, 30)}, 'time in set'
assert {time(12, 30): 'a'}[time(12, 30)] == 'a', 'time as dict key'

# === truthiness: all time objects are truthy (including midnight) ===
# CPython: time(0, 0) is truthy because only timedelta(0) is falsy.
assert bool(time(0, 0)), 'time(0, 0) should be truthy'
assert bool(time(12, 30)), 'time(12, 30) should be truthy'

# === aware time with tzinfo ===
utc = timezone.utc
t_utc = time(12, 30, tzinfo=utc)
assert t_utc.tzinfo is utc, 'time.tzinfo should preserve the timezone object identity'
assert t_utc.hour == 12, 'aware time.hour'
assert t_utc.minute == 30, 'aware time.minute'

plus1 = timezone(timedelta(hours=1), 'P1')
t_plus1 = time(12, 30, tzinfo=plus1)
assert t_plus1.tzinfo is plus1, 'time.tzinfo should preserve named tz identity'
assert repr(t_plus1) == "datetime.time(12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'P1'))", (
    'aware time repr should include tzinfo'
)
assert repr(t_utc) == 'datetime.time(12, 30, tzinfo=datetime.timezone.utc)', 'aware UTC time repr'
assert str(t_utc) == '12:30:00+00:00', 'aware time str should include offset'
assert t_utc.isoformat() == '12:30:00+00:00', 'aware time isoformat should include offset'
assert str(t_plus1) == '12:30:00+01:00', 'aware time str with +01:00 offset'

# === aware/naive equality: naive != aware ===
assert (time(12, 30) == t_utc) is False, 'naive time should not equal aware time'
assert (time(12, 30) != t_utc) is True, 'naive/aware inequality should be True'

# === aware time equality: different tz but same offset should compare equal ===
utc2 = timezone(timedelta(0))
t_utc2 = time(12, 30, tzinfo=utc2)
assert t_utc == t_utc2, 'aware times with equal offsets should compare equal'

# === tzinfo identity stability across attribute access ===
t_id = time(1, 2, 3, tzinfo=plus1)
assert t_id.tzinfo is t_id.tzinfo, 'time.tzinfo should be stable across repeated attribute access'

# === argument validation ===
try:
    time(24)
    assert False, 'hour 24 should raise ValueError'
except ValueError as e:
    assert str(e) == 'hour must be in 0..23, not 24', f'hour OOB message: {e}'

try:
    time(-1)
    assert False, 'hour -1 should raise ValueError'
except ValueError as e:
    assert str(e) == 'hour must be in 0..23, not -1', f'hour negative message: {e}'

try:
    time(0, 60)
    assert False, 'minute 60 should raise ValueError'
except ValueError as e:
    assert str(e) == 'minute must be in 0..59, not 60', f'minute OOB message: {e}'

try:
    time(0, -1)
    assert False, 'minute -1 should raise ValueError'
except ValueError as e:
    assert str(e) == 'minute must be in 0..59, not -1', f'minute negative message: {e}'

try:
    time(0, 0, 60)
    assert False, 'second 60 should raise ValueError'
except ValueError as e:
    assert str(e) == 'second must be in 0..59, not 60', f'second OOB message: {e}'

try:
    time(0, 0, 0, 1000000)
    assert False, 'microsecond 1000000 should raise ValueError'
except ValueError as e:
    assert str(e) == 'microsecond must be in 0..999999, not 1000000', f'microsecond OOB message: {e}'

try:
    time(0, 0, 0, -1)
    assert False, 'microsecond -1 should raise ValueError'
except ValueError as e:
    assert str(e) == 'microsecond must be in 0..999999, not -1', f'microsecond negative message: {e}'

# === unknown keyword argument ===
try:
    time(0, foo=1)
    assert False, 'unknown kwarg should raise TypeError'
except TypeError as e:
    assert str(e) == "this function got an unexpected keyword argument 'foo'", f'time unknown kwarg: {e}'

# === tzinfo type validation ===
try:
    time(0, 0, tzinfo='UTC')
    assert False, 'string tzinfo should raise TypeError'
except TypeError as e:
    assert str(e) == "tzinfo argument must be None or of a tzinfo subclass, not type 'str'", f'time tzinfo type: {e}'

# === fold argument ===
assert time(12, 0, fold=0).fold == 0, 'time with fold=0'
assert time(12, 0, fold=1).fold == 1, 'time with fold=1'
assert repr(time(12, 0, fold=1)) == 'datetime.time(12, 0, fold=1)', 'fold=1 should appear in repr'
assert repr(time(12, 0, fold=0)) == 'datetime.time(12, 0)', 'fold=0 should not appear in repr'

try:
    time(12, 0, fold=2)
    assert False, 'fold=2 should raise ValueError'
except ValueError as e:
    assert str(e) == 'fold must be either 0 or 1, not 2', f'time fold OOB: {e}'
