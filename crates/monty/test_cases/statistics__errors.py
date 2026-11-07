import statistics
from statistics import StatisticsError


def check(exc_type, message, fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except exc_type as e:
        assert isinstance(e, exc_type), (type(e), e)
        assert str(e) == message, (str(e), message)
    else:
        assert False, f'{fn.__name__} did not raise'


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

# === quantiles arguments ===
check(ValueError, "Unknown method: 'linear'", statistics.quantiles, [1, 2, 3], method='linear')
check(ValueError, 'Unknown method: None', statistics.quantiles, [1, 2, 3], method=None)
check(ValueError, "Unknown method: 'Inclusive'", statistics.quantiles, [1, 2, 3], method='Inclusive')
check(TypeError, 'quantiles() takes 1 positional argument but 2 were given', statistics.quantiles, [1, 2], 4)

# === unsupported element types ===
check(TypeError, "can't convert type 'str' to numerator/denominator", statistics.mean, ['a', 'b'])
check(TypeError, "can't convert type 'NoneType' to numerator/denominator", statistics.mean, [1, None])
check(TypeError, "can't convert type 'str' to numerator/denominator", statistics.variance, ['a', 'b'])
check(TypeError, "can't convert type 'str' to numerator/denominator", statistics.stdev, ['a', 'b'])
check(TypeError, "can't convert type 'list' to numerator/denominator", statistics.pvariance, [[1], [2]])
check(TypeError, "unsupported operand type(s) for -: 'str' and 'int'", statistics.variance, ['a', 'b'], 1)
check(TypeError, "'int' object is not iterable", statistics.mean, 5)
check(TypeError, "'NoneType' object is not iterable", statistics.median, None)
for fn in (statistics.mode, statistics.multimode):
    try:
        fn([[1], [2]])
        assert False, 'expected TypeError'
    except TypeError as e:
        # Monty words this as a dict-key failure; CPython says only "unhashable type"
        assert "unhashable type: 'list'" in str(e)
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
