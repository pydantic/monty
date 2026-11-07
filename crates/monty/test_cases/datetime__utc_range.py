# /// monty
# skip-cpython-windows = true
# ///
# utcfromtimestamp() across the whole datetime range. CPython on Windows converts through the C
# runtime's gmtime, which refuses timestamps far before 1970 or after the year 3000 with
# `OSError: [Errno 22]`, so only the Monty side of this case runs there. Monty gives
# CPython-on-POSIX results on every host. Every expectation is CPython 3.14 on Linux.
from datetime import datetime


def _raises(func, *args, **kwargs):
    try:
        func(*args, **kwargs)
    except Exception as e:
        return type(e).__name__ + ': ' + str(e)
    return 'no error'


# epoch milliseconds before 1970
assert repr(datetime.utcfromtimestamp(-1762546800123 / 1000)) == 'datetime.datetime(1914, 2, 24, 3, 39, 59, 877000)'

# the whole range, and the instant just outside it
assert datetime.utcfromtimestamp(-62135596800) == datetime(1, 1, 1)
assert datetime.utcfromtimestamp(253402300799) == datetime(9999, 12, 31, 23, 59, 59)
for _ts, _msg in [
    (253402300800, 'ValueError: year must be in 1..9999, not 10000'),
    (-62135596801, 'ValueError: year must be in 1..9999, not 0'),
    (253402300799.9999999, 'ValueError: year must be in 1..9999, not 10000'),
    (1e20, 'OverflowError: timestamp out of range for platform time_t'),
    (float('inf'), 'OverflowError: timestamp out of range for platform time_t'),
    (2**70, 'OverflowError: timestamp out of range for platform time_t'),
    (float('nan'), 'ValueError: Invalid value NaN (not a number)'),
    ('x', 'TypeError: argument must be int or float, not str'),
    (None, 'TypeError: argument must be int or float, not NoneType'),
    (1 + 0j, 'TypeError: argument must be int or float, not complex'),
]:
    assert _raises(lambda *a, **k: datetime.utcfromtimestamp(*a, **k), _ts) == _msg, _ts
