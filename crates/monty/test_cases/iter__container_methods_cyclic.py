# Verifies that list and dict methods iterating internally
# (list.count, list.index, list.remove, dict.update) correctly handle
# cyclic / self-referential elements. The Rust implementation runs each
# of these inside a `try_for_each` that holds a recursion-depth token
# for the duration of iteration; this exercise ensures the token is
# released on every exit path (success, break, error) and that re-entry
# via cyclic `py_eq` is bounded by the recursion limit rather than
# panicking.

# === count / index / remove on a self-cycle ===
# `a == a` short-circuits via identity, so methods that compare items
# against `a` itself never actually recurse — they should just see one
# match per cyclic occurrence.
a = []
a.append(a)
assert a.count(a) == 1, 'count of self in a single-element self-cycle'
assert a.index(a) == 0, 'index of self in self-cycle'

# === cyclic element interleaved with other items ===
c = [1, 2]
c.append(c)
c.append(3)
assert len(c) == 4, 'sanity: cyclic list length unchanged by append-of-self'
assert c.count(1) == 1, 'count of primitive preceding cycle'
assert c.count(c) == 1, 'count of cycle entry'
assert c.count(3) == 1, 'count of primitive following cycle'
assert c.index(3) == 3, 'index past the cyclic slot'
assert c.index(c) == 2, 'index of the cyclic slot itself'

# remove of the cyclic element breaks the cycle and leaves the rest intact.
c.remove(c)
assert len(c) == 3, 'remove drops exactly one element'
assert c[0] == 1, 'first element preserved'
assert c[1] == 2, 'second element preserved'
assert c[2] == 3, 'tail element preserved'

# remove of a non-cyclic element from a list that still contains a cycle —
# verifies the iteration still reaches items past the cyclic slot.
d = [1, 2]
d.append(d)
d.append(3)
d.remove(3)
assert len(d) == 3, 'remove found and dropped the tail element'
assert d[0] == 1
assert d[1] == 2
assert d[2] is d, 'cyclic slot intact after removing an unrelated element'

# === two distinct cycles ===
# Comparing two separate cyclic structures must descend until the
# recursion limit is hit; the methods that iterate must surface that
# RecursionError cleanly rather than panicking or leaking the
# in-flight recursion token.
x = []
x.append(x)
y = []
y.append(y)
try:
    x.count(y)
    assert False, 'expected RecursionError for count across distinct cycles'
except RecursionError:
    pass

try:
    x.index(y)
    assert False, 'expected RecursionError for index across distinct cycles'
except RecursionError:
    pass

try:
    x.remove(y)
    assert False, 'expected RecursionError for remove across distinct cycles'
except RecursionError:
    pass
# x still has its self-reference; the failed remove did not mutate it.
assert len(x) == 1
assert x[0] is x

# === dict.update with a cyclic source ===
# Update from a dict whose values include a self-reference. The source
# iteration runs under a recursion token; on success the token must be
# released cleanly so the subsequent operation works.
src = {'k': 1}
src['self'] = src
dst = {}
dst.update(src)
assert dst['k'] == 1
assert dst['self'] is src, 'dict.update copies the value reference, not a deep clone'

# Self-update is a no-op for size; this exercises the case where the
# `self.set` call inside the iteration mutates (replaces) an entry while
# the source iteration is in progress.
g = {'a': 1, 'b': 2}
g.update(g)
assert g == {'a': 1, 'b': 2}, 'self-update leaves dict contents unchanged'
