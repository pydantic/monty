import operator

# === arithmetic ===
assert operator.add(1, 2) == 3
assert operator.add('a', 'b') == 'ab'
assert operator.sub(5, 3) == 2
assert operator.mul(3, 4) == 12
assert operator.mul('ab', 2) == 'abab'
assert operator.truediv(7, 2) == 3.5
assert operator.floordiv(7, 2) == 3
assert operator.mod(7, 3) == 1
assert operator.pow(2, 10) == 1024
assert operator.neg(5) == -5
assert operator.pos(-5) == -5
assert operator.abs(-5) == 5
assert operator.abs(-2.5) == 2.5
assert operator.index(7) == 7
assert operator.index(True) == 1

# === bitwise ===
assert operator.and_(12, 10) == 8
assert operator.or_(12, 10) == 14
assert operator.xor(12, 10) == 6
assert operator.invert(5) == -6
assert operator.lshift(1, 4) == 16
assert operator.rshift(16, 2) == 4
assert operator.and_({1, 2}, {2, 3}) == {2}

# === comparison and logic ===
assert operator.eq(1, 1) is True
assert operator.ne(1, 1) is False
assert operator.lt(1, 2) is True
assert operator.le(2, 2) is True
assert operator.gt(1, 2) is False
assert operator.ge(2, 3) is False
assert operator.not_(0) is True
assert operator.not_([1]) is False
assert operator.truth([1]) is True
assert operator.truth('') is False
a = []
assert operator.is_(a, a) is True
assert operator.is_(a, []) is False
assert operator.is_not(a, []) is True

# === sequences ===
assert operator.contains([1, 2], 2) is True
assert operator.contains('abc', 'z') is False
assert operator.getitem([1, 2, 3], 1) == 2
assert operator.getitem({'a': 1}, 'a') == 1
assert operator.getitem('abcd', slice(1, 3)) == 'bc'
assert operator.concat([1], [2]) == [1, 2]
assert operator.concat('a', 'b') == 'ab'
assert operator.countOf([1, 2, 1, 3, 1], 1) == 3
assert operator.countOf('hello', 'l') == 2
assert operator.indexOf([5, 6, 7], 7) == 2

# === in-place ===
lst = [1]
assert operator.iadd(lst, [2]) is lst
assert lst == [1, 2]
assert operator.iconcat(lst, [3]) is lst
assert lst == [1, 2, 3]
assert operator.iadd(1, 2) == 3
assert operator.isub(5, 2) == 3
assert operator.imul(3, 3) == 9
assert operator.itruediv(9, 2) == 4.5
assert operator.ifloordiv(9, 2) == 4
assert operator.imod(9, 4) == 1
assert operator.ipow(2, 3) == 8
assert operator.iand(6, 3) == 2
assert operator.ior(4, 1) == 5
assert operator.ixor(6, 3) == 5
assert operator.ilshift(1, 3) == 8
assert operator.irshift(8, 3) == 1

d = {}
assert operator.setitem(d, 'k', 1) is None
assert d == {'k': 1}

# === call ===
assert operator.call(len, [1, 2]) == 2
assert operator.call(lambda x, y=0: x + y, 1, y=2) == 3


# === user classes dispatch ===
class V:
    def __init__(self, n):
        self.n = n

    def __add__(self, other):
        return V(self.n + other.n)

    def __matmul__(self, other):
        return self.n * other.n


assert operator.add(V(1), V(2)).n == 3
assert operator.matmul(V(3), V(4)) == 12


# === as callables ===
assert list(map(operator.neg, [1, 2])) == [-1, -2]
assert sorted([3, 1, 2], key=operator.neg) == [3, 2, 1]

# === errors ===
try:
    operator.add(1, 'a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'int' and 'str'"
try:
    operator.truediv(1, 0)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'
try:
    operator.lt(1, 'a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'int' and 'str'"
try:
    operator.abs('a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "bad operand type for abs(): 'str'"
try:
    operator.index('a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"
try:
    operator.indexOf([1], 2)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'sequence.index(x): x not in sequence'
try:
    operator.getitem([1], 5)
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'list index out of range'
try:
    operator.add(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'add expected 2 arguments, got 1'
try:
    operator.neg()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '_operator.neg() takes exactly one argument (0 given)'
try:
    operator.add(1, 2, x=3)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '_operator.add() takes no keyword arguments'
