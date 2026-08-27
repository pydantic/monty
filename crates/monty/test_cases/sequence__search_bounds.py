# `start`/`end` bounds of the sequence searches follow CPython's slice-index
# rules: `__index__` is dispatched, and an out-of-range int clamps to the
# sequence rather than raising.
from collections import deque, namedtuple

P = namedtuple('P', 'a b')


class Idx:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


# === Oversized bounds clamp instead of raising ===
BIG = 10**30
assert 'abcabc'.find('b', BIG) == -1
assert 'abcabc'.find('b', -BIG) == 1
assert 'abcabc'.find('b', 0, BIG) == 1
assert 'abcabc'.count('b', BIG) == 0
assert 'abcabc'.startswith('b', BIG) is False
assert 'abcabc'.endswith('c', 0, BIG) is True
assert b'abcabc'.find(b'b', BIG) == -1
assert b'abcabc'.find(b'b', -BIG) == 1
assert b'abcabc'.count(b'b', 0, BIG) == 2
assert b'abcabc'.startswith(b'a', -BIG) is True
assert [1, 2, 3].index(2, -BIG) == 1
assert (1, 2, 3).index(2, -BIG) == 1
assert P(1, 2).index(2, -BIG) == 1
assert deque([1, 2, 3]).index(2, -BIG) == 1

# A bound past the end still finds nothing, reported by each search's own error.
try:
    'abcabc'.index('b', BIG)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'substring not found'

try:
    [1, 2, 3].index(2, BIG)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'list.index(x): x not in list'

try:
    (1, 2, 3).index(2, BIG)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'tuple.index(x): x not in tuple'

# === `__index__` bounds ===
assert 'abcabc'.find('b', Idx(2)) == 4
assert b'abcabc'.find(b'b', Idx(2)) == 4
assert [1, 2, 3].index(2, Idx(0)) == 1
assert (1, 2, 3).index(2, Idx(0)) == 1
assert deque([1, 2, 3]).index(2, Idx(0)) == 1
# One whose `__index__` overflows clamps like a literal of the same size.
assert 'abcabc'.find('b', Idx(-BIG)) == 1
assert [1, 2, 3].index(2, Idx(-BIG)) == 1

# `bool` is an int, so it indexes directly.
assert 'abcabc'.find('b', True) == 1
assert [1, 2, 3].index(2, True) == 1

# === `None` bounds ===
# Real slicing accepts `None` for "no bound", and so do the string searches.
assert 'abcabc'.find('b', None) == 1
assert 'abcabc'.find('b', None, None) == 1
assert b'abcabc'.find(b'b', None) == 1
assert b'abcabc'.startswith(b'b', None, None) is False

# `index()` on a sequence does not: there is no bound to leave unset.
for call in (
    lambda: [1, 2, 3].index(2, None),
    lambda: (1, 2, 3).index(2, None),
    lambda: deque([1, 2, 3]).index(2, None),
):
    try:
        call()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'slice indices must be integers or have an __index__ method'

# === Bounds that are not integers at all ===
for call in (
    lambda: 'abcabc'.find('b', 'x'),
    lambda: 'abcabc'.find('b', 1.0),
    lambda: b'abcabc'.find(b'b', 'x'),
    lambda: b'abcabc'.startswith(b'b', []),
):
    try:
        call()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'slice indices must be integers or None or have an __index__ method'

for call in (
    lambda: [1, 2, 3].index(2, 'x'),
    lambda: (1, 2, 3).index(2, 1.0),
    lambda: P(1, 2).index(2, 'x'),
    lambda: deque([1, 2, 3]).index(2, 1.0),
):
    try:
        call()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'slice indices must be integers or have an __index__ method'


# === A bound whose `__index__` mutates the sequence ===
# The length is read after every bound is coerced, so a negative bound resolves
# against the mutated sequence and the walk never runs off the end of a
# shortened one.
class Clear:
    def __init__(self, target):
        self.target = target

    def __index__(self):
        self.target.clear()
        return 0


for empty in (deque([1, 2, 3]), [1, 2, 3]):
    try:
        empty.index(2, Clear(empty))
        assert False, 'expected ValueError'
    except ValueError:
        pass

cleared = deque([1, 2, 3])
try:
    cleared.index(2, 0, Clear(cleared))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'deque.index(x): x not in deque'

grown = deque([1, 2, 3])


class Grow:
    def __index__(self):
        grown.extend([2, 2, 2])
        return -1


assert grown.index(2, Grow()) == 5
assert list(grown) == [1, 2, 3, 2, 2, 2]
