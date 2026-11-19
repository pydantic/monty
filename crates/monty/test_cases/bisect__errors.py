import bisect
from collections import Counter, defaultdict

# Exact Clinic binding messages and conversion/error ordering, for all functions.
for f, name in [
    (bisect.bisect_left, 'bisect_left'),
    (bisect.bisect_right, 'bisect_right'),
    (bisect.insort_left, 'insort_left'),
    (bisect.insort_right, 'insort_right'),
]:
    cases = [
        (([], 1), {'lo': -1}, ValueError, 'lo must be non-negative'),
        (([], 1), {'lo': None}, TypeError, "'NoneType' object cannot be interpreted as an integer"),
        (([], 1), {'hi': 1.5, 'lo': 0.5}, TypeError, "'float' object cannot be interpreted as an integer"),
        (([], 1), {'lo': 1.5}, TypeError, "'float' object cannot be interpreted as an integer"),
        (([], 1), {'lo': 10**50}, OverflowError, 'Python int too large to convert to C ssize_t'),
        (([], 1), {'hi': 10**50}, OverflowError, "cannot fit 'int' into an index-sized integer"),
        (([], 1), {'lo': -1, 'hi': 1.5}, TypeError, "argument should be integer or None, not 'float'"),
        ((1, 1), {}, TypeError, "object of type 'int' has no len()"),
        ((1, 1), {'hi': 0}, TypeError, "'int' object does not support indexing"),
        (({}, 1), {}, TypeError, 'dict is not a sequence'),
        ((defaultdict(int), 1), {}, TypeError, 'collections.defaultdict is not a sequence'),
        ((set(), 1), {}, TypeError, "'set' object does not support indexing"),
        (([1], 2), {'hi': 99}, IndexError, 'list index out of range'),
    ]
    for args, kwargs, error, message in cases:
        try:
            f(*args, **kwargs)
            assert False, 'expected failure'
        except error as exc:
            assert str(exc) == message

# Search evaluates no keys for empty intervals; insertion evaluates x first.
assert bisect.bisect_left([], 0, key=42) == 0
assert bisect.bisect_right([], 0, key=42) == 0
for f in [bisect.insort_left, bisect.insort_right]:
    try:
        f([], 0, key=42)
        assert False, 'expected failure'
    except TypeError as exc:
        assert str(exc) == "'int' object is not callable"
    try:
        f([], 0, lo=-1, key=lambda x: 1 / 0)
        assert False, 'expected failure'
    except ZeroDivisionError as exc:
        assert str(exc) == 'division by zero'
    try:
        f((1, 2), 1)
        assert False, 'expected failure'
    except AttributeError as exc:
        assert str(exc) == "'tuple' object has no attribute 'insert'"

for f, message in [
    (bisect.bisect_left, "'<' not supported between instances of 'str' and 'int'"),
    (bisect.bisect_right, "'<' not supported between instances of 'int' and 'str'"),
]:
    try:
        f(['a'], 1)
        assert False, 'expected failure'
    except TypeError as exc:
        assert str(exc) == message

# Exceptions from key functions propagate and insertion leaves the list alone.
for f in [bisect.bisect_left, bisect.bisect_right, bisect.insort_left, bisect.insort_right]:
    a = [1, 2]
    try:
        f(a, 1, key=lambda x: 1 / 0)
        assert False, 'expected failure'
    except ZeroDivisionError as exc:
        assert str(exc) == 'division by zero'
    assert a == [1, 2]


try:
    bisect.insort_left(Counter(), 3)
    assert False, 'expected failure'
except AttributeError as exc:
    assert str(exc) == "'Counter' object has no attribute 'insert'"

# Interned strings/bytes take a different insertion-error path than heap containers.
for insert in [bisect.insort_left, bisect.insort_right]:
    for sequence, value, message in [
        ('abc', 'b', "'str' object has no attribute 'insert'"),
        (b'abc', 98, "'bytes' object has no attribute 'insert'"),
    ]:
        try:
            insert(sequence, value)
            assert False, 'expected failure'
        except AttributeError as exc:
            assert str(exc) == message

# Range lengths can exceed the signed index width without allocating the elements.
for function in [bisect.bisect_left, bisect.bisect_right, bisect.insort_left, bisect.insort_right]:
    for bounds in [{}, {'hi': None}, {'hi': -1}]:
        try:
            function(range(-(2**62), 2**62), 0, **bounds)
            assert False, 'expected failure'
        except OverflowError as exc:
            assert str(exc) == 'Python int too large to convert to C ssize_t'

# An explicit upper bound avoids consulting even an overflowing range length.
assert bisect.bisect_left(range(-(2**62), 2**62), 0, hi=0) == 0
assert bisect.bisect_right(range(-(2**62), 2**62), 0, hi=1) == 1


class EqualityFailure:
    def __eq__(self, other):
        raise ValueError('equality failed')


# Lexicographic searches must propagate an element's equality exception.
for search in [bisect.bisect_left, bisect.bisect_right]:
    try:
        search([[EqualityFailure()]], [EqualityFailure()])
        assert False, 'expected failure'
    except ValueError as exc:
        assert str(exc) == 'equality failed'
