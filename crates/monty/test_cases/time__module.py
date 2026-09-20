# call-external
# The `time` stdlib module: `time()` and `sleep()`, both served by the host.
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

# === zone constants ===
# a case with no `# timezone=` marker runs in UTC on both sides
assert time.timezone == 0
assert time.altzone == 0
assert time.daylight == 0
assert time.tzname == ('UTC', 'UTC')

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
