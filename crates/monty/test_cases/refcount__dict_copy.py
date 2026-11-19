from collections import Counter, defaultdict


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
    normalized = dict(special)
    counted = Counter(source)
assert copied[key] is shared
assert special_copy[key] is shared
assert special_copy.default_factory is factory
assert normalized[key] is shared
assert counted[key] is shared


class BadKey:
    def __hash__(self):
        return hash('x')

    def __eq__(self, other):
        raise ValueError('keyword collision')


# A failure while applying keywords releases the copied entries and remaining arguments.
bad = BadKey()
for constructor in [dict, Counter]:
    try:
        constructor({bad: shared}, x=shared)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'keyword collision'
# ref-counts={'Factory': 2, 'factory': 3, 'shared': 7, 'key': 7, 'source': 1, 'special': 1, 'copied': 1, 'special_copy': 1, 'normalized': 1, 'counted': 1, 'BadKey': 2, 'bad': 1}
