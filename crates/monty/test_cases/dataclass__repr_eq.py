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
assert repr(p) == 'Point(x=1, y=2)'
assert str(p) == 'Point(x=1, y=2)'
assert repr(Point(3, 4)) == 'Point(x=3, y=4)'


# === repr of non-int fields uses repr() of each value ===
@dataclass
class User:
    name: str
    active: bool


assert repr(User('alice', True)) == "User(name='alice', active=True)"


# === Nested repr (dataclass inside a list / inside a dataclass) ===
assert repr([Point(1, 2), Point(3, 4)]) == '[Point(x=1, y=2), Point(x=3, y=4)]'


@dataclass
class Wrap:
    p: Point


assert repr(Wrap(Point(1, 2))) == 'Wrap(p=Point(x=1, y=2))'


# === Equality: same class + equal fields ===
assert Point(1, 2) == Point(1, 2)
assert Point(1, 2) != Point(1, 3)
assert not (Point(1, 2) == Point(2, 1)), 'field order matters'


# === Equality across types / non-dataclasses is False ===
assert Point(1, 2) != Other(1, 2)
assert Point(1, 2) != (1, 2)
assert Point(1, 2) != {'x': 1, 'y': 2}


# === Equality composes: containers and nesting ===
assert Point(1, 2) in [Point(3, 4), Point(1, 2)]
assert Wrap(Point(1, 2)) == Wrap(Point(1, 2))
assert Wrap(Point(1, 2)) != Wrap(Point(1, 3))


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
assert repr(n) == 'Node(x=...)'


@dataclass
class Pair:
    a: object
    b: object


pair = Pair(1, None)
pair.b = pair
assert repr(pair) == 'Pair(a=1, b=...)'


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


# === A defaulted field left unassigned reads its class-level default ===
# The generated methods use ordinary attribute access, which falls back to the
# class namespace, so a custom `__init__` skipping a defaulted field is fine.
@dataclass
class Defaulted:
    a: int
    b: int = 2

    def __init__(self, a: int) -> None:
        self.a = a


assert repr(Defaulted(1)) == 'Defaulted(a=1, b=2)'
assert Defaulted(1) == Defaulted(1)
assert Defaulted(1) != Defaulted(3)

# Those reads see the *live* class attribute, where construction uses the
# default captured at decoration time (see refcount__dataclass_defaults.py).
Defaulted.b = 99
assert repr(Defaulted(1)) == 'Defaulted(a=1, b=99)'
assigned = Defaulted(1)
assigned.b = 2
assert assigned != Defaulted(1)


# === A function-valued default binds as a method, exactly as `self.x` does ===
# Reading it off the class goes through the descriptor protocol, so each
# instance sees its *own* bound method and two of them compare unequal.
def on_event(self: object) -> int:
    return 1


@dataclass
class Hooked:
    a: int
    hook: object = on_event

    def __init__(self, a: int) -> None:
        self.a = a


assert Hooked(1) != Hooked(1)
hooked = Hooked(1)
assert hooked == hooked
# Only the stability of the rendering, since the two engines spell a bound
# method differently (see limitations/dataclasses.md).
assert repr(Hooked(1)) == repr(Hooked(1))


# === Each field is read as it is written, not all up front ===
# CPython's generated `__repr__` is an f-string evaluated left to right, so a
# field whose `__repr__` mutates a later one shows the mutated value.
class Mutator:
    def __repr__(self) -> str:
        mutated.b = 'mutated'
        return 'M'


@dataclass
class Mutated:
    a: object
    b: object


mutated = Mutated(Mutator(), 'original')
assert repr(mutated) == "Mutated(a=M, b='mutated')"


# === An unset `__class__` field resolves to the class object ===
# `self.__class__` answers even with nothing in the instance dict or the class
# namespace, and the generated methods use ordinary attribute access.
@dataclass
class Classy:
    __class__: object

    def __init__(self) -> None:
        pass


assert repr(Classy()) == 'Classy(__class__=' + repr(Classy) + ')'
assert Classy() == Classy()
