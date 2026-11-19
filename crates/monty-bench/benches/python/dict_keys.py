mapping = {'a': 1, 'b': 2}
for _ in range(10_000):
    view = mapping.keys()
mapping['c'] = 3
assert list(view) == ['a', 'b', 'c']
