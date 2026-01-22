# === iter() on various iterables ===
# iter() creates an iterator from an iterable

# iter() on list
it = iter([1, 2, 3])
assert next(it) == 1, 'iter list: first element should be 1'
assert next(it) == 2, 'iter list: second element should be 2'
assert next(it) == 3, 'iter list: third element should be 3'

# iter() on tuple
it = iter((10, 20))
assert next(it) == 10, 'iter tuple: first element should be 10'
assert next(it) == 20, 'iter tuple: second element should be 20'

# iter() on string
it = iter('ab')
assert next(it) == 'a', 'iter string: first element should be a'
assert next(it) == 'b', 'iter string: second element should be b'

# iter() on range
it = iter(range(3))
assert next(it) == 0, 'iter range: first element should be 0'
assert next(it) == 1, 'iter range: second element should be 1'
assert next(it) == 2, 'iter range: third element should be 2'

# iter() on dict iterates over keys
d = {'x': 1, 'y': 2}
it = iter(d)
keys = [next(it), next(it)]
assert 'x' in keys, 'iter dict: x should be in keys'
assert 'y' in keys, 'iter dict: y should be in keys'

# === next() with default value ===
# next() returns default when iterator is exhausted

it = iter([42])
assert next(it) == 42, 'next: first element should be 42'
assert next(it, 'done') == 'done', 'next with default: should return default when exhausted'

# Check default with various types
it = iter([])
assert next(it, None) is None, 'next with None default: should return None'
assert next(it, 0) == 0, 'next with 0 default: should return 0'
assert next(it, []) == [], 'next with empty list default: should return empty list'

# === iter() on iterator returns itself ===
# Calling iter() on an iterator should return the same iterator

original = iter([1, 2, 3])
same = iter(original)
# They should iterate over the same values
assert next(original) == 1, 'iter on iterator: original first should be 1'
assert next(same) == 2, 'iter on iterator: same should continue from 2'
assert next(original) == 3, 'iter on iterator: original should continue to 3'

# === Multiple independent iterators ===
# Creating multiple iterators over the same iterable should be independent

data = [1, 2, 3]
it1 = iter(data)
it2 = iter(data)
assert next(it1) == 1, 'independent iterators: it1 first should be 1'
assert next(it1) == 2, 'independent iterators: it1 second should be 2'
assert next(it2) == 1, 'independent iterators: it2 first should be 1 (independent)'
assert next(it1) == 3, 'independent iterators: it1 third should be 3'
assert next(it2) == 2, 'independent iterators: it2 second should be 2'
