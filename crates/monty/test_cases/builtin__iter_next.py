# === iter() on various iterables ===
# iter() creates an iterator from an iterable

# iter() on list
it = iter([1, 2, 3])
assert next(it) == 1
assert next(it) == 2
assert next(it) == 3

# iter() on tuple
it = iter((10, 20))
assert next(it) == 10
assert next(it) == 20

# iter() on string
it = iter('ab')
assert next(it) == 'a'
assert next(it) == 'b'

# iter() on range
it = iter(range(3))
assert next(it) == 0
assert next(it) == 1
assert next(it) == 2

# iter() on dict iterates over keys
d = {'x': 1, 'y': 2}
it = iter(d)
keys = [next(it), next(it)]
assert 'x' in keys
assert 'y' in keys

# === next() with default value ===
# next() returns default when iterator is exhausted

it = iter([42])
assert next(it) == 42
assert next(it, 'done') == 'done'

# Check default with various types
it = iter([])
assert next(it, None) is None
assert next(it, 0) == 0
assert next(it, []) == []

# === iter() on iterator returns itself ===
# Calling iter() on an iterator should return the same iterator

original = iter([1, 2, 3])
same = iter(original)
# They should iterate over the same values
assert next(original) == 1
assert next(same) == 2
assert next(original) == 3

# === Multiple independent iterators ===
# Creating multiple iterators over the same iterable should be independent

data = [1, 2, 3]
it1 = iter(data)
it2 = iter(data)
assert next(it1) == 1
assert next(it1) == 2
assert next(it2) == 1
assert next(it1) == 3
assert next(it2) == 2
