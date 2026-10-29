# === property: decorator form ===
class Temp:
    def __init__(self, c):
        self._c = c

    @property
    def f(self):
        return self._c * 9 / 5 + 32

    @f.setter
    def f(self, value):
        self._c = (value - 32) * 5 / 9


t = Temp(100)
assert t.f == 212.0
t.f = 32
assert t._c == 0.0
assert t.f == 32.0
assert type(Temp.f) is property
assert isinstance(Temp.f, property)
assert repr(Temp.f).startswith('<property object at 0x')

# === property: getattr / hasattr / setattr go through the descriptor ===
assert getattr(t, 'f') == 32.0
assert hasattr(t, 'f')
setattr(t, 'f', 212)
assert t._c == 100.0


# === property: read-only and missing getter ===
class ReadOnly:
    @property
    def x(self):
        return 1


r = ReadOnly()
assert r.x == 1
try:
    r.x = 2
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "property 'x' of 'ReadOnly' object has no setter"


class NoGetter:
    x = property()


try:
    NoGetter().x
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "property 'x' of 'NoGetter' object has no getter"
assert not hasattr(NoGetter(), 'x'), 'hasattr is False for a getter-less property'


# === property: the getter's exceptions propagate ===
class Raises:
    @property
    def boom(self):
        raise ValueError('boom')


try:
    Raises().boom
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'boom'


# === property: a data descriptor wins over a same-named instance write ===
class Counter:
    def __init__(self):
        self.writes = []

    def _get(self):
        return len(self.writes)

    def _set(self, value):
        self.writes.append(value)

    n = property(_get, _set, None, 'the count')


c = Counter()
c.n = 'a'
c.n = 'b'
assert c.n == 2
assert c.writes == ['a', 'b']
assert Counter.n.__doc__ == 'the count'
assert Counter.n.fget is Counter._get
assert Counter.n.fset is Counter._set
assert Counter.n.fdel is None


# === property: getter/setter/deleter return copies ===
def get_one(self):
    return 1


def set_one(self, value):
    pass


base = property(get_one)
with_setter = base.setter(set_one)
assert with_setter is not base
assert with_setter.fget is get_one
assert with_setter.fset is set_one
assert base.fset is None
assert base.getter(set_one).fget is set_one
assert base.deleter(set_one).fdel is set_one
assert repr(base.setter).startswith('<built-in method setter of property object at 0x')
assert property().__doc__ is None
assert property(doc='d').__doc__ == 'd'


# === property: calling the value a property returns ===
class Holder:
    @property
    def handler(self):
        return lambda x: x * 2


assert Holder().handler(21) == 42


# === property: added to the class after instances exist ===
class Late:
    pass


def value(self):
    return 'late'


late = Late()
Late.value = property(value)
assert late.value == 'late'
try:
    late.value = 1
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "property 'value' of 'Late' object has no setter"


# === staticmethod ===
class Math:
    @staticmethod
    def add(a, b):
        return a + b

    def call_add(self):
        return self.add(1, 1)


assert Math.add(1, 2) == 3
assert Math().add(3, 4) == 7
assert Math().call_add() == 2
assert Math.add is Math().add
add_ref = Math.add
assert add_ref(5, 5) == 10


def plain(x):
    return x


sm = staticmethod(plain)
assert type(sm) is staticmethod
assert sm.__func__ is plain
assert sm.__wrapped__ is plain
assert sm(7) == 7
assert repr(sm).startswith('<staticmethod(<function ')


# === classmethod ===
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    @classmethod
    def origin(cls):
        return cls(0, 0)

    @classmethod
    def describe(cls, suffix):
        return cls is Point, suffix


p = Point.origin()
assert (p.x, p.y) == (0, 0)
assert type(p) is Point
assert Point(1, 2).origin().x == 0
assert Point.describe('!') == (True, '!')
assert Point(1, 2).describe('?') == (True, '?')
bound = Point.origin
assert bound().y == 0

cm = classmethod(plain)
assert type(cm) is classmethod
assert cm.__func__ is plain
assert cm.__wrapped__ is plain
assert repr(cm).startswith('<classmethod(<function ')
try:
    cm(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'classmethod' object is not callable"


# === other method decorators ===
def twice(func):
    def wrapper(self, x):
        return func(self, func(self, x))

    return wrapper


class Doubler:
    @twice
    def inc(self, x):
        return x + 1

    @staticmethod
    @twice
    def unused(self, x):
        return x


assert Doubler().inc(0) == 2
assert Doubler.unused(None, 5) == 5

# === constructor argument errors ===
try:
    property(1, 2, 3, 4, 5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'property() takes at most 4 arguments (5 given)'
try:
    staticmethod()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'staticmethod expected 1 argument, got 0'
try:
    classmethod(1, 2)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'classmethod expected 1 argument, got 2'
try:
    property().setter()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'property.setter() takes exactly one argument (0 given)'
