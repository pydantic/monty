import statistics
from statistics import StatisticsError


def check(exc_type, message, fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except exc_type as e:
        assert str(type(e)) == str(exc_type)
        assert str(e) == message
    else:
        assert False, 'did not raise'


# === empty and too-short data ===
check(StatisticsError, 'mean requires at least one data point', statistics.mean, [])
check(StatisticsError, 'mean requires at least one data point', statistics.mean, iter([]))
check(StatisticsError, 'fmean requires at least one data point', statistics.fmean, [])
check(StatisticsError, 'fmean requires at least one data point', statistics.fmean, iter([]))
check(StatisticsError, 'no median for empty data', statistics.median, [])
check(StatisticsError, 'no median for empty data', statistics.median_low, [])
check(StatisticsError, 'no median for empty data', statistics.median_high, [])
check(StatisticsError, 'no mode for empty data', statistics.mode, [])
check(StatisticsError, 'no mode for empty data', statistics.mode, '')
check(StatisticsError, 'variance requires at least two data points', statistics.variance, [1])
check(StatisticsError, 'variance requires at least two data points', statistics.variance, [])
check(StatisticsError, 'pvariance requires at least one data point', statistics.pvariance, [])
check(StatisticsError, 'stdev requires at least two data points', statistics.stdev, [1.5])
check(StatisticsError, 'pstdev requires at least one data point', statistics.pstdev, [])
check(StatisticsError, 'variance requires at least two data points', statistics.variance, [1], 1)
check(StatisticsError, 'must have at least one data point', statistics.quantiles, [])
check(StatisticsError, 'n must be at least 1', statistics.quantiles, [1, 2, 3], n=0)
check(StatisticsError, 'n must be at least 1', statistics.quantiles, [], n=-2)

# === fmean weights ===
check(StatisticsError, 'data and weights must be the same length', statistics.fmean, [1, 2], [1])
check(StatisticsError, 'data and weights must be the same length', statistics.fmean, [1], [1, 2])
check(StatisticsError, 'sum of weights must be non-zero', statistics.fmean, [1, 2], [0, 0])
check(StatisticsError, 'sum of weights must be non-zero', statistics.fmean, [1, 2], [1, -1])

# a list of weights is read live by `sumprod`, so one that grows while `data` is read no longer matches it
grown = [1, 1, 1]


class Grow:
    """Yields 1.0, 2.0, 3.0 and appends to `grown` partway through."""

    def __init__(self):
        self.i = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.i += 1
        if self.i == 2:
            grown.append(5)
        if self.i > 3:
            raise StopIteration
        return float(self.i)


check(StatisticsError, 'data and weights must be the same length', statistics.fmean, Grow(), grown)

# === quantiles arguments ===
check(ValueError, "Unknown method: 'linear'", statistics.quantiles, [1, 2, 3], method='linear')
check(ValueError, 'Unknown method: None', statistics.quantiles, [1, 2, 3], method=None)
check(ValueError, "Unknown method: 'Inclusive'", statistics.quantiles, [1, 2, 3], method='Inclusive')
check(TypeError, 'quantiles() takes 1 positional argument but 2 were given', statistics.quantiles, [1, 2], 4)

# === quantiles uses `n` as CPython does: compared with `<`, used as a repeat count, then as a `range` bound ===
check(StatisticsError, 'must have at least one data point', statistics.quantiles, [], n=2**70)
check(StatisticsError, 'n must be at least 1', statistics.quantiles, [], n=0.5)
check(TypeError, "'<' not supported between instances of 'str' and 'int'", statistics.quantiles, [1, 2], n='a')
check(TypeError, "'<' not supported between instances of 'NoneType' and 'int'", statistics.quantiles, [1, 2], n=None)
check(TypeError, "can't multiply sequence by non-int of type 'float'", statistics.quantiles, [1], n=3.0)
check(OverflowError, "cannot fit 'int' into an index-sized integer", statistics.quantiles, [1], n=2**70)
check(TypeError, "'float' object cannot be interpreted as an integer", statistics.quantiles, [1, 2], n=2.0)
check(ValueError, "Unknown method: 'bad'", statistics.quantiles, [1, 2], n=2.0, method='bad')

# === unsupported element types ===
check(TypeError, "can't convert type 'str' to numerator/denominator", statistics.mean, ['a', 'b'])
check(TypeError, "can't convert type 'NoneType' to numerator/denominator", statistics.mean, [1, None])
check(TypeError, "can't convert type 'str' to numerator/denominator", statistics.variance, ['a', 'b'])
check(TypeError, "can't convert type 'str' to numerator/denominator", statistics.stdev, ['a', 'b'])
check(TypeError, "can't convert type 'list' to numerator/denominator", statistics.pvariance, [[1], [2]])
check(TypeError, "unsupported operand type(s) for -: 'str' and 'int'", statistics.variance, ['a', 'b'], 1)
check(TypeError, "'int' object is not iterable", statistics.mean, 5)
check(TypeError, "'NoneType' object is not iterable", statistics.median, None)
check(TypeError, "'<' not supported between instances of 'str' and 'int'", statistics.median, [1, 'a'])
check(TypeError, "mean() missing 1 required positional argument: 'data'", statistics.mean)
check(TypeError, 'mean() takes 1 positional argument but 2 were given', statistics.mean, [1], [2])
check(TypeError, "mean() got an unexpected keyword argument 'x'", statistics.mean, [1], x=1)

# === StatisticsError is a ValueError that is also catchable by name ===
try:
    statistics.median([])
except ValueError as e:
    assert isinstance(e, StatisticsError)
    assert e.args == ('no median for empty data',)


# === argument errors name the function that was called ===
for name, fn in (
    ('mean', statistics.mean),
    ('median', statistics.median),
    ('median_low', statistics.median_low),
    ('median_high', statistics.median_high),
    ('mode', statistics.mode),
    ('multimode', statistics.multimode),
    ('stdev', statistics.stdev),
    ('pstdev', statistics.pstdev),
    ('variance', statistics.variance),
    ('pvariance', statistics.pvariance),
):
    check(TypeError, f"{name}() missing 1 required positional argument: 'data'", fn)


# === a user-defined subtraction can make the sum of squares negative ===
class Negative:
    def __sub__(self, other):
        return self

    def __mul__(self, other):
        return -1


try:
    statistics.stdev([Negative(), Negative()], xbar=0)
except ValueError as e:
    assert e.args == ('isqrt() argument must be nonnegative',)
else:
    assert False, 'stdev of a negative sum of squares did not raise'
assert statistics.variance([Negative(), Negative()], xbar=0) == -2
