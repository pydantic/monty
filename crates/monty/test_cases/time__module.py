# call-external
# The `time` stdlib module's clocks and `sleep()`. The conversion functions are in
# time__struct_time.py; the zone-dependent ones in datetime__zone_*.py.
import time


def check(fn, expected):
    try:
        fn()
        raise AssertionError('expected failure')
    except Exception as exc:
        got = f'{type(exc).__name__}: {exc}'
        assert got == expected, f'{got!r} != {expected!r}'


# === time.time() ===
start = time.time()
assert isinstance(start, float)
# well past 2020, and not so far out that the clock is obviously wrong
assert 1_600_000_000.0 < start < 32_000_000_000.0
assert time.time() >= start

# === the other wall clocks ===
# all of these read the same session clock, so they agree to within the time the
# assertions themselves take
assert isinstance(time.time_ns(), int)
assert time.time_ns() > 1_600_000_000_000_000_000
assert abs(time.time_ns() / 1e9 - time.time()) < 60
for clock in (time.monotonic, time.perf_counter):
    first = clock()
    assert isinstance(first, float)
    assert clock() >= first

# === the process clocks ===
# only the shape here: Monty's default `process_time='zero'` makes these constant,
# which CPython cannot match, so os_policy.rs asserts the values
for process_clock in (time.process_time, time.thread_time):
    assert isinstance(process_clock(), float)
    assert process_clock() >= 0.0
for process_clock_ns in (time.process_time_ns, time.thread_time_ns):
    assert isinstance(process_clock_ns(), int)
    assert process_clock_ns() >= 0

# === zone constants ===
# only the shape here: the values are asserted in datetime__zone_default.py, which
# skips CPython on Windows because the harness cannot set its zone there
assert type(time.timezone) is int
assert type(time.altzone) is int
assert time.daylight in (0, 1)
assert -86400 < time.timezone < 86400
assert -86400 < time.altzone < 86400
assert type(time.tzname) is tuple
assert len(time.tzname) == 2
assert all(type(name) is str for name in time.tzname)

# === time.sleep() ===
assert time.sleep(0) is None
assert time.sleep(0.001) is None
assert time.sleep(1e-9) is None
# ints, bools and long ints are all accepted lengths
assert time.sleep(0) is None
assert time.sleep(False) is None
assert time.time() >= start

# === signature errors ===
check(lambda: time.time(1), 'TypeError: time.time() takes no arguments (1 given)')
check(lambda: time.time(x=1), 'TypeError: time.time() takes no keyword arguments')
check(lambda: time.sleep(), 'TypeError: time.sleep() takes exactly one argument (0 given)')
check(lambda: time.sleep(0, 1), 'TypeError: time.sleep() takes exactly one argument (2 given)')
check(lambda: time.sleep(secs=0), 'TypeError: time.sleep() takes no keyword arguments')
# every other clock takes nothing at all, and says so the same way
for name, zero_arg in (
    ('time_ns', time.time_ns),
    ('monotonic', time.monotonic),
    ('monotonic_ns', time.monotonic_ns),
    ('perf_counter', time.perf_counter),
    ('perf_counter_ns', time.perf_counter_ns),
    ('process_time', time.process_time),
    ('process_time_ns', time.process_time_ns),
    ('thread_time', time.thread_time),
    ('thread_time_ns', time.thread_time_ns),
):
    check(lambda fn=zero_arg: fn(1), f'TypeError: time.{name}() takes no arguments (1 given)')
    check(lambda fn=zero_arg: fn(x=1), f'TypeError: time.{name}() takes no keyword arguments')

# === bad sleep lengths ===
# a float passes straight through, anything else must be an integer, so a type
# with only __float__ is rejected the same way a str is
check(lambda: time.sleep('a'), "TypeError: 'str' object cannot be interpreted as an integer or float")
check(lambda: time.sleep(None), "TypeError: 'NoneType' object cannot be interpreted as an integer or float")
check(lambda: time.sleep([0]), "TypeError: 'list' object cannot be interpreted as an integer or float")
# an __index__-able class is an acceptable length, a __float__-only one is not


class Index:
    def __index__(self) -> int:
        return 0


class Float:
    def __float__(self) -> float:
        return 0.0


assert time.sleep(Index()) is None
check(lambda: time.sleep(Float()), "TypeError: 'Float' object cannot be interpreted as an integer or float")

check(lambda: time.sleep(-1), 'ValueError: sleep length must be non-negative')
check(lambda: time.sleep(-0.001), 'ValueError: sleep length must be non-negative')
check(lambda: time.sleep(float('nan')), 'ValueError: Invalid value NaN (not a number)')
check(lambda: time.sleep(1e18), 'OverflowError: timestamp out of range for C PyTime_t')
check(lambda: time.sleep(float('inf')), 'OverflowError: timestamp out of range for C PyTime_t')
check(lambda: time.sleep(10**30), 'OverflowError: timestamp out of range for C PyTime_t')
# out of range outranks negative, as CPython converts before checking the sign
check(lambda: time.sleep(float('-inf')), 'OverflowError: timestamp out of range for C PyTime_t')
check(lambda: time.sleep(-1e18), 'OverflowError: timestamp out of range for C PyTime_t')
check(lambda: time.sleep(-(10**30)), 'OverflowError: timestamp out of range for C PyTime_t')
