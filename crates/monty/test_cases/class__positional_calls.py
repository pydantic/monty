import sys


class Record:
    def __init__(self, first, second, third, /):
        self.values = (first, second, third)

    def receiver(self):
        return self

    def pair(self, first, second, /):
        return (self.values, first, second)

    def many(self, a, b, c, d, e, f, g, h, i):
        return (a, b, c, d, e, f, g, h, i)

    def recurse(self, depth, value):
        if depth:
            return self.recurse(depth - 1, value)
        return value

    def fail(self, value):
        raise ValueError(value)


shared = ['kept']
record = Record(shared, shared, [3])
assert record.receiver() is record
assert record.values[0] is shared
assert record.values[1] is shared

for _ in range(5):
    direct = record.pair(shared, [4])
    cached = record.pair
    indirect = cached(shared, [4])
    assert direct == indirect == ((shared, shared, [3]), shared, [4])
    assert direct[1] is shared
    assert indirect[1] is shared
    assert record.many(1, 2, 3, 4, 5, 6, 7, 8, shared)[8] is shared
    assert record.recurse(5, shared) is shared

assert Record([1], [2], [3]).receiver().values == ([1], [2], [3])
assert (Record(1, 2, 3).values, Record(4, 5, 6).values) == ((1, 2, 3), (4, 5, 6))

# An instance attribute overrides the class method without binding a receiver.
record.pair = lambda first, second: (first, second)
assert record.pair(shared, 5) == (shared, 5)
record = Record(shared, shared, [3])
assert record.pair(6, 7) == ((shared, shared, [3]), 6, 7)


class Fallbacks:
    def __init__(self, value=10):
        self.value = value

    def default(self, extra=2):
        return self.value + extra

    def keywords(self, first, *, second):
        return (first, second)

    def variadic(self, *values):
        return values


fallback = Fallbacks()
assert fallback.default() == 12
assert fallback.default(3) == 13
assert fallback.default(extra=4) == 14
assert fallback.keywords([1], second=[2]) == ([1], [2])
assert fallback.variadic(1, 2, 3) == (1, 2, 3)
assert Fallbacks(value=11).value == 11


def make_class(captured):
    class Captured:
        def __init__(self, value):
            self.value = value + captured

        def read(self, extra):
            return self.value + captured + extra

    return Captured


assert make_class(3)(4).read(5) == 15


class Failing:
    def __init__(self, value):
        raise ValueError(value)


for _ in range(5):
    try:
        Failing('constructor')
    except ValueError as exc:
        assert str(exc) == 'constructor'
    else:
        assert False

    try:
        record.fail('method')
    except ValueError as exc:
        assert str(exc) == 'method'
    else:
        assert False

    try:
        record.recurse(2, shared).missing
    except AttributeError:
        pass
    else:
        assert False

    try:
        Record(1, 2)
    except TypeError as exc:
        assert str(exc).endswith("__init__() missing 1 required positional argument: 'third'")
    else:
        assert False

    try:
        record.pair(1, 2, 3)
    except TypeError as exc:
        assert str(exc).endswith('pair() takes 3 positional arguments but 4 were given')
    else:
        assert False

    assert Record(1, 2, 3).pair(4, 5) == ((1, 2, 3), 4, 5)


class BadReturn:
    def __init__(self, value):
        return value


try:
    BadReturn([1])
except TypeError as exc:
    assert str(exc) == "__init__() should return None, not 'list'"
else:
    assert False

assert Record(1, 2, 3).receiver().values == (1, 2, 3)


# Rejected frame pushes must release the receiver, arguments and pending instance.
sys.setrecursionlimit(30)


class Recursive:
    def __init__(self, depth, value):
        if depth:
            self.child = Recursive(depth - 1, value)
        else:
            self.value = value


for _ in range(2):
    try:
        Recursive(100, shared)
    except RecursionError:
        pass
    else:
        assert False

    try:
        record.recurse(100, shared)
    except RecursionError:
        pass
    else:
        assert False

    assert Recursive(2, shared).child.child.value is shared
    assert record.recurse(2, shared) is shared
