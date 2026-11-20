import statistics
from statistics import mean, pstdev, pvariance, stdev, variance

# === mean is correctly rounded, not a naive float sum ===
assert mean([0.1] * 10) == 0.1
assert mean([0.1, 0.2, 0.3]) == 0.2
assert mean([1e100, 1.0, -1e100]) == 0.3333333333333333
assert mean([1e308, 1e308]) == 1e308
assert mean([5e-324, 5e-324]) == 5e-324
assert mean([-0.0]) == 0.0
assert mean([1, 2, 3, 4]) == 2.5
assert mean([1.5, 2.5, 2.5, 2.75, 3.25, 4.75]) == 2.875

# === mean result types ===
assert type(mean([1, 2, 3])) is int and mean([1, 2, 3]) == 2
assert type(mean([1, 2])) is float and mean([1, 2]) == 1.5
assert type(mean([1, 2.0, 3])) is float
assert type(mean([True, True])) is int and mean([True, True]) == 1
assert type(mean([2.0])) is float
assert type(mean([7])) is int

# === large ints are exact ===
big = 10**40
assert mean([big, big + 2]) == big + 1
assert type(mean([big, big + 2])) is int
assert mean([big, big + 1]) == 1e40
assert type(mean([big, big + 1])) is float
assert mean([2**100, 1]) == 6.338253001141147e29
assert mean([10**400, 10**400]) == 10**400

# === iterators and mixed containers ===
assert mean(iter([1, 2, 3, 4])) == 2.5
assert mean(x * 0.5 for x in range(1, 5)) == 1.25
assert mean((1, 2, 3)) == 2
assert mean({1, 2, 3}) == 2
assert mean(range(10)) == 4.5
assert mean({1: 'a', 3: 'b'}) == 2

# === non-finite data ===
inf = float('inf')
nan = float('nan')
assert mean([1.0, inf]) == inf
assert mean([1.0, -inf]) == -inf
assert mean([inf, -inf]) != mean([inf, -inf])
assert mean([1.0, nan]) != mean([1.0, nan])
assert mean([inf, inf]) == inf

# === variance family ===
data = [1.5, 2.5, 2.5, 2.75, 3.25, 4.75]
assert stdev(data) == 1.0810874155219827
assert pstdev(data) == 0.986893273527251
assert variance(data) == 1.16875
assert pvariance(data) == 0.9739583333333334
assert variance(data) == statistics.variance(data, statistics.mean(data))
assert pvariance(data) == pvariance(data, mean(data))
assert stdev(data) ** 2 == stdev(data) * stdev(data)
assert variance([1, 2, 3, 4, 5]) == 2.5
assert type(variance([1, 3])) is int and variance([1, 3]) == 2
assert type(variance([1.0, 3.0])) is float
assert pvariance([1, 2, 3, 4]) == 1.25
assert type(pvariance([2, 2])) is int and pvariance([2, 2]) == 0
assert pvariance([5]) == 0
assert pstdev([5]) == 0.0 and type(pstdev([5])) is float
assert pstdev([1, 1, 1]) == 0.0
assert stdev([1, 2, 3, 4, 5, 6, 7, 8, 9]) == 2.7386127875258306
assert pstdev([0.1] * 10) == 0.0
assert variance([0.1] * 10) == 0.0
assert pvariance([1e8 + 0.1, 1e8 + 0.2, 1e8 + 0.3]) == 0.00666666686534883
assert pvariance([0.1, 0.2, 0.3]) == 0.006666666666666665

# === exact variance on awkward data ===
assert pvariance([1e9 + 1, 1e9 + 2, 1e9 + 3, 1e9 + 4]) == 1.25
assert variance([big, big + 1, big + 2]) == 1
assert type(variance([big, big + 1, big + 2])) is int
assert pstdev([big, big + 2]) == 1.0
assert stdev([10**200, 10**200 + 10**100]) == 7.071067811865475e99

# === xbar and mu ===
assert variance([1, 2, 3, 4], 2.5) == variance([1, 2, 3, 4])
assert variance([1, 2, 3, 4], xbar=2) == 2
assert type(variance([1, 2, 3, 4], xbar=2)) is int
assert pvariance([1, 2, 3, 4], mu=2) == 1.5
assert type(pvariance([1, 2, 3, 4], mu=2)) is float
assert pvariance([1, 3], 2) == 1
assert type(pvariance([1, 3], 2)) is int
assert pstdev([1, 2, 3, 4], mu=2.5) == pstdev([1, 2, 3, 4])
assert stdev([1, 2, 3, 4], xbar=0) == 3.1622776601683795
assert variance(iter([1, 2, 3, 4]), None) == variance([1, 2, 3, 4])

# === non-finite variance ===
assert variance([1.0, inf, 2.0]) == inf
assert pvariance([nan, 1.0]) != pvariance([nan, 1.0])
try:
    stdev([1.0, inf])
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'inf or nan encountered in data'
    assert type(e).__name__ == 'ValueError'
try:
    pstdev([nan, 1.0])
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'inf or nan encountered in data'
