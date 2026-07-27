# Native `@dataclass` value semantics: synthesized `__repr__` (and `__str__`),
# field-wise `__eq__`, and the default `eq=True, frozen=False` case being
# unhashable — all matching CPython.
from dataclasses import dataclass


@dataclass
class Point:
    x: int
    y: int


@dataclass
class Other:
    x: int
    y: int


# === repr / str ===
p = Point(1, 2)
assert repr(p) == 'Point(x=1, y=2)', 'synthesized repr'
assert str(p) == 'Point(x=1, y=2)', 'str falls back to repr'
assert repr(Point(3, 4)) == 'Point(x=3, y=4)', 'repr with other values'


# === repr of non-int fields uses repr() of each value ===
@dataclass
class User:
    name: str
    active: bool


assert repr(User('alice', True)) == "User(name='alice', active=True)", 'string field is quoted in repr'


# === Nested repr (dataclass inside a list / inside a dataclass) ===
assert repr([Point(1, 2), Point(3, 4)]) == '[Point(x=1, y=2), Point(x=3, y=4)]', 'repr nests through a list'


@dataclass
class Wrap:
    p: Point


assert repr(Wrap(Point(1, 2))) == 'Wrap(p=Point(x=1, y=2))', 'repr nests through a dataclass field'


# === Equality: same class + equal fields ===
assert Point(1, 2) == Point(1, 2), 'equal dataclasses compare equal'
assert Point(1, 2) != Point(1, 3), 'differing fields are not equal'
assert not (Point(1, 2) == Point(2, 1)), 'field order matters'


# === Equality across types / non-dataclasses is False ===
assert Point(1, 2) != Other(1, 2), 'different dataclass types are never equal'
assert Point(1, 2) != (1, 2), 'a dataclass is not equal to a tuple'
assert Point(1, 2) != {'x': 1, 'y': 2}, 'a dataclass is not equal to a dict'


# === Equality composes: containers and nesting ===
assert Point(1, 2) in [Point(3, 4), Point(1, 2)], 'equal dataclass found in a list'
assert Wrap(Point(1, 2)) == Wrap(Point(1, 2)), 'nested dataclass equality'
assert Wrap(Point(1, 2)) != Wrap(Point(1, 3)), 'nested dataclass inequality'


# === Default dataclass (eq=True, frozen=False) is unhashable ===
try:
    hash(Point(1, 2))
    assert False, 'expected unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'Point'", f'wrong message: {e!r}'


# `__hash__ = None` in the body is CPython's explicit unhashable opt-out, which
# is already what a default dataclass does — so it is honoured, not rejected
# (unlike a real `__hash__` implementation, see tests/dataclass_rejections.rs).
@dataclass
class OptedOut:
    x: int
    __hash__ = None


try:
    hash(OptedOut(1))
    assert False, 'expected unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'OptedOut'", f'wrong message: {e!r}'


# === A self-referential field renders `...`, not an infinite nesting ===
@dataclass
class Node:
    x: object


n = Node(None)
n.x = n
assert repr(n) == 'Node(x=...)', 'the cycle guard survives instance repr dispatch'


@dataclass
class Pair:
    a: object
    b: object


pair = Pair(1, None)
pair.b = pair
assert repr(pair) == 'Pair(a=1, b=...)', 'only the cycling field is elided'


# === A declared field left uninitialized raises, as the attribute access does ===
@dataclass
class Partial:
    a: int
    b: int

    def __init__(self, a: int) -> None:
        self.a = a


try:
    repr(Partial(1))
    assert False, 'expected an uninitialized field to raise'
except AttributeError as e:
    assert str(e) == "'Partial' object has no attribute 'b'", f'wrong message: {e!r}'

try:
    Partial(1) == Partial(1)
    assert False, 'expected an uninitialized field to raise'
except AttributeError as e:
    assert str(e) == "'Partial' object has no attribute 'b'", f'wrong message: {e!r}'


# A cycle reached through a container is elided at the container, and two
# dataclasses referring to each other nest exactly once before eliding.
boxed = Node(None)
boxed.x = [boxed]
assert repr(boxed) == 'Node(x=[...])'

left = Node(None)
right = Node(None)
left.x = right
right.x = left
assert repr(left) == 'Node(x=Node(x=...))'

# The comparison chain short-circuits, so an earlier unequal field is reported
# before the uninitialized one is ever read; and identity wins outright, since
# CPython's generated `__eq__` opens with `self is other`.
assert Partial(1) != Partial(2)
partial = Partial(1)
assert partial == partial
