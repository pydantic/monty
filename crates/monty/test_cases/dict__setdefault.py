from collections import Counter, defaultdict

events = []


class Key:
    def __init__(self, number):
        self.number = number

    def __hash__(self):
        events.append(('hash', self.number))
        return 5

    def __eq__(self, other):
        events.append(('eq', self.number))
        return self is other


mapping = {}
first = Key(1)
second = Key(2)
default = ['default']
assert mapping.setdefault(first, default) is default
assert events == [('hash', 1)]
events.clear()
assert mapping.setdefault(second, default) is default
assert events == [('hash', 2), ('eq', 1)]
events.clear()
replacement = ['unused']
assert mapping.setdefault(first, replacement) is default
assert events == [('hash', 1)]
assert list(mapping) == [first, second]

mapping = {1: default}
assert mapping.setdefault(True, replacement) is default
assert next(iter(mapping)) is not True
assert {}.setdefault('missing') is None
special = defaultdict(list)
assert special.setdefault('missing') is None
counts = Counter(a=0)
assert counts.setdefault('a', 9) == 0
assert counts.setdefault('b', 2) == 2
recursive = {}
assert recursive.setdefault('self', recursive) is recursive
recursive.clear()


class ReentrantHash:
    def __hash__(self):
        mapping['side'] = 7
        return 13


key = ReentrantHash()
mapping = {}
assert mapping.setdefault(key, default) is default
assert mapping['side'] == 7
assert len(mapping) == 2


class ReentrantEquality:
    def __init__(self, mutate):
        self.mutate = mutate

    def __hash__(self):
        return 7

    def __eq__(self, other):
        if self.mutate:
            self.mutate = False
            mapping.clear()
            mapping['side'] = 7
        return False


stored = ReentrantEquality(True)
incoming = ReentrantEquality(False)
mapping = {stored: default}
assert mapping.setdefault(incoming, replacement) is replacement
assert mapping['side'] == 7
assert len(mapping) == 2
assert mapping[incoming] is replacement


class RaisingHash:
    def __hash__(self):
        raise ValueError('hash failed')


for _ in range(100):
    try:
        mapping.setdefault(RaisingHash(), ['temporary'])
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'hash failed'
    try:
        mapping.setdefault([], ['temporary'])
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "cannot use 'list' as a dict key (unhashable type: 'list')"
assert len(mapping) == 2
