from collections import defaultdict


class Factory:
    def __call__(self):
        return []


factory = Factory()
shared = []
key = ('a heap-backed dictionary key',)
source = {key: shared}
special = defaultdict(factory, source)
for _ in range(100):
    copied = source.copy()
    special_copy = special.copy()
assert copied[key] is shared
assert special_copy[key] is shared
assert special_copy.default_factory is factory
# ref-counts={'Factory': 2, 'factory': 3, 'shared': 5, 'key': 5, 'source': 1, 'special': 1, 'copied': 1, 'special_copy': 1}
