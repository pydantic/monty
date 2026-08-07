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
assert 'x'.center(Idx()) == 'x '

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
