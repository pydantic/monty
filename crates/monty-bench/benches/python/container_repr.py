# Container repr over 1,000-element containers, half of them tracked strings. Guards the
# mutation-safe repr paths: checked iteration for list/tuple/dict, snapshots for
# deque/set/Counter, and the amortized time polls.

from collections import Counter, deque

words = [str(i) for i in range(500)]
xs = words + list(range(500))
tup = tuple(xs)
d = {w: i for i, w in enumerate(words)}
st = set(words)
dq = deque(xs)
c = Counter({w: i % 7 for i, w in enumerate(words)})

n = 0
for _ in range(20):
    n += len(repr(xs)) + len(repr(tup)) + len(repr(d))
    n += len(repr(st)) + len(repr(dq)) + len(repr(c))
assert n == 628_320
