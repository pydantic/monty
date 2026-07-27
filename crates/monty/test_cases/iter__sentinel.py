# The two-argument iter(callable, sentinel) form.
assert type(iter(lambda: 1, 1)).__name__ == 'callable_iterator'

# === drive until sentinel ===
# Calls the callable repeatedly until its result == sentinel; the sentinel
# itself is never yielded, so values after it in the source are never seen.
data = iter([1, 2, 3, 0, 9])
assert list(iter(lambda: next(data), 0)) == [1, 2, 3]

# === self-iterable ===
ci = iter(lambda: 1, 0)
assert iter(ci) is ci

# === rich == comparison (by value, not identity) ===
# A freshly built list equals the sentinel by ==, so iteration stops at once.
assert list(iter(lambda: [1, 2], [1, 2])) == []
src = iter([[9], [1, 2], [3]])
assert list(iter(lambda: next(src), [1, 2])) == [[9]]

# === immediate sentinel -> empty ===
assert list(iter(lambda: 7, 7)) == []

# === `in` consumes it, like any other iterator ===
# It has no __contains__, so membership falls back to iteration and stops at the
# match, leaving the rest of the iterator available.
src3 = iter([1, 2, 3, 0])
partial = iter(lambda: next(src3), 0)
assert 2 in partial
assert list(partial) == [3]
src4 = iter([1, 2, 0])
assert 99 not in iter(lambda: next(src4), 0)
exhausted = iter(lambda: 7, 7)
assert 7 not in exhausted


# === callable exception propagates unchanged ===
def boom():
    raise ValueError('boom')


try:
    next(iter(boom, 0))
    assert False, 'expected ValueError to propagate'
except ValueError as e:
    assert str(e) == 'boom'

# === argument errors: iter is positional-only, 1..=2 args ===
try:
    iter()
    assert False, 'expected TypeError for no args'
except TypeError as e:
    assert str(e) == 'iter expected at least 1 argument, got 0'
try:
    iter([1], 2, 3)
    assert False, 'expected TypeError for three args'
except TypeError as e:
    assert str(e) == 'iter expected at most 2 arguments, got 3'
try:
    iter(x=[1])
    assert False, 'expected TypeError for a keyword argument'
except TypeError as e:
    assert str(e) == 'iter() takes no keyword arguments'

# === non-callable first argument ===
try:
    iter(5, 0)
    assert False, 'expected TypeError for non-callable first arg'
except TypeError as e:
    assert str(e) == 'iter(v, w): v must be callable'

# === next() default vs StopIteration on an exhausted callable_iterator ===
z = iter(lambda: 0, 0)
assert next(z, 'DEF') == 'DEF'
try:
    next(z)
    assert False, 'expected StopIteration with no default'
except StopIteration:
    pass


# === callable raising StopIteration propagates as-is (not swallowed) ===
def stop():
    raise StopIteration


try:
    next(iter(stop, 0))
    assert False, 'expected StopIteration to propagate'
except StopIteration:
    pass

# === for-loop drives it, then stays stopped after exhaustion ===
src2 = iter([5, 6, 0])
ci2 = iter(lambda: next(src2), 0)
collected = []
for x in ci2:
    collected.append(x)
assert collected == [5, 6]
assert next(ci2, 'STOPPED') == 'STOPPED'

# === StopIteration from the callable ends iteration, as in CPython ===
stop_calls = [0]


def stops():
    stop_calls[0] += 1
    if stop_calls[0] >= 3:
        raise StopIteration
    return stop_calls[0]


assert list(iter(stops, 99)) == [1, 2]
stop_calls[0] = 0
stop_it = iter(stops, 99)
assert (next(stop_it), next(stop_it)) == (1, 2)
assert next(stop_it, 'done') == 'done'
