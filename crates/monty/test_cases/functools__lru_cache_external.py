# call-external
# A cached function is run as an ordinary frame, so it may suspend to the host
# part-way through and still store its result on the way back — the counters
# below only add up if the pending store survives the suspension.
import functools

calls = []


@functools.cache
def fetch(n):
    calls.append(n)
    return add_ints(n, 100)


assert fetch(1) == 101
assert fetch(1) == 101
assert fetch(2) == 102
assert calls == [1, 2]
assert fetch.cache_info() == (1, 2, None, 2)


# The host call may also sit under a nested frame, so several cached frames are
# waiting to store at once.
@functools.cache
def outer(n):
    return fetch(n) + inner(n)


@functools.cache
def inner(n):
    return concat_strings('x' * n, '!').count('x')


assert outer(2) == 104
assert outer(2) == 104
assert calls == [1, 2]
assert outer.cache_info() == (1, 1, None, 1)
assert inner.cache_info() == (0, 1, None, 1)

# === The wrapped callable itself may be external ===
direct = functools.cache(add_ints)
assert direct(2, 3) == 5
assert direct(2, 3) == 5
assert direct.cache_info() == (1, 1, None, 1)

stacked = functools.cache(functools.cache(concat_strings))
assert stacked('a', 'b') == 'ab'
assert stacked('a', 'b') == 'ab'
assert stacked.cache_info() == (1, 1, None, 1)
assert stacked.__wrapped__.cache_info() == (0, 1, None, 1)


# === An initializer may suspend before its instance is cached ===
class Box:
    def __init__(self, n):
        self.value = add_ints(n, 10)


cached_box = functools.cache(Box)
box = cached_box(5)
assert box.value == 15
assert cached_box(5) is box
assert cached_box.cache_info() == (1, 1, None, 1)
