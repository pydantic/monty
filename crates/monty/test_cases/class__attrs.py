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
assert Point.count == 0, 'class attr created by assignment'
Point.count += 1
assert Point.count == 1, 'augmented assignment on class attr'
assert p.count == 1, 'new class attr visible through existing instance'
assert q.count == 1, 'new class attr visible through second instance'

Point.kind = 'dot'
assert Point.kind == 'dot', 'class attr rebound'
assert q.kind == 'dot', 'rebound class attr visible through instance'

# a function assigned to the class becomes a method (binds self)
Point.triple = lambda self: self.x * 3
assert p.triple() == 9, 'function assigned to class binds self'

# setattr builtin on a class object
setattr(Point, 'via_setattr', 42)
assert Point.via_setattr == 42, 'setattr on class object'
assert p.via_setattr == 42, 'setattr class attr visible through instance'

# === setattr/getattr on instances ===
setattr(p, 'x', 10)
assert p.x == 10, 'setattr rebinds instance attr'
assert getattr(p, 'x') == 10, 'getattr reads instance attr'
setattr(p, 'fresh', 'n')
assert p.fresh == 'n', 'setattr creates new instance attr'
assert not hasattr(q, 'fresh'), 'instance attr not shared with other instances'

# === unbound method access with explicit self ===
assert Point.double(p) == 20, 'unbound call with explicit self'
assert Point.double(q) == 8, 'unbound call with other instance'

# === instance attr shadowing a class variable ===
q.kind = 'special'
assert q.kind == 'special', 'instance attr shadows class var'
assert Point.kind == 'dot', 'class var unchanged by instance shadow'
assert p.kind == 'dot', 'other instances still see the class var'

# === method reassigned on an instance ===
# an instance-dict function is NOT bound (same as CPython): called with no args
p.double = lambda: 'shadowed'
assert p.double() == 'shadowed', 'instance attr shadows method'
assert q.double() == 8, 'other instances keep the class method'

# === mutable class variable mutated through an instance ===
# `q.lst += [9]` mutates the shared list in place AND creates an instance
# attr referencing the same list (CPython augmented-assignment semantics)
q.lst += [9]
assert Point.lst == [1, 2, 9], 'augmented assignment mutated the shared list'
assert p.lst == [1, 2, 9], 'other instance sees the mutation'
assert q.lst is Point.lst, 'instance attr references the same list'
Point.lst.append(7)
assert q.lst == [1, 2, 9, 7], 'direct class-list mutation visible everywhere'

# === obj.__class__ ===
assert p.__class__ is Point, '__class__ returns the class object'
assert Point(0).__class__ is Point, '__class__ on a fresh instance'
assert p.__class__.__name__ == 'Point', '__class__.__name__ chains'
assert type(p) is p.__class__, 'type(obj) and obj.__class__ agree'
# calling `obj.__class__(...)` constructs a new instance, both when accessed as
# a value first and when called directly (the two must be consistent)
cls = p.__class__
assert cls(5).x == 5, 'obj.__class__ is callable to construct a new instance'
assert p.__class__(6).x == 6, 'obj.__class__(...) direct call constructs an instance'
assert p.__class__(7).__class__ is Point, 'instance from __class__ call has the right class'


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


assert Documented.__doc__ == 'the docs', 'class docstring stored in __doc__'
assert Documented().__doc__ == 'the docs', 'instances read __doc__ from the class'
assert Undocumented.__doc__ is None, '__doc__ is None without a docstring'
assert Undocumented().__doc__ is None, 'instance __doc__ is None without a docstring'
assert ExplicitDoc.__doc__ == 'explicit', 'explicit __doc__ member'
assert OverriddenDoc.__doc__ == 'overridden', 'explicit __doc__ overrides the docstring'


class DocRead:
    "doc"

    y = __doc__


assert DocRead.y == 'doc', 'class body reads its own __doc__ binding'


# === __name__ ===
class NamedBar:
    __name__ = 'bar'


assert NamedBar.__name__ == 'NamedBar', 'type.__name__ descriptor shadows the member'
assert NamedBar().__name__ == 'bar', 'instances see the __name__ member'

# `__name__` is always a plain str, so calling it raises the same TypeError
# CPython gives for calling any non-callable str, not an AttributeError.
try:
    NamedBar.__name__()
    assert False, 'expected calling __name__ to fail'
except TypeError as exc:
    assert str(exc) == "'str' object is not callable", 'calling __name__ reports the str-not-callable TypeError'

# === type-object attribute errors ===
try:
    Point.nope
    assert False, 'expected attribute get to fail'
except AttributeError as exc:
    assert str(exc) == "type object 'Point' has no attribute 'nope'", 'class attr get error'
try:
    Point.nope(1)
    assert False, 'expected attribute call to fail'
except AttributeError as exc:
    assert str(exc) == "type object 'Point' has no attribute 'nope'", 'class attr call error'


# === keyword args to a class without __init__ ===
class Empty:
    pass


try:
    Empty(k=1)
    assert False, 'expected keyword construction to fail'
except TypeError as exc:
    assert str(exc) == 'Empty() takes no arguments', 'keyword args to no-__init__ class'


# === walrus in a lambda body inside a class (binds in the lambda scope) ===
class WithLambda:
    f = lambda self: (z := 3) + 1


assert WithLambda().f() == 4, 'walrus in lambda body inside class body'

# === reference cycle through the class namespace ===
Point.self_ref = Point
assert Point.self_ref is Point, 'class can reference itself via a class attr'
Point.self_ref = None
assert Point.self_ref is None, 'cycle broken by rebinding'
