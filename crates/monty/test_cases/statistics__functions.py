import statistics
from collections import Counter
from statistics import (
    StatisticsError,
    fmean,
    median,
    median_high,
    median_low,
    mode,
    multimode,
    quantiles,
)

# === median family ===
assert median([1, 3, 5]) == 3
assert median([1, 3, 5, 7]) == 4.0
assert median([3, 1, 2]) == 2
assert median([1, 2]) == 1.5
assert median([5]) == 5
assert type(median([1, 2, 3])) is int
assert type(median([1, 3])) is float
assert median([1.5, 2.5, 4.0]) == 2.5
assert median(iter([9, 1, 5, 3])) == 4.0
assert median(x for x in [3, 1, 2]) == 2
assert median([0.1, 0.2]) == 0.15000000000000002
assert median([10**30, 10**30 + 2]) == 1e30
assert median_low([1, 2, 3, 4]) == 2
assert median_high([1, 2, 3, 4]) == 3
assert median_low([1, 2, 3]) == 2
assert median_high([1, 2, 3]) == 2
assert median_low(['b', 'a', 'c', 'd']) == 'b'
assert median_high(['b', 'a', 'c', 'd']) == 'c'
assert median_low([5]) == 5 and median_high([5]) == 5
assert median_low((3, 1, 2, 4)) == 2

# === mode and multimode ===
assert mode([1, 1, 2, 3]) == 1
assert mode(['a', 'b', 'b', 'a', 'c']) == 'a'
assert mode([2, 2, 1, 1]) == 2
assert mode('aabbb') == 'b'
assert mode(iter([3, 3, 4])) == 3
assert mode([1.0, 1, True, 2]) == 1.0
assert multimode([1, 1, 2, 2, 3]) == [1, 2]
assert multimode('aabbbcc') == ['b']
assert multimode([]) == []
assert multimode('') == []
assert multimode(iter([2, 1, 1, 2])) == [2, 1]
assert multimode([(1, 2), (1, 2), (3,)]) == [(1, 2)]
assert multimode([None, None, 0]) == [None]
# a mapping is iterated for its keys, as `Counter(iter(data))` sees it, not read as counts
assert mode({'a': 1, 'b': 5}) == 'a'
assert multimode({'a': 1, 'b': 5}) == ['a', 'b']
assert mode(Counter({'x': 1, 'y': 3})) == 'x'
assert multimode(Counter({'x': 1, 'y': 3})) == ['x', 'y']

# === fmean ===
assert fmean([1, 2, 3, 4]) == 2.5
assert fmean([0.1] * 10) == 0.1
assert fmean([0.1, 0.2, 0.3]) == 0.19999999999999998
assert type(fmean([1, 2, 3])) is float
assert fmean(iter([1, 2, 3, 4])) == 2.5
assert fmean(x for x in [2, 4]) == 3.0
assert fmean([3.5, 4.5, 6.0], [3, 2, 1]) == (3.5 * 3 + 4.5 * 2 + 6.0) / 6
assert fmean([1, 2], weights=[1, 1]) == 1.5
assert fmean([1, 2, 3], weights=iter([0, 0, 1])) == 3.0
assert fmean([10, 20], weights=(1, 3)) == 17.5
assert fmean([float('inf'), 1]) == float('inf')


# weights are read before data, so a one-shot iterator feeding both is split in that order
class Draw:
    """A lazy iterator that pulls `count` values from `source` only when it is iterated."""

    def __init__(self, source, count):
        self.source = source
        self.count = count

    def __iter__(self):
        return self

    def __next__(self):
        if self.count == 0:
            raise StopIteration
        self.count -= 1
        return next(self.source)


shared = iter([1.0, 2.0, 3.0, 4.0])
assert fmean(shared, Draw(shared, 2)) == (3.0 * 1.0 + 4.0 * 2.0) / 3.0


# === quantiles ===
data = [105, 129, 87, 86, 111, 111, 89, 81, 108, 92, 110, 100, 75, 105, 103, 109, 76, 119, 99, 91, 103, 129]
assert len(quantiles(data)) == 3
assert quantiles(data) == quantiles(sorted(data))
assert quantiles([1, 2, 3, 4, 5]) == [1.5, 3.0, 4.5]
assert quantiles([1, 2, 3, 4, 5], method='inclusive') == [2.0, 3.0, 4.0]
assert quantiles([1, 2, 3, 4, 5], n=2) == [3.0]
assert quantiles([1, 2, 3, 4, 5], n=10) == quantiles([5, 4, 3, 2, 1], n=10)
assert quantiles([1, 2, 3, 4, 5], n=1) == []
assert quantiles([1, 2], n=True) == []
# a single datum is `data * (n - 1)`, returned before `method` is looked at
assert quantiles([7], n=3) == [7, 7]
assert quantiles([7], n=3, method='bad') == [7, 7]
assert quantiles([1, 2, 3, 4, 5], n=100, method='inclusive')[49] == 3.0
assert len(quantiles([1, 2, 3, 4, 5], n=100)) == 99
assert quantiles([7], n=4) == [7, 7, 7]
assert quantiles([7], n=1) == []
assert quantiles([2.5], n=3, method='inclusive') == [2.5, 2.5]
assert quantiles(iter([4, 1, 3, 2]), n=4, method='inclusive') == [1.75, 2.5, 3.25]
assert quantiles((1, 2), n=4) == [0.75, 1.5, 2.25]
assert quantiles([0.1, 0.2, 0.3, 0.4], n=3, method='inclusive') == [0.20000000000000004, 0.3]
assert quantiles([1, 2, 3, 4, 5], n=True) == []

# === StatisticsError ===
assert statistics.StatisticsError is StatisticsError
try:
    statistics.mean([])
    assert False, 'expected StatisticsError'
except ValueError as e:
    assert type(e).__name__ == 'StatisticsError'
    assert str(e) == 'mean requires at least one data point'
    assert e.args == ('mean requires at least one data point',)
