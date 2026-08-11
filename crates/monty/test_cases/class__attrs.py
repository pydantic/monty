# === class attribute assignment ===
class Point:
    kind = 'point'
    lst = [1, 2]

    def __init__(self, x):
        self.x = x

    def double(self):
        return self.x * 2


p = Point(3)
q = Point(4)

Point.count = 0
assert Point.count == 0
Point.count += 1
assert Point.count == 1
assert p.count == 1
assert q.count == 1

Point.kind = 'dot'
assert Point.kind == 'dot'
assert q.kind == 'dot'

# a function assigned to the class becomes a method (binds self)
Point.triple = lambda self: self.x * 3
assert p.triple() == 9

# setattr builtin on a class object
setattr(Point, 'via_setattr', 42)
assert Point.via_setattr == 42
assert p.via_setattr == 42

# === setattr/getattr on instances ===
setattr(p, 'x', 10)
assert p.x == 10
assert getattr(p, 'x') == 10
setattr(p, 'fresh', 'n')
assert p.fresh == 'n'
assert not hasattr(q, 'fresh'), 'instance attr not shared with other instances'

# === unbound method access with explicit self ===
assert Point.double(p) == 20
assert Point.double(q) == 8

# === instance attr shadowing a class variable ===
q.kind = 'special'
assert q.kind == 'special'
assert Point.kind == 'dot'
assert p.kind == 'dot'

# === method reassigned on an instance ===
# an instance-dict function is NOT bound (same as CPython): called with no args
p.double = lambda: 'shadowed'
assert p.double() == 'shadowed'
assert q.double() == 8

# === mutable class variable mutated through an instance ===
# `q.lst += [9]` mutates the shared list in place AND creates an instance
# attr referencing the same list (CPython augmented-assignment semantics)
q.lst += [9]
assert Point.lst == [1, 2, 9]
assert p.lst == [1, 2, 9]
assert q.lst is Point.lst
Point.lst.append(7)
assert q.lst == [1, 2, 9, 7]

# === obj.__class__ ===
assert p.__class__ is Point
assert Point(0).__class__ is Point
assert p.__class__.__name__ == 'Point'
assert type(p) is p.__class__
# calling `obj.__class__(...)` constructs a new instance, both when accessed as
# a value first and when called directly (the two must be consistent)
cls = p.__class__
assert cls(5).x == 5
assert p.__class__(6).x == 6
assert p.__class__(7).__class__ is Point


# === __doc__ ===
class Documented:
    """the docs"""

    x = 1


class Undocumented:
    pass


class ExplicitDoc:
    __doc__ = 'explicit'


class OverriddenDoc:
    """original"""

    __doc__ = 'overridden'


assert Documented.__doc__ == 'the docs'
assert Documented().__doc__ == 'the docs'
assert Undocumented.__doc__ is None
assert Undocumented().__doc__ is None
assert ExplicitDoc.__doc__ == 'explicit'
assert OverriddenDoc.__doc__ == 'overridden'


class DocRead:
    "doc"

    y = __doc__


assert DocRead.y == 'doc'


# === __name__ ===
class NamedBar:
    __name__ = 'bar'


assert NamedBar.__name__ == 'NamedBar'
assert NamedBar().__name__ == 'bar'

# `__name__` is always a plain str, so calling it raises the same TypeError
# CPython gives for calling any non-callable str, not an AttributeError.
try:
    NamedBar.__name__()
    assert False, 'expected calling __name__ to fail'
except TypeError as exc:
    assert str(exc) == "'str' object is not callable"

# === type-object attribute errors ===
try:
    Point.nope
    assert False, 'expected attribute get to fail'
except AttributeError as exc:
    assert str(exc) == "type object 'Point' has no attribute 'nope'"
try:
    Point.nope(1)
    assert False, 'expected attribute call to fail'
except AttributeError as exc:
    assert str(exc) == "type object 'Point' has no attribute 'nope'"


# === keyword args to a class without __init__ ===
class Empty:
    pass


try:
    Empty(k=1)
    assert False, 'expected keyword construction to fail'
except TypeError as exc:
    assert str(exc) == 'Empty() takes no arguments'


# === walrus in a lambda body inside a class (binds in the lambda scope) ===
class WithLambda:
    f = lambda self: (z := 3) + 1


assert WithLambda().f() == 4

# === reference cycle through the class namespace ===
Point.self_ref = Point
assert Point.self_ref is Point
Point.self_ref = None
assert Point.self_ref is None
