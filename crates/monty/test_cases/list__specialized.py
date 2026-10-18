# Integer and float buffers must preserve Python types through every mutation.
for initial in ([1, 2, 3], [1.0, 2.0, 3.0]):
    for incompatible in (None, True, 4.5, 2**80, 'text', [], {}):
        for operation in ('append', 'insert', 'set', 'extend', 'iadd'):
            xs = initial.copy()
            alias = xs
            it = iter(xs)
            assert next(it) == initial[0]
            if operation == 'append':
                xs.append(incompatible)
                expected = initial + [incompatible]
            elif operation == 'insert':
                xs.insert(1, incompatible)
                expected = [initial[0], incompatible] + initial[1:]
            elif operation == 'set':
                xs[1] = incompatible
                expected = [initial[0], incompatible, initial[2]]
            elif operation == 'extend':
                xs.extend([incompatible])
                expected = initial + [incompatible]
            else:
                xs += [incompatible]
                expected = initial + [incompatible]
            assert xs is alias
            assert xs == expected
            assert list(it) == expected[1:]
            assert [type(v) for v in xs] == [type(v) for v in expected]

for xs in ([], [1, 2, 3], [1.0, -0.0, 3.0], [1, None, 3]):
    for step in (1, -1, 2, -2, 2**63 - 1, -(2**63)):
        assert xs[::step] == list(tuple(xs)[::step])
    assert xs * 0 == []
    assert xs * 3 == list(tuple(xs) * 3)
    copied = xs.copy()
    copied.reverse()
    assert copied == xs[::-1]
    copied.clear()
    copied.append(1.5)
    assert type(copied[0]) is float

for initial in ([3, 1, 2], [3.0, 1.0, 2.0]):
    xs = initial.copy()
    xs.extend(xs)
    assert xs == initial * 2
    xs += xs
    assert xs == initial * 4
    xs.sort()
    assert xs == sorted(initial * 4)
    assert xs.pop() == initial[0]
    xs.remove(initial[1])
    assert len(xs) == 10

xs = [1, 2]
xs.append(xs)
assert xs[-1] is xs
assert repr(xs) == '[1, 2, [...]]'
assert xs.copy()[-1] is xs

xs = [1.0, -0.0, float('inf'), float('-inf'), float('nan')]
assert str(xs[1]) == '-0.0'
assert xs[-1] != xs[-1]
assert str(xs[:][-1]) == 'nan'

# A wide accumulator may overflow i64 and later return to its range.
limit = 2**63 - 1
assert sum([limit, limit, -limit, -limit]) == 0
assert sum([-limit - 1, -limit - 1]) == -(2**64)
assert sum([limit, limit], limit) == 3 * limit
assert sum([1, 2], True) == 4
assert type(sum([1, 2], True)) is int
xs = [1]
xs.pop()
assert sum(xs, True) is True
assert sum([1, 2], 0.5) == 3.5
assert sum([1, 2], 2**100) == 2**100 + 3
try:
    sum([1, 2], None)
    assert False
except TypeError:
    pass

# A key callback sees the detached list and may change its representation.
xs = [3, 1, 2]


def key(v):
    xs.append('changed')
    return v


try:
    xs.sort(key=key)
    assert False
except ValueError:
    pass
assert xs == [1, 2, 3]

# Typed float reductions agree with generic storage, including exceptional values.
for values in ([1.5, 2.25, -0.5], [1e16, 1.0, -1e16], [-0.0], [float('inf'), float('-inf')], [float('nan')]):
    generic = values.copy()
    generic.append(None)
    generic.pop()
    for start in (0, True, 3, 0.5, -0.0, 2**100):
        actual = sum(values, start)
        expected = sum(generic, start)
        assert type(actual) is type(expected)
        if actual != actual:
            assert expected != expected
        else:
            assert actual == expected
            assert str(actual) == str(expected)
empty_float = [1.0]
empty_float.pop()
assert sum(empty_float, True) is True
assert sum(empty_float, None) is None
assert str(sum([-0.0], -0.0)) == '-0.0'
try:
    sum([1.0], None)
    assert False
except TypeError:
    pass
