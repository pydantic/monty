# Basic user-defined classes: construction, instance attributes, methods,
# class variables, type()/isinstance(), identity equality and bound methods.


class Point:
    # class variable shared across instances
    origin_count = 0

    def __init__(self, x: int, y: int) -> None:
        self.x = x
        self.y = y

    def total(self) -> int:
        return self.x + self.y

    def scaled(self, factor: int = 2) -> int:
        return self.total() * factor

    def move(self, dx: int, dy: int) -> None:
        self.x += dx
        self.y += dy


# === Construction and __init__ ===
p = Point(3, 4)
assert p.x == 3
assert p.y == 4

# === Instance methods ===
assert p.total() == 7
assert p.scaled() == 14
assert p.scaled(3) == 21
assert p.scaled(factor=10) == 70

# === Mutating attributes via a method ===
p.move(1, 1)
assert p.x == 4
assert p.y == 5
assert p.total() == 9

# === Mutating attributes directly ===
p.x = 100
assert p.x == 100
assert p.total() == 105

# === Setting a new attribute not declared in __init__ ===
p.z = 7
assert p.z == 7

# === Class variables ===
assert Point.origin_count == 0
assert p.origin_count == 0
q = Point(1, 1)
assert q.origin_count == 0

# === Independent instances ===
assert p.x == 100 and q.x == 1, 'instances have independent attributes'

# === type() returns the class object ===
assert type(p) is Point
assert type(p) is type(q)
assert type(p).__name__ == 'Point'

# === isinstance ===
assert isinstance(p, Point)
assert isinstance(p, (int, Point))
assert not isinstance(5, Point), 'isinstance false for a non-instance'


class Other:
    def __init__(self) -> None:
        self.v = 1


o = Other()
assert not isinstance(o, Point), 'isinstance false for a different class'
assert type(o) is not Point

# === Identity equality (no user __eq__) ===
assert p == p
assert p != q
assert (p == q) is False

# === Instances are always truthy ===
assert bool(p) is True
if q:
    pass
else:
    assert False, 'instance should be truthy in a condition'

# === Bound methods ===
m = p.total
assert m() == 105
move = p.move
move(10, 10)
assert p.x == 110 and p.y == 15, 'bound method with arguments mutates the instance'

# === getattr() / hasattr() ===
assert getattr(p, 'x') == 110
assert getattr(p, 'total')() == 125
assert getattr(p, 'nope', 'default') == 'default'
assert hasattr(p, 'x')
assert hasattr(p, 'total')
assert not hasattr(p, 'nope'), 'hasattr false for a missing attribute'

# === A class with no __init__ ===


class Empty:
    pass


e = Empty()
assert type(e) is Empty
assert type(e).__name__ == 'Empty'
assert isinstance(e, Empty)

# === A class whose only members are methods ===


class Counter:
    def __init__(self) -> None:
        self.n = 0

    def inc(self) -> None:
        self.n += 1

    def get(self) -> int:
        return self.n


c = Counter()
c.inc()
c.inc()
c.inc()
assert c.get() == 3

# === Error cases ===
try:
    e.nope
    assert False, 'expected AttributeError for missing attribute'
except AttributeError as exc:
    assert str(exc) == "'Empty' object has no attribute 'nope'"

try:
    e.nope()
    assert False, 'expected AttributeError for missing method'
except AttributeError as exc:
    assert str(exc) == "'Empty' object has no attribute 'nope'"

try:
    Empty(1)
    assert False, 'expected TypeError when passing args to a class with no __init__'
except TypeError as exc:
    assert str(exc) == 'Empty() takes no arguments'

# === Exception raised inside __init__ propagates (and the half-built instance
# is cleaned up — checked under memory-model-checks) ===


class Boom:
    def __init__(self, x: int) -> None:
        self.x = x
        raise ValueError('boom')


try:
    Boom(1)
    assert False, 'expected ValueError from __init__'
except ValueError as exc:
    assert str(exc) == 'boom'

# === Reference cycles between instances are reclaimable (exercises GC tracing
# of Instance children) ===


class Link:
    def __init__(self) -> None:
        self.other = None


n1 = Link()
n2 = Link()
n1.other = n2
n2.other = n1  # cycle: n1 <-> n2
assert n1.other.other is n1

# Self reference.
n1.other = n1
assert n1.other is n1

# === Bound methods hash by identity: the same bound-method object works as a
# dict key (CPython hashes by (instance, func); see limitations/classes.md) ===

m = c.inc
d = {m: 'inc'}
assert d[m] == 'inc'
assert hash(m) == hash(m)
s = {m, m}
assert len(s) == 1

# === A name bound more than once in the class body: last binding wins, the
# replaced (heap-allocated) value is released ===


class Rebound:
    items = [1]
    items = [2, 3]


assert Rebound.items == [2, 3]

# === Exotic __init__ members: CPython's type.__call__ looks __init__ up with
# descriptor binding, so only plain functions bind the new instance as self;
# anything else is called with the constructor args unchanged and must still
# return None ===


class _Helper:
    def __init__(self, x=None):
        self.x = x


class InitIsClass:
    __init__ = _Helper


try:
    InitIsClass()
    assert False, 'expected InitIsClass() to raise'
except TypeError as e:
    assert str(e) == "__init__() should return None, not '_Helper'"


class InitNotCallable:
    __init__ = 42


try:
    InitNotCallable()
    assert False, 'expected InitNotCallable() to raise'
except TypeError as e:
    assert str(e) == "'int' object is not callable"


class InitReturnsValue:
    def __init__(self):
        return 'nope'


try:
    InitReturnsValue()
    assert False, 'expected InitReturnsValue() to raise'
except TypeError as e:
    assert str(e) == "__init__() should return None, not 'str'"


class InitAsync:
    async def __init__(self):
        pass


try:
    InitAsync()
    assert False, 'expected InitAsync() to raise'
except TypeError as e:
    assert str(e) == "__init__() should return None, not 'coroutine'"


# A builtin __init__ that returns None: the instance is constructed and the
# builtin receives only the constructor args (no self).
class InitBuiltin:
    __init__ = print


ib = InitBuiltin('init-builtin-arg')
assert type(ib) is InitBuiltin


# A bound method used as __init__ keeps its own receiver; the new instance is
# not prepended.
class Recorder:
    def __init__(self):
        self.calls = []

    def record(self, *args):
        self.calls.append(args)


rec = Recorder()


class InitBoundMethod:
    __init__ = rec.record


ibm = InitBoundMethod(1, 2)
assert type(ibm) is InitBoundMethod
assert rec.calls == [(1, 2)]


# === `...` as the class body (common stub idiom) ===
class Stub: ...


s = Stub()
assert type(s) is Stub
s.x = 1
assert s.x == 1
