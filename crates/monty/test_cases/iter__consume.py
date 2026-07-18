# Phase 1a: an existing iterator is itself iterable (its __iter__ returns self),
# so for-loops, list()/tuple()/sum(), and comprehensions all drive it.
# Today only next(it) works on a bare iterator; these consumption sites fail
# with "'iterator' object is not iterable".

# === for-loop over an iterator ===
out = []
for x in iter([1, 2, 3]):
    out.append(x)
assert out == [1, 2, 3], 'for-loop drives an existing iterator'

# === constructors consume an iterator ===
assert list(iter([4, 5, 6])) == [4, 5, 6], 'list() consumes an iterator'
assert tuple(iter([7, 8])) == (7, 8), 'tuple() consumes an iterator'
assert sum(iter([1, 2, 3])) == 6, 'sum() consumes an iterator'

# === iter(it) is it, and consumption shares the underlying state ===
it = iter([10, 20, 30])
assert iter(it) is it, 'iter() of an iterator returns the same object'
assert next(it) == 10, 'next() advances the iterator'
assert list(it) == [20, 30], 'list() continues where next() left off (shared state)'

# === comprehension over an iterator ===
assert [x * 2 for x in iter([1, 2, 3])] == [2, 4, 6], 'comprehension consumes an iterator'
