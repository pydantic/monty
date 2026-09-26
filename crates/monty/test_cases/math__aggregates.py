import math

# === Norms: dimensions, scaling, signs, and special values ===
assert math.hypot() == 0.0
assert type(math.hypot()) is float
assert math.hypot(-7) == 7.0
assert math.hypot(3, -4) == 5.0
assert math.hypot(2, 3, 6) == 7.0
assert math.hypot(*([1.0] * 100)) == 10.0
assert math.hypot(True, False) == 1.0
assert math.copysign(1, math.hypot(-0.0)) == 1
assert math.isclose(math.hypot(3e200, 4e200), 5e200, rel_tol=1e-15)
assert math.isclose(math.hypot(3e-200, 4e-200), 5e-200, rel_tol=1e-15)
assert math.hypot(3e-323, 4e-323) == 5e-323
assert math.hypot(5e-324, 5e-324) == 5e-324
assert math.hypot(1e308, 1e308) == 1.4142135623730951e308
assert math.hypot(1.7e308, 1.7e308) == math.inf
assert math.hypot(math.nan, -math.inf) == math.inf
assert math.hypot(math.inf, math.nan) == math.inf
assert math.isnan(math.hypot(math.nan))
assert math.isnan(math.hypot(0, math.nan))
assert math.hypot(2**100) == 2.0**100
assert math.hypot(1267650600228229401496703205376) == 2.0**100

# These inputs exposed rounding errors in earlier CPython norm algorithms.
for x, y, expected in [
    (572330659.1077356, 1180450261.5893514, 1311878501.7832494),
    (712049370.5674208, 791476409.2831447, 1064635718.251647),
    (1037023154.6745788, 794204895.5538545, 1306207655.5635877),
]:
    assert math.hypot(x, y) == expected
    assert math.dist([x, y], [0, 0]) == expected

assert math.dist([], []) == 0.0
assert type(math.dist([], [])) is float
assert math.dist((1, 2), (4, 6)) == 5.0
assert math.dist(iter([2, 3, 6]), (0 for _ in range(3))) == 7.0
assert math.dist([3e-323, 4e-323], [0, 0]) == 5e-323
assert math.dist([1e308], [-1e308]) == math.inf
assert math.isnan(math.dist([math.inf], [math.inf]))
assert math.dist([math.inf, math.nan], [0, 0]) == math.inf
assert math.dist([2**100], [0]) == 2.0**100

for p, q in [([1], []), ([], [1]), (['bad'], []), (['bad'], [1, 2])]:
    try:
        math.dist(p, q)
        assert False, 'unequal dimensions must fail before coordinate conversion'
    except ValueError as e:
        assert str(e) == 'both points must have the same number of dimensions'

# === Accurate sums: cancellation, rounding ties, and partials spanning exponents ===
assert math.fsum([]) == 0.0
assert type(math.fsum([])) is float
assert math.copysign(1, math.fsum([-0.0])) == 1
assert math.fsum([True, False, 2]) == 3.0
assert math.fsum(x for x in [1e100, 1, -1e100]) == 1.0
assert math.fsum([1e100, 1, -1e100, 1e-100, 1e50, -1, -1e50]) == 1e-100
assert math.fsum([2.0**53, -0.5, -(2.0**-54)]) == 2.0**53 - 1.0
assert math.fsum([2.0**53, 1.0, 2.0**-100]) == 2.0**53 + 2.0
assert math.fsum([1e16, 1.0, 1e-16]) == 10000000000000002.0
assert math.fsum([-1e16, -1.0, -1e-16]) == -10000000000000002.0
assert math.fsum([5e-324] * 10) == 5e-323
assert math.fsum([2**100, 1, -(2**100)]) == 1.0
assert math.fsum([math.inf, 1]) == math.inf
assert math.fsum([-math.inf, 1]) == -math.inf
assert math.isnan(math.fsum([math.nan, 1]))
assert math.isnan(math.fsum([math.nan, math.inf]))
partials = [2.0**n - 2.0 ** (n + 50) + 2.0 ** (n + 52) for n in range(-1074, 972, 2)]
assert math.fsum(partials + [-(2.0**1022)]) == 1.3305602063564798e292

for values in [[math.inf, -math.inf], [math.nan, math.inf, -math.inf]]:
    try:
        math.fsum(values)
        assert False, 'opposite infinities must fail'
    except ValueError as e:
        assert str(e) == '-inf + inf in fsum'

for values in [[1e308, 1e308], [math.nan, 1e308, 1e308]]:
    try:
        math.fsum(values)
        assert False, 'intermediate overflow must fail'
    except OverflowError as e:
        assert str(e) == 'intermediate overflow in fsum'

# === Products: integer preservation, generators, and the start value ===
assert math.prod([]) == 1
assert type(math.prod([])) is int
assert math.prod(range(1, 6)) == 120
assert math.prod(x for x in [2, 3, 4]) == 24
assert math.prod([2, 3], start=5) == 30
assert math.prod([2, 3], start=0.5) == 3.0
assert type(math.prod([2, 3], start=0.5)) is float
assert math.prod([2**50, 2**60, -3]) == -3 * 2**110
assert math.prod([True, True]) == 1
assert math.prod([True, False]) == 0
assert math.prod([1e308, 2.0]) == math.inf
assert math.isnan(math.prod([0.0, math.inf]))
assert math.copysign(1, math.prod([-0.0, 2])) == -1
start = ['retained']
assert math.prod([], start=start) is start
assert math.prod([2, 3], start='x') == 'xxxxxx'
assert math.prod([2], start=start) == ['retained', 'retained']

# === Dot products: exact integers and compensated float multiplication ===
assert math.sumprod([], []) == 0
assert type(math.sumprod([], [])) is int
assert math.sumprod(iter([10, 20, 30]), (1, 2, 3)) == 140
assert math.sumprod((x for x in [1.5, 2.5]), [3.5, 4.5]) == 16.5
assert math.sumprod([True, False], [True, True]) == 1
assert math.sumprod([1.5, 2.5], [True, False]) == 1.5
assert math.sumprod([True, False], [1.5, 2.5]) == 1.5
assert math.sumprod([2**100, 1], [2**100, 2]) == 2**200 + 2
assert math.sumprod([2**62, 2**62], [1, 1]) == 2**63
assert math.sumprod([2**40], [2**40]) == 2**80
assert math.sumprod([1e100, 1.0, -1e100], [1.0, 1.0, 1.0]) == 1.0
assert math.sumprod([1e100, 1.0, -1e100], [1, 1, 1]) == 1.0
assert math.sumprod([1.0], [2**100]) == 2.0**100
assert math.sumprod([2**100], [1.0]) == 2.0**100
a = 2.0**-50
assert math.sumprod([a - 1.0, 1.0], [a + 1.0, 1.0]) == a * a
assert math.sumprod([-5, -5, 10], [1.5, 4611686018427387904, 2305843009213693952]) == 0.0
assert math.sumprod([1.0, math.inf], [2.0, 3.0]) == math.inf
assert math.isnan(math.sumprod([1e308, -1e308], [2.0, 2.0]))
assert math.isnan(math.sumprod([0.0], [math.inf]))
assert math.isnan(math.sumprod([math.nan], [1]))

for p, q in [([1], []), ([], [1]), ([1, 2], [3]), ([1], [2, 3])]:
    try:
        math.sumprod(p, q)
        assert False, 'unequal lengths must fail'
    except ValueError as e:
        assert str(e) == 'Inputs are not the same length'

# === Fused multiply-add: a single rounding, overflow, and invalid operations ===
assert math.fma(2, 3, 4) == 10.0
assert type(math.fma(2, 3, 4)) is float
assert math.fma(True, 2, False) == 2.0
assert math.fma(a - 1.0, a + 1.0, 1.0) == a * a
assert math.fma(1e308, 2.0, -1e308) == 1e308
assert math.fma(5e-324, 1, 5e-324) == 1e-323
assert math.fma(2**100, 1, 0) == 2.0**100
assert math.copysign(1, math.fma(-0.0, 1.0, -0.0)) == -1
assert math.fma(math.inf, 1, 0) == math.inf
assert math.isnan(math.fma(0, math.inf, math.nan))
assert math.isnan(math.fma(math.nan, 1, 2))
for x, y, z in [(0, math.inf, 1), (math.inf, 0, 1), (math.inf, 1, -math.inf)]:
    try:
        math.fma(x, y, z)
        assert False, 'invalid fused operation must fail'
    except ValueError as e:
        assert str(e) == 'invalid operation in fma'
try:
    math.fma(1e308, 2, 0)
    assert False, 'fused overflow must fail'
except OverflowError as e:
    assert str(e) == 'overflow in fma'

# === Conversion and iteration errors release retained values ===
for function, args in [
    (math.hypot, (math.inf, 'bad')),
    (math.dist, (['bad'], [0])),
    (math.fsum, ([1, 'bad'],)),
    (math.fma, (1, 'bad', 3)),
]:
    try:
        function(*args)
        assert False, 'non-numeric arguments must fail'
    except TypeError as e:
        assert str(e) == 'must be real number, not str'

for function, args in [
    (math.hypot, (2**2000,)),
    (math.dist, ([2**2000], [0])),
    (math.fsum, ([2**2000],)),
    (math.fma, (1, 2**2000, 0)),
    (math.sumprod, ([1.0], [2**2000])),
    (math.sumprod, ([2**2000], [1.0])),
    (math.sumprod, ([2**2000, 1.0], [1, 1.0])),
    (math.prod, ([2**2000, 1.0],)),
    (math.prod, ([1.0, 2**2000],)),
]:
    try:
        function(*args)
        assert False, 'float conversion must reject overflowing integers'
    except OverflowError as e:
        assert str(e) == 'int too large to convert to float'


def broken(values):
    """Yield retained values before failing inside a native aggregation loop."""
    values = iter(values)

    def step():
        """Raise after the last successful item."""
        value = next(values, None)
        if value is None:
            raise RuntimeError('iterator failed')
        return value

    return iter(step, None)


for function, args in [
    (math.prod, (broken([2**100]),)),
    (math.fsum, (broken([1.0]),)),
    (math.dist, (broken([1.0]), [0])),
    (math.dist, ([0], broken([1.0]))),
    (math.sumprod, (broken([1.0]), [2])),
    (math.sumprod, ([1], broken([2**100]))),
]:
    try:
        function(*args)
        assert False, 'iterator errors must propagate'
    except RuntimeError as e:
        assert str(e) == 'iterator failed'

for function, args, message in [
    (math.prod, ([2**100, {}],), "unsupported operand type(s) for *: 'int' and 'dict'"),
    (math.sumprod, ([2**100, {}], [1, 2]), "unsupported operand type(s) for *: 'dict' and 'int'"),
    (math.sumprod, (['abc'], [2]), "unsupported operand type(s) for +: 'int' and 'str'"),
    (math.prod, (None,), "'NoneType' object is not iterable"),
    (math.dist, ([2**100], None), "'NoneType' object is not iterable"),
    (math.sumprod, ([2**100], None), "'NoneType' object is not iterable"),
]:
    try:
        function(*args)
        assert False, 'invalid operands must fail'
    except TypeError as e:
        assert str(e) == message
