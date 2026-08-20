from dataclasses import dataclass


# === object.__setattr__ writes an instance attribute ===
class Point:
    def __init__(self, x):
        self.x = x


@dataclass(frozen=True)
class FrozenPoint:
    x: int


def make_frozen_point():
    return FrozenPoint(1)


p = Point(1)
object.__setattr__(p, 'x', 5)
assert p.x == 5

# === It creates attributes the class never declared ===
object.__setattr__(p, 'y', 'new')
assert p.y == 'new'

# === Values of any type, including references ===
object.__setattr__(p, 'items', [1, 2, 3])
assert p.items == [1, 2, 3]
object.__setattr__(p, 'items', None)
assert p.items is None

# === Repeated writes replace, they do not accumulate ===
for i in range(5):
    object.__setattr__(p, 'x', i)
assert p.x == 4

# === Bound as a value, then called ===
setter = object.__setattr__
setter(p, 'x', 99)
assert p.x == 99

# === `object` itself reprs as a class ===
assert repr(object) == "<class 'object'>"

# === A non-string name is rejected ===
try:
    object.__setattr__(p, 1, 'v')
    assert False, 'expected TypeError for a non-string attribute name'
except TypeError as exc:
    assert str(exc) == "attribute name must be string, not 'int'"

# === A value with no instance __dict__ is rejected ===
try:
    object.__setattr__(1, 'x', 2)
    assert False, 'expected AttributeError for an int'
except AttributeError as exc:
    assert str(exc) == "'int' object has no attribute 'x' and no __dict__ for setting new attributes"

try:
    object.__setattr__('s', 'x', 2)
    assert False, 'expected AttributeError for a str'
except AttributeError as exc:
    assert str(exc) == "'str' object has no attribute 'x' and no __dict__ for setting new attributes"

# === Wrong arity — CPython counts `obj` as the bound receiver, not an argument ===
try:
    object.__setattr__(p, 'x')
    assert False, 'expected TypeError for two arguments'
except TypeError as exc:
    assert str(exc) == '__setattr__ expected 2 arguments, got 1'

try:
    object.__setattr__(p, 'x', 1, 2)
    assert False, 'expected TypeError for four arguments'
except TypeError as exc:
    assert str(exc) == '__setattr__ expected 2 arguments, got 3'

# === No receiver at all fails in the descriptor, before the arity check ===
try:
    object.__setattr__()
    assert False, 'expected TypeError for no arguments'
except TypeError as exc:
    assert str(exc) == "descriptor '__setattr__' of 'object' object needs an argument"

# === Keywords are rejected by the slot wrapper ===
try:
    object.__setattr__(p, 'x', value=1)
    assert False, 'expected TypeError for a keyword argument'
except TypeError as exc:
    assert str(exc) == 'wrapper __setattr__() takes no keyword arguments'

# === It bypasses a frozen dataclass, the escape hatch CPython's own
# generated `__init__` uses ===
frozen = make_frozen_point()
try:
    frozen.x = 5
    assert False, 'expected FrozenInstanceError for a direct assignment'
except AttributeError as exc:
    assert str(exc) == "cannot assign to field 'x'"
object.__setattr__(frozen, 'x', 5)
assert frozen.x == 5
object.__setattr__(frozen, 'z', 'new')
assert frozen.z == 'new'

# === `__name__` resolves; the rest of object's members do not (see
# limitations/classes.md — CPython has them, so only `__name__` is testable here) ===
assert object.__name__ == 'object'

# === `object` is the universal base, so isinstance always says yes ===
assert isinstance(p, object)
assert isinstance(5, object)
assert isinstance('s', object)
assert isinstance(None, object)
assert isinstance(Point, object)
assert isinstance(5, (str, object))
# and it does not make every isinstance check true
assert not isinstance(5, str), 'object must not short-circuit other checks'
