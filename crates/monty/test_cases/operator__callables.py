import functools
import operator


# === itemgetter ===
g = operator.itemgetter(1)
assert g('abc') == 'b'
assert g([1, 2, 3]) == 2
assert operator.itemgetter('a')({'a': 5}) == 5
assert operator.itemgetter(0, 2)('abc') == ('a', 'c')
assert operator.itemgetter(slice(1, 3))('abcd') == 'bc'
assert repr(operator.itemgetter('a')) == "operator.itemgetter('a')"
assert repr(operator.itemgetter('a', 1)) == "operator.itemgetter('a', 1)"
assert type(g) is operator.itemgetter

pairs = [('b', 2), ('a', 3), ('c', 1)]
assert sorted(pairs, key=operator.itemgetter(0)) == [('a', 3), ('b', 2), ('c', 1)]
assert sorted(pairs, key=operator.itemgetter(1)) == [('c', 1), ('b', 2), ('a', 3)]
assert sorted(pairs, key=operator.itemgetter(1, 0), reverse=True) == [('a', 3), ('b', 2), ('c', 1)]
assert min(pairs, key=operator.itemgetter(1)) == ('c', 1)
assert max(pairs, key=operator.itemgetter(1)) == ('a', 3)
pairs.sort(key=operator.itemgetter(1))
assert pairs == [('c', 1), ('b', 2), ('a', 3)]
assert list(map(operator.itemgetter(0), pairs)) == ['c', 'b', 'a']

try:
    operator.itemgetter()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'itemgetter expected 1 argument, got 0'
try:
    operator.itemgetter(1, x=2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'itemgetter() takes no keyword arguments'
try:
    g()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'itemgetter expected 1 argument, got 0'
try:
    g([1], [2])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'itemgetter expected 1 argument, got 2'
try:
    g([1])
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'list index out of range'
try:
    operator.itemgetter('x')({})
    assert False, 'expected KeyError'
except KeyError as exc:
    assert exc.args == ('x',)


# === attrgetter ===
class Inner:
    def __init__(self, v):
        self.v = v


class Outer:
    def __init__(self, inner):
        self.a = 1
        self.b = inner


o = Outer(Inner(7))
assert operator.attrgetter('a')(o) == 1
assert operator.attrgetter('b.v')(o) == 7
assert operator.attrgetter('a', 'b.v')(o) == (1, 7)
assert repr(operator.attrgetter('a.b')) == "operator.attrgetter('a.b')"
assert repr(operator.attrgetter('a', 'b.c')) == "operator.attrgetter('a', 'b.c')"
assert type(operator.attrgetter('a')) is operator.attrgetter
outers = [Outer(Inner(3)), Outer(Inner(1)), Outer(Inner(2))]
assert [x.b.v for x in sorted(outers, key=operator.attrgetter('b.v'))] == [1, 2, 3]
assert max(outers, key=operator.attrgetter('b.v')).b.v == 3

try:
    operator.attrgetter()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'attrgetter expected 1 argument, got 0'
try:
    operator.attrgetter(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'attribute name must be a string'
try:
    operator.attrgetter('zz')(o)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'Outer' object has no attribute 'zz'"
try:
    operator.attrgetter('a.zz')(o)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'int' object has no attribute 'zz'"


# === methodcaller ===
assert operator.methodcaller('upper')('abc') == 'ABC'
assert operator.methodcaller('split', ',')('a,b') == ['a', 'b']
assert operator.methodcaller('split', ',', maxsplit=1)('a,b,c') == ['a', 'b,c']
assert repr(operator.methodcaller('m')) == "operator.methodcaller('m')"
assert repr(operator.methodcaller('m', 1, x=2)) == "operator.methodcaller('m', 1, x=2)"
assert type(operator.methodcaller('m')) is operator.methodcaller
assert sorted(['b', 'A', 'c'], key=operator.methodcaller('lower')) == ['A', 'b', 'c']

try:
    operator.methodcaller()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'methodcaller needs at least one argument, the method name'
try:
    operator.methodcaller(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'method name must be a string'
try:
    operator.methodcaller('nope')('x')
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'str' object has no attribute 'nope'"


# === identity and hashing ===
assert hash(g) == hash(g)
assert {g: 1}[g] == 1
assert g == g
assert g != operator.itemgetter(1)

# === with reduce ===
assert functools.reduce(operator.add, [1, 2, 3, 4]) == 10
assert functools.reduce(operator.mul, [1, 2, 3, 4], 10) == 240
