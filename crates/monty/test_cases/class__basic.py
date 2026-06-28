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
assert p.x == 3, 'instance attribute x set by __init__'
assert p.y == 4, 'instance attribute y set by __init__'

# === Instance methods ===
assert p.total() == 7, 'method reads instance attributes'
assert p.scaled() == 14, 'method uses default argument'
assert p.scaled(3) == 21, 'method positional argument overrides default'
assert p.scaled(factor=10) == 70, 'method keyword argument'

# === Mutating attributes via a method ===
p.move(1, 1)
assert p.x == 4, 'method mutated x via self.x += dx'
assert p.y == 5, 'method mutated y via self.y += dy'
assert p.total() == 9, 'method sees mutated attributes'

# === Mutating attributes directly ===
p.x = 100
assert p.x == 100, 'attribute set directly'
assert p.total() == 105, 'method sees directly-set attribute'

# === Setting a new attribute not declared in __init__ ===
p.z = 7
assert p.z == 7, 'new attribute can be added to an instance'

# === Class variables ===
assert Point.origin_count == 0, 'class variable read on the class'
assert p.origin_count == 0, 'class variable read through an instance'
q = Point(1, 1)
assert q.origin_count == 0, 'class variable shared across instances'

# === Independent instances ===
assert p.x == 100 and q.x == 1, 'instances have independent attributes'

# === type() returns the class object ===
assert type(p) is Point, 'type(instance) is the class object'
assert type(p) is type(q), 'two instances of the same class share their type'
assert type(p).__name__ == 'Point', 'class __name__'

# === isinstance ===
assert isinstance(p, Point), 'isinstance true for the right class'
assert isinstance(p, (int, Point)), 'isinstance with a tuple of classes'
assert not isinstance(5, Point), 'isinstance false for a non-instance'


class Other:
    def __init__(self) -> None:
        self.v = 1


o = Other()
assert not isinstance(o, Point), 'isinstance false for a different class'
assert type(o) is not Point, 'different classes are distinct'

# === Identity equality (no user __eq__) ===
assert p == p, 'an instance equals itself'
assert p != q, 'distinct instances are not equal'
assert (p == q) is False, 'distinct instances compare unequal'

# === Instances are always truthy ===
assert bool(p) is True, 'instances are truthy'
if q:
    pass
else:
    assert False, 'instance should be truthy in a condition'

# === Bound methods ===
m = p.total
assert m() == 105, 'bound method captures self and is callable'
move = p.move
move(10, 10)
assert p.x == 110 and p.y == 15, 'bound method with arguments mutates the instance'

# === getattr() / hasattr() ===
assert getattr(p, 'x') == 110, 'getattr reads an instance attribute'
assert getattr(p, 'total')() == 125, 'getattr returns a callable bound method'
assert getattr(p, 'nope', 'default') == 'default', 'getattr returns default for missing attribute'
assert hasattr(p, 'x'), 'hasattr true for an existing attribute'
assert hasattr(p, 'total'), 'hasattr true for a method'
assert not hasattr(p, 'nope'), 'hasattr false for a missing attribute'

# === A class with no __init__ ===


class Empty:
    pass


e = Empty()
assert type(e) is Empty, 'no-init class still constructs'
assert type(e).__name__ == 'Empty', 'no-init class name'
assert isinstance(e, Empty), 'isinstance on no-init class'

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
assert c.get() == 3, 'method-only class accumulates state'

# === Error cases ===
try:
    e.nope
    assert False, 'expected AttributeError for missing attribute'
except AttributeError as exc:
    assert str(exc) == "'Empty' object has no attribute 'nope'", 'missing attribute message'

try:
    e.nope()
    assert False, 'expected AttributeError for missing method'
except AttributeError as exc:
    assert str(exc) == "'Empty' object has no attribute 'nope'", 'missing method message'

try:
    Empty(1)
    assert False, 'expected TypeError when passing args to a class with no __init__'
except TypeError as exc:
    assert str(exc) == 'Empty() takes no arguments', 'no-init takes no arguments message'

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
    assert str(exc) == 'boom', '__init__ exception propagates'

# === Reference cycles between instances are reclaimable (exercises GC tracing
# of Instance children) ===


class Link:
    def __init__(self) -> None:
        self.other = None


n1 = Link()
n2 = Link()
n1.other = n2
n2.other = n1  # cycle: n1 <-> n2
assert n1.other.other is n1, 'cycle navigable through attributes'

# Self reference.
n1.other = n1
assert n1.other is n1, 'instance can reference itself'
