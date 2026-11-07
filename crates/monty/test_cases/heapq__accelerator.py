import _heapq
import heapq

# `heapq` re-exports the accelerator's functions: they are the same objects.
names = [
    'heappush',
    'heappop',
    'heappushpop',
    'heapreplace',
    'heapify',
    'heappush_max',
    'heappop_max',
    'heappushpop_max',
    'heapreplace_max',
    'heapify_max',
]
for name in names:
    assert getattr(heapq, name) is getattr(_heapq, name), name

# The accelerator works on its own.
h = []
for x in [5, 1, 4, 2, 3]:
    _heapq.heappush(h, x)
assert h == [1, 2, 4, 5, 3]
assert _heapq.heappop(h) == 1
assert h == [2, 3, 4, 5]
assert _heapq.heappushpop(h, 0) == 0
assert _heapq.heapreplace(h, 9) == 2
assert h == [3, 5, 4, 9]

m = [3, 1, 2]
_heapq.heapify_max(m)
assert m == [3, 1, 2]
_heapq.heappush_max(m, 7)
assert m == [7, 3, 2, 1]
assert _heapq.heappop_max(m) == 7
assert m == [3, 1, 2]
assert _heapq.heappushpop_max(m, 10) == 10
assert _heapq.heapreplace_max(m, 0) == 3
assert m == [2, 1, 0]

# Errors come from the accelerator under its own name for keywords.
try:
    _heapq.heappush([], item=1)
except TypeError as e:
    assert str(e) == '_heapq.heappush() takes no keyword arguments', str(e)
else:
    assert False, 'did not raise'

try:
    _heapq.heappop(())
except TypeError as e:
    assert str(e) == 'heappop() argument must be list, not tuple', str(e)
else:
    assert False, 'did not raise'
