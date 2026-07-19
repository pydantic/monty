# An iterator is itself iterable (its __iter__ returns self), so for-loops,
# list()/tuple()/sum() and comprehensions all drive an existing iterator,
# sharing its position rather than restarting it.

# === for-loop over an iterator ===
out = []
for x in iter([1, 2, 3]):
    out.append(x)
assert out == [1, 2, 3]

# === constructors consume an iterator ===
assert list(iter([4, 5, 6])) == [4, 5, 6]
assert tuple(iter([7, 8])) == (7, 8)
assert sum(iter([1, 2, 3])) == 6

# === iter(it) is it, and consumption shares the underlying state ===
it = iter([10, 20, 30])
assert iter(it) is it
assert next(it) == 10
assert list(it) == [20, 30]

# Repeated iter() returns the same object rather than nesting, so a delegation
# chain deeper than 1 is not reachable from Python at all.
deep = iter([1, 2, 3])
for _ in range(200):
    deep = iter(iter(deep))
assert next(deep) == 1
assert list(deep) == [2, 3]

# === comprehension over an iterator ===
assert [x * 2 for x in iter([1, 2, 3])] == [2, 4, 6]
