from collections import Counter, defaultdict

events = []
forbidden = False


class Key:
    def __hash__(self):
        events.append('hash')
        if forbidden:
            raise RuntimeError('self update must not hash')
        return 7

    def __eq__(self, other):
        events.append('eq')
        return self is other


key = Key()
shared = []
source = {key: shared}
events.clear()
forbidden = True
alias = source
assert source.update(alias) is None
assert events == []
assert len(source) == 1
assert next(iter(source.values())) is shared
source.update(source, extra=42)
assert source['extra'] == 42
source.update(source, extra=43)
assert source['extra'] == 43
assert list(source)[0] is key
assert events == []
source |= source
assert source is alias
assert next(iter(source)) is key
assert next(iter(source.values())) is shared
assert events == []
source.clear()
assert source.update(source) is None

special = defaultdict(list, a=shared)
factory = special.default_factory
special.update(special, b=3)
assert special['a'] is shared
assert special['b'] == 3
assert special.default_factory is factory

# Counter self-updates add counts, and in-place union still prunes nonpositive counts.
counts = Counter(a=2)
counts.update(counts, a=1)
assert counts['a'] == 5
counts = Counter({'keep': 3, 'zero': 0, 'negative': -2})
counts |= counts
assert list(counts) == ['keep']
assert counts['keep'] == 3
