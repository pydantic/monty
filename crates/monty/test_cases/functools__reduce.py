import functools

# === reduce ===
assert functools.reduce(lambda a, b: a + b, [1, 2, 3, 4]) == 10
assert functools.reduce(lambda a, b: a + b, [1, 2, 3], 10) == 16
assert functools.reduce(lambda a, b: a + b, [], 5) == 5
assert functools.reduce(lambda a, b: a * b, range(1, 6)) == 120
assert functools.reduce(lambda a, b: a + b, 'abc') == 'abc'
assert functools.reduce(lambda a, b: a + b, {'x': 1, 'y': 2}) == 'xy'

# a one-item iterable returns that item without calling the function
assert functools.reduce(lambda a, b: 1 / 0, [7]) == 7
assert functools.reduce(lambda a, b: 1 / 0, [], 7) == 7

# `initial` is also accepted by keyword
assert functools.reduce(lambda a, b: a + b, [1, 2], initial=10) == 13

# the accumulator is threaded left to right
assert functools.reduce(lambda a, b: (a, b), [1, 2, 3]) == ((1, 2), 3)
assert functools.reduce(lambda a, b: a + [b], [1, 2], []) == [1, 2]


# an exception from the function propagates unchanged
def raiser(a, b):
    raise ValueError('inner')


try:
    functools.reduce(raiser, [1, 2])
    assert False, 'expected reduce to fail'
except ValueError as exc:
    assert str(exc) == 'inner'

try:
    functools.reduce(lambda a, b: a, [])
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == 'reduce() of empty iterable with no initial value'

# the callable is never checked up front, so an empty iterable wins
try:
    functools.reduce(5, [])
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == 'reduce() of empty iterable with no initial value'

try:
    functools.reduce(5, [1, 2])
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == "'int' object is not callable"

try:
    functools.reduce(lambda a, b: a, 5)
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == 'reduce() arg 2 must support iteration'

try:
    functools.reduce(lambda a, b: a)
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == 'reduce() takes at least 2 positional arguments (1 given)'

try:
    functools.reduce(lambda a, b: a, [1], 2, 3)
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == 'reduce() takes at most 3 arguments (4 given)'

try:
    functools.reduce(lambda a, b: a, [1], bogus=1)
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == "reduce() got an unexpected keyword argument 'bogus'"

# `function` and `iterable` are positional-only, so the arity error comes first
try:
    functools.reduce(function=lambda a, b: a, iterable=[1])
    assert False, 'expected reduce to fail'
except TypeError as exc:
    assert str(exc) == 'reduce() takes at least 2 positional arguments (0 given)'
