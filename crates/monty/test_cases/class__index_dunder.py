# `__index__` dispatch: a user class usable anywhere CPython accepts an index.
import itertools


class Idx:
    def __index__(self):
        return 2


class Zero:
    def __index__(self):
        return 0


class Negative:
    def __index__(self):
        return -1


class BigIdx:
    def __index__(self):
        return 10**30


class NegBigIdx:
    def __index__(self):
        return -(10**30)


i = Idx()

# === subscripting ===
assert [10, 20, 30][i] == 30
assert (7, 8, 9)[i] == 9
assert 'abcdef'[i] == 'c'
assert b'abcdef'[i] == 99
assert range(10)[i] == 2
assert [10, 20, 30][Negative()] == 30

# Heap-allocated str/bytes take the same path as the interned literals above.
built = 'abc' + 'def'
assert built[i] == 'c'
built_bytes = b'abc' + b'def'
assert built_bytes[i] == 99

# === slicing ===
assert 'abcdef'[i:] == 'cdef'
assert 'abcdef'[:i] == 'ab'
assert 'abcdef'[::i] == 'ace'
assert [1, 2, 3, 4][i:] == [3, 4]
assert [1, 2, 3, 4][Zero() : i] == [1, 2]
assert b'abcdef'[i:] == b'cdef'

# === slice bounds beyond i64 clamp, they do not raise ===
# Both a bare int literal and an `__index__` returning one land in the same
# conversion; unlike plain indexing, slicing clamps instead of raising.
assert [1, 2, 3][10**30 :] == []
assert [1, 2, 3][: 10**30] == [1, 2, 3]
assert [1, 2, 3][-(10**30) :] == [1, 2, 3]
assert [1, 2, 3][BigIdx() :] == []
assert [1, 2, 3][: BigIdx()] == [1, 2, 3]
assert [1, 2, 3][NegBigIdx() :] == [1, 2, 3]
assert [1, 2, 3][slice(BigIdx(), None)] == []
assert 'abc'[BigIdx() :] == ''
assert 'abc'[: BigIdx()] == 'abc'
assert [1, 2, 3][:: 10**30] == [1]
assert [1, 2, 3][:: -(10**30)] == [3]

# === integer arguments ===
# NB sequence repetition (`'ab' * Idx()`) is NOT covered — each `py_mul_impl`
# owns its own coercion; see limitations/classes.md.
assert list(range(i)) == [0, 1]
assert list(range(Zero(), i)) == [0, 1]
assert list(itertools.repeat('x', i)) == ['x', 'x']
assert 'abcabc'.find('b', i) == 4
assert b'abcabc'.find(b'b', i) == 4
assert b'a,b,c'.split(b',', i) == [b'a', b'b', b'c']
assert 'x'.center(Idx()) == 'x '


# The bytes methods convert their integer arguments before checking that the
# first one is bytes-like, so a raising bound wins over the `str` TypeError.
class BoundBoom:
    def __index__(self):
        raise RuntimeError('bound')


try:
    b'abc'.find('a', BoundBoom())
    assert False, 'expected the bound to convert before the sub check'
except RuntimeError as exc:
    assert str(exc) == 'bound'

try:
    b'abc'.split('a', BoundBoom())
    assert False, 'expected maxsplit to convert before the sep check'
except RuntimeError as exc:
    assert str(exc) == 'bound'

# === a non-int return is rejected ===


class BadReturn:
    def __index__(self):
        return 'nope'


try:
    [1, 2, 3][BadReturn()]
    assert False, 'expected __index__ returning str to raise'
except TypeError as exc:
    assert str(exc) == '__index__ returned non-int (type str)'

try:
    'abc'[BadReturn() :]
    assert False, 'expected __index__ returning str to raise in a slice'
except TypeError as exc:
    assert str(exc) == '__index__ returned non-int (type str)'

# === a class without __index__ is still rejected ===


class NoIndex:
    pass


try:
    'abc'[NoIndex() :]
    assert False, 'expected a class without __index__ to raise'
except TypeError as exc:
    assert str(exc) == 'slice indices must be integers or None or have an __index__ method'

try:
    range(NoIndex())
    assert False, 'expected a class without __index__ to raise'
except TypeError as exc:
    assert str(exc) == "'NoIndex' object cannot be interpreted as an integer"

# === a raising __index__ propagates ===


class Boom:
    def __index__(self):
        raise ValueError('boom')


try:
    [1, 2, 3][Boom()]
    assert False, 'expected a raising __index__ to propagate'
except ValueError as exc:
    assert str(exc) == 'boom'

# === __index__ is looked up on the class, not the instance ===
inst = NoIndex()
inst.__index__ = lambda: 1
try:
    [1, 2, 3][inst]
    assert False, 'expected an instance-dict __index__ to be ignored'
except TypeError as exc:
    assert str(exc) != ''

# === out of range is an IndexError, not a coercion failure ===
try:
    [1, 2, 3][Idx().__index__() + 10]
    assert False, 'expected an out-of-range index to raise'
except IndexError as exc:
    assert str(exc) == 'list index out of range'

# === an oversized __index__ return is a catchable IndexError ===
# The dunder runs a nested interpreter loop, so the coercion's own error is
# raised after that loop returns. It must still reach the enclosing handler,
# including at module scope, and name the object asked for the index rather
# than the `int` it returned.


class WideIdx:
    def __index__(self):
        return 10**30


wide_idx = WideIdx()
try:
    [1, 2, 3][wide_idx]
    assert False, 'expected an oversized __index__ to raise'
except IndexError as exc:
    assert str(exc) == "cannot fit 'WideIdx' into an index-sized integer"

# A plain int keeps CPython's `int` wording.
try:
    [1, 2, 3][10**30]
    assert False, 'expected an oversized literal index to raise'
except IndexError as exc:
    assert str(exc) == "cannot fit 'int' into an index-sized integer"


# The same coercion inside a function frame, which took a different path.
def _wide_in_function():
    try:
        [1, 2, 3][WideIdx()]
        return 'no raise'
    except IndexError as exc:
        return str(exc)


assert _wide_in_function() == "cannot fit 'WideIdx' into an index-sized integer"

# An integer argument coerced by a builtin call raises catchably too.
try:
    'x'.center(WideIdx())
    assert False, 'expected an oversized width to raise'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

# === a mutating __index__ resolves against the post-call list ===
# `__index__` can run arbitrary code, so it may resize the very list being
# indexed. Every length must be read *after* the dunder returns; reading it
# first resolves a negative bound against a stale length.
shrink_target = [9, 9, 9, 9, 9, 9, 9, 9, 7, 7]


class ShrinkStart:
    def __index__(self):
        while len(shrink_target) > 3:
            shrink_target.pop()
        return -2


# -2 normalizes against the post-call length 3, so the search starts at 1.
assert shrink_target.index(9, ShrinkStart()) == 1
assert shrink_target == [9, 9, 9]

grow_target = [5, 6, 7, 8, 9, 10, 11, 12]


class GrowStart:
    def __index__(self):
        grow_target.extend([99, 99, 99, 99, 99, 99, 99, 99])
        return -3


assert grow_target.index(99, GrowStart()) == 13

end_target = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]


class ShrinkEnd:
    def __index__(self):
        while len(end_target) > 4:
            end_target.pop()
        return -1


# The window becomes [0, 3), so the 4 left at index 3 is out of range.
try:
    end_target.index(4, 0, ShrinkEnd())
    assert False, 'expected the stale end bound to exclude the match'
except ValueError as exc:
    assert str(exc) == 'list.index(x): x not in list'

# Subscripting reads the length after the dunder too.
sub_target = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]


class ShrinkSubscript:
    def __index__(self):
        sub_target.clear()
        return 9


try:
    sub_target[ShrinkSubscript()]
    assert False, 'expected the emptied list to raise IndexError'
except IndexError as exc:
    assert str(exc) == 'list index out of range'

grow_sub = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]


class GrowSubscript:
    def __index__(self):
        grow_sub.extend(range(100))
        return 50


assert grow_sub[GrowSubscript()] == 40
