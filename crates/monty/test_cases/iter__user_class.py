# A user class implementing the iterator protocol is iterable everywhere:
# `for`, `list()`, `sum()`, comprehensions, `in`, unpacking and `next()`.
#
# `iter()` returns whatever `__iter__` returns, with no wrapper interposed, so
# identity and type are preserved exactly as in CPython.


# === self-iterator: __iter__ returns self ===
class Countdown:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self.n <= 0:
            raise StopIteration
        self.n -= 1
        return self.n + 1


assert [x for x in Countdown(3)] == [3, 2, 1]
assert list(Countdown(3)) == [3, 2, 1]
assert sum(Countdown(4)) == 10
assert tuple(Countdown(2)) == (2, 1)
assert sorted(Countdown(3)) == [1, 2, 3]
assert max(Countdown(3)) == 3
assert set(Countdown(3)) == {1, 2, 3}

collected = []
for x in Countdown(2):
    collected.append(x)
assert collected == [2, 1]

# === iter() interposes no wrapper ===
c = Countdown(2)
assert iter(c) is c
assert type(iter(c)) is Countdown
assert type(iter(c)).__name__ == 'Countdown'


# === separate iterable and iterator; the container is re-iterable ===
class MyRangeIter:
    def __init__(self, hi):
        self.i = 0
        self.hi = hi

    def __iter__(self):
        return self

    def __next__(self):
        if self.i >= self.hi:
            raise StopIteration
        self.i += 1
        return self.i


class MyRange:
    def __init__(self, hi):
        self.hi = hi

    def __iter__(self):
        return MyRangeIter(self.hi)


r = MyRange(3)
assert list(r) == [1, 2, 3]
assert list(r) == [1, 2, 3]
assert iter(r) is not r
assert type(iter(r)) is MyRangeIter

# === next() and next(default) ===
c = Countdown(2)
assert next(c) == 2
assert next(c) == 1
assert next(c, 'DONE') == 'DONE'


# === __iter__ returning a builtin iterator (the common delegation pattern) ===
class Wrap:
    def __init__(self, items):
        self._items = items

    def __iter__(self):
        return iter(self._items)


assert list(Wrap([1, 2, 3])) == [1, 2, 3]
assert sum(Wrap([1, 2, 3])) == 6
w = Wrap([1, 2])
assert (list(w), list(w)) == ([1, 2], [1, 2])
assert list(Wrap('ab')) == ['a', 'b']
assert list(Wrap({'k': 1})) == ['k']


# `__iter__` may return any iterator type, including a `callable_iterator`,
# which `py_is_iterator` has to accept alongside the generic and list ones.
counter = [0]


def bump():
    counter[0] += 1
    return counter[0]


class WrapCallable:
    def __iter__(self):
        return iter(bump, 4)


assert list(WrapCallable()) == [1, 2, 3]


# === every consumption site reaches a user iterable ===
class CD:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self.n <= 0:
            raise StopIteration
        self.n -= 1
        return self.n + 1


assert 2 in CD(3)
assert 9 not in CD(3)
assert [*CD(3)] == [3, 2, 1]
assert (*CD(2),) == (2, 1)
assert {*CD(2)} == {1, 2}
a, b, d = CD(3)
assert (a, b, d) == (3, 2, 1)
head, *tail = CD(3)
assert (head, tail) == (3, [2, 1])
assert '-'.join(Wrap(['a', 'b'])) == 'a-b'
assert b'-'.join(Wrap([b'a', b'b'])) == b'a-b'
assert dict(Wrap([('k', 1)])) == {'k': 1}


# === a user iterator is not latched: __next__ runs again after StopIteration ===
class Resumable:
    def __init__(self):
        self.n = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n >= 2:
            raise StopIteration
        return self.n


res = Resumable()
assert (list(res), list(res)) == ([1], [])


# === __iter__ must return an iterator ===
class BadIter:
    def __iter__(self):
        return 42


try:
    iter(BadIter())
    assert False, 'expected TypeError for a non-iterator __iter__'
except TypeError as e:
    assert str(e) == "iter() returned non-iterator of type 'int'"


class NoNext:
    pass


class ReturnsNoNext:
    def __iter__(self):
        return NoNext()


try:
    list(ReturnsNoNext())
    assert False, 'expected TypeError for an __iter__ result without __next__'
except TypeError as e:
    assert str(e) == "iter() returned non-iterator of type 'NoNext'"


# === __next__ without __iter__: next() works, iter() does not ===
class NextOnly:
    def __next__(self):
        return 1


assert next(NextOnly()) == 1
try:
    iter(NextOnly())
    assert False, 'expected TypeError for a class with no __iter__'
except TypeError as e:
    assert str(e) == "'NextOnly' object is not iterable"


# === a class with neither dunder keeps each site's own message ===
class Plain:
    pass


try:
    next(Plain())
    assert False, 'expected TypeError for next() on a plain instance'
except TypeError as e:
    assert str(e) == "'Plain' object is not an iterator"
try:
    2 in Plain()
    assert False, 'expected TypeError for `in` on a plain instance'
except TypeError as e:
    assert str(e) == "argument of type 'Plain' is not a container or iterable"
try:
    [*Plain()]
    assert False, 'expected TypeError for `*` unpack of a plain instance'
except TypeError as e:
    assert str(e) == 'Value after * must be an iterable, not Plain'
try:
    _p, _q = Plain()
    assert False, 'expected TypeError for unpacking a plain instance'
except TypeError as e:
    assert str(e) == 'cannot unpack non-iterable Plain object'
try:
    ''.join(Plain())
    assert False, 'expected TypeError for join of a plain instance'
except TypeError as e:
    assert str(e) == 'can only join an iterable'


# === __iter__ = None opts out of the protocol ===
# The None is never called: every site reports not-iterable, exactly as if
# __iter__ were absent. The two unpacking sites keep the plain message rather
# than their own wording, because the slot is filled (CPython's slot_tp_iter).
class OptOut:
    __iter__ = None


try:
    iter(OptOut())
    assert False, 'expected TypeError for iter() of an opted-out class'
except TypeError as e:
    assert str(e) == "'OptOut' object is not iterable"
try:
    list(OptOut())
    assert False, 'expected TypeError for list() of an opted-out class'
except TypeError as e:
    assert str(e) == "'OptOut' object is not iterable"
try:
    for _x in OptOut():
        pass
    assert False, 'expected TypeError for a for loop over an opted-out class'
except TypeError as e:
    assert str(e) == "'OptOut' object is not iterable"
try:
    sorted(OptOut())
    assert False, 'expected TypeError for sorted() of an opted-out class'
except TypeError as e:
    assert str(e) == "'OptOut' object is not iterable"
try:
    _p, _q = OptOut()
    assert False, 'expected TypeError for unpacking an opted-out class'
except TypeError as e:
    assert str(e) == "'OptOut' object is not iterable"
try:
    _head, *_tail = OptOut()
    assert False, 'expected TypeError for starred unpacking of an opted-out class'
except TypeError as e:
    assert str(e) == "'OptOut' object is not iterable"
try:
    [*OptOut()]
    assert False, 'expected TypeError for `*` unpack of an opted-out class'
except TypeError as e:
    assert str(e) == "'OptOut' object is not iterable"
try:
    ''.join(OptOut())
    assert False, 'expected TypeError for join of an opted-out class'
except TypeError as e:
    assert str(e) == 'can only join an iterable'
try:
    2 in OptOut()
    assert False, 'expected TypeError for `in` on an opted-out class'
except TypeError as e:
    assert str(e) == "argument of type 'OptOut' is not a container or iterable"


# === __next__ = None is not an opt-out ===
# Only __iter__ (and __contains__) treat None as absent, so this class stays an
# iterator and the None is reached and called.
class NoneNext:
    def __iter__(self):
        return self

    __next__ = None


assert iter(NoneNext()) is not None
try:
    list(NoneNext())
    assert False, 'expected TypeError from calling a None __next__'
except TypeError as e:
    assert str(e) == "'NoneType' object is not callable"


# === exceptions from the dunders propagate unchanged ===
class BoomIter:
    def __iter__(self):
        raise ValueError('boom')


class BoomNext:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('kaboom')


try:
    list(BoomIter())
    assert False, 'expected ValueError from __iter__'
except ValueError as e:
    assert str(e) == 'boom'
try:
    ''.join(BoomIter())
    assert False, 'expected ValueError from __iter__ via join'
except ValueError as e:
    assert str(e) == 'boom'
try:
    list(BoomNext())
    assert False, 'expected ValueError from __next__'
except ValueError as e:
    assert str(e) == 'kaboom'


# === reversed() is not implied by the iterator protocol ===
# CPython needs __reversed__, or __len__ + __getitem__; Monty dispatches
# neither, so a user iterator is never reversible (see limitations/classes.md).
try:
    reversed(CD(2))
    assert False, 'expected TypeError for reversed(user iterator)'
except TypeError as e:
    assert str(e) == "'CD' object is not reversible"
