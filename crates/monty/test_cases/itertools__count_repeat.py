import itertools

# === count: basic progression ===
c = itertools.count()
assert [next(c) for _ in range(3)] == [0, 1, 2]
assert [next(itertools.count(10)) for _ in range(1)] == [10]
c2 = itertools.count(10, 2)
assert [next(c2) for _ in range(3)] == [10, 12, 14]
c3 = itertools.count(2, -1)
assert [next(c3) for _ in range(3)] == [2, 1, 0]
c4 = itertools.count(2.5, 0.5)
assert [next(c4) for _ in range(3)] == [2.5, 3.0, 3.5]

# start/step are accepted as keywords
assert [x for _, x in zip(range(2), itertools.count(start=5))] == [5, 6]
assert [x for _, x in zip(range(2), itertools.count(start=5, step=2))] == [5, 7]
assert [x for _, x in zip(range(2), itertools.count(step=3))] == [0, 3]
# bool is an int subclass, so it counts as a number — and is widened to int,
# which is why the reprs below show `count(1)` rather than `count(True)`
assert [x for _, x in zip(range(2), itertools.count(True))] == [1, 2]
assert [x for _, x in zip(range(3), itertools.count(0, True))] == [0, 1, 2]
assert repr(itertools.count(True)) == 'count(1)'
assert repr(itertools.count(5, True)) == 'count(5)'

# === count: promotion to big integers ===
big = itertools.count(2**62, 2**62)
assert [next(big) for _ in range(3)] == [4611686018427387904, 9223372036854775808, 13835058055282163712]
assert next(itertools.count(2**70)) == 1180591620717411303424

# === count: repr ===
# The step is shown unless it is an *integer* equal to 1.
assert repr(itertools.count()) == 'count(0)'
assert repr(itertools.count(10)) == 'count(10)'
assert repr(itertools.count(10, 2)) == 'count(10, 2)'
assert repr(itertools.count(0, 1.0)) == 'count(0, 1.0)'
assert repr(itertools.count(0, True)) == 'count(0)'
assert repr(itertools.count(2.5, 0.5)) == 'count(2.5, 0.5)'
assert repr(itertools.count(2**70)) == 'count(1180591620717411303424)'
# repr tracks the position, not the start
c5 = itertools.count(0, 2)
next(c5)
assert repr(c5) == 'count(2, 2)'

# === count: protocol ===
c6 = itertools.count()
assert iter(c6) is c6
assert bool(itertools.count())
assert str(type(itertools.count())) == "<class 'itertools.count'>"
# distinct iterators never compare equal, but one is equal to itself
assert (itertools.count() == itertools.count()) is False, 'distinct counts differ'
c7 = itertools.count()
assert (c7 == c7) is True, 'identity equality'

# === count: errors ===
# A non-number start OR step gives the same single message, checked after binding.
for bad in [lambda: itertools.count('x'), lambda: itertools.count(1, 'x'), lambda: itertools.count(step='x')]:
    try:
        bad()
        assert False, 'expected TypeError for non-numeric count argument'
    except TypeError as exc:
        assert str(exc) == 'a number is required'

try:
    itertools.count(1, 2, 3)
    assert False, 'expected TypeError for too many count arguments'
except TypeError as exc:
    assert str(exc) == 'count() takes at most 2 arguments (3 given)'

# arity is counted over positionals AND keywords together
try:
    itertools.count(1, 2, step=3)
    assert False, 'expected TypeError for count arity over the total'
except TypeError as exc:
    assert str(exc) == 'count() takes at most 2 arguments (3 given)'

try:
    itertools.count(bad=1)
    assert False, 'expected TypeError for unexpected count keyword'
except TypeError as exc:
    assert str(exc) == "count() got an unexpected keyword argument 'bad'"

# === repeat: basic ===
assert list(itertools.repeat(7, 3)) == [7, 7, 7]
assert list(itertools.repeat(7, 0)) == []
assert list(itertools.repeat(9, -1)) == []
assert list(itertools.repeat(9, times=2)) == [9, 9]
assert list(itertools.repeat(times=2, object=9)) == [9, 9]
assert list(itertools.repeat(9, True)) == [9]
assert tuple(itertools.repeat('x', 3)) == ('x', 'x', 'x')
assert sum(itertools.repeat(2, 4)) == 8
r = itertools.repeat('a', 2)
assert [next(r), next(r)] == ['a', 'a']
assert next(r, 'exhausted') == 'exhausted'
# an omitted `times` repeats forever
ri = itertools.repeat(5)
assert [next(ri) for _ in range(3)] == [5, 5, 5]

# the SAME object comes back each time, never a copy
obj = [1]
rp = itertools.repeat(obj, 2)
a = next(rp)
b = next(rp)
assert a is b
assert a is obj

# === repeat: repr ===
# The count shown is what is LEFT, so it shrinks as the iterator is consumed.
assert repr(itertools.repeat(7)) == 'repeat(7)'
assert repr(itertools.repeat(7, 3)) == 'repeat(7, 3)'
assert repr(itertools.repeat([1, 2], 2)) == 'repeat([1, 2], 2)'
r2 = itertools.repeat('a', 3)
next(r2)
assert repr(r2) == "repeat('a', 2)"
r3 = itertools.repeat(1, 2)
assert list(r3) == [1, 1]
assert repr(r3) == 'repeat(1, 0)'
assert repr(itertools.repeat(itertools.repeat(1, 1), 2)) == 'repeat(repeat(1, 1), 2)'

# === repeat: protocol ===
r4 = itertools.repeat(1, 2)
assert iter(r4) is r4
assert str(type(itertools.repeat(1))) == "<class 'itertools.repeat'>"
assert 3 in itertools.repeat(3, 2)

# === repeat: errors ===
# `times` goes through the same coercion as CPython's `n` format unit.
for bad_times in ['z', 2.0, None]:
    try:
        itertools.repeat(9, bad_times)
        assert False, 'expected TypeError for non-integer times'
    except TypeError as exc:
        assert str(exc) == f"'{type(bad_times).__name__}' object cannot be interpreted as an integer"

try:
    itertools.repeat(9, 2**70)
    assert False, 'expected OverflowError for oversized times'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

try:
    itertools.repeat()
    assert False, 'expected TypeError for missing repeat object'
except TypeError as exc:
    assert str(exc) == "repeat() missing required argument 'object' (pos 1)"

# a missing required argument is reported before an unexpected keyword
try:
    itertools.repeat(bad=1)
    assert False, 'expected TypeError for missing repeat object'
except TypeError as exc:
    assert str(exc) == "repeat() missing required argument 'object' (pos 1)"

try:
    itertools.repeat(1, 2, 3)
    assert False, 'expected TypeError for too many repeat arguments'
except TypeError as exc:
    assert str(exc) == 'repeat() takes at most 2 arguments (3 given)'

try:
    itertools.repeat(9, 'z', times=1)
    assert False, 'expected TypeError for repeat arity over the total'
except TypeError as exc:
    assert str(exc) == 'repeat() takes at most 2 arguments (3 given)'
