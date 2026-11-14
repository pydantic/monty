# The `@dataclass(...)` keyword form, for the options Monty implements:
# `eq` and `frozen`. All behaviour here matches CPython.
from dataclasses import FrozenInstanceError, dataclass


# === The empty call form behaves like the bare decorator ===
@dataclass()
class Called:
    x: int


assert repr(Called(1)) == 'Called(x=1)'
assert Called(1) == Called(1)


# === Explicitly spelling the defaults matches the bare decorator ===
@dataclass(eq=True, frozen=False)
class Defaults:
    x: int


assert Defaults(1) == Defaults(1)
try:
    hash(Defaults(1))
    assert False, 'a default dataclass is unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'Defaults'"


# === frozen=True: construction, repr and equality are unchanged ===
@dataclass(frozen=True)
class Point:
    x: int
    y: int


p = Point(1, 2)
assert repr(p) == 'Point(x=1, y=2)'
assert Point(1, 2) == Point(1, 2)
assert Point(1, 2) != Point(1, 3)

# === frozen=True rejects attribute assignment ===
try:
    p.x = 9
    assert False, 'expected a frozen assignment to raise'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'x'"
assert p.x == 1

# Assigning an attribute that is not a declared field is refused too.
try:
    p.extra = 1
    assert False, 'expected a frozen assignment to raise'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'extra'"

# === frozen=True with the default eq=True is hashable, by field values ===
assert hash(Point(1, 2)) == hash(Point(1, 2))
assert hash(Point(1, 2)) == hash((1, 2))
assert len({Point(1, 2), Point(1, 2)}) == 1
assert len({Point(1, 2), Point(3, 4)}) == 2
assert {Point(1, 2): 'a'}[Point(1, 2)] == 'a'


# === eq=False falls back to identity, and stays hashable ===
@dataclass(eq=False)
class NoEq:
    x: int


a = NoEq(1)
assert a == a
assert not (NoEq(1) == NoEq(1)), 'distinct instances are unequal without eq'
assert isinstance(hash(a), int), 'eq=False leaves the class hashable'
assert repr(NoEq(1)) == 'NoEq(x=1)'


# === eq=False with frozen=True hashes by identity, not by fields ===
@dataclass(eq=False, frozen=True)
class NoEqFrozen:
    x: int


b = NoEqFrozen(1)
assert isinstance(hash(b), int)
assert hash(b) == hash(b)
try:
    b.x = 2
    assert False, 'expected a frozen assignment to raise'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'x'"


# === A generated hash outranks the class body's own __eq__/__hash__ ===
# CPython writes the hash after the body has run, and treats a `__hash__ = None`
# sitting beside an `__eq__` as the opt-out `type` inserted rather than one the
# author wrote — so `eq=True, frozen=True` hashes by fields through both.
@dataclass(frozen=True)
class BodyEq:
    x: int

    def __eq__(self, other):
        return True


assert hash(BodyEq(1)) == hash((1,))


@dataclass(frozen=True)
class BodyEqNoneHash:
    x: int

    def __eq__(self, other):
        return True

    __hash__ = None


assert hash(BodyEqNoneHash(1)) == hash((1,))


# A `__hash__ = None` with no `__eq__` beside it is deliberate, and survives.
@dataclass(frozen=True)
class NoneHash:
    x: int

    __hash__ = None


try:
    hash(NoneHash(1))
    assert False, 'expected an explicit __hash__ = None to stay unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'NoneHash'"


# So does a real one, which no decoration overwrites.
@dataclass(frozen=True)
class BodyHash:
    x: int

    def __hash__(self):
        return 7


assert hash(BodyHash(1)) == 7


# === eq=False generates no hash, so the body's __eq__ opt-out stands ===
@dataclass(eq=False, frozen=True)
class NoEqBodyEq:
    x: int

    def __eq__(self, other):
        return True


try:
    hash(NoEqBodyEq(1))
    assert False, 'expected a body __eq__ to leave the class unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'NoEqBodyEq'"


@dataclass(eq=False)
class NoEqBodyEqMutable:
    x: int

    def __eq__(self, other):
        return True


try:
    hash(NoEqBodyEqMutable(1))
    assert False, 'expected a body __eq__ to leave the class unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'NoEqBodyEqMutable'"


# === An unknown keyword is rejected ===
def unknown_keyword():
    @dataclass(bogus=True)
    class Bogus:
        x: int


try:
    unknown_keyword()
    assert False, 'expected an unknown keyword to raise'
except TypeError as e:
    assert str(e) == "dataclass() got an unexpected keyword argument 'bogus'"


# === The class may be passed positionally alongside the options ===
class Manual:
    __annotations__ = {'x': 'int'}


Applied = dataclass(Manual, frozen=True)
assert Applied is Manual
m = Applied(5)
assert m.x == 5
try:
    m.x = 6
    assert False, 'expected a frozen assignment to raise'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'x'"


# === The options compose with the cycle guard and uninitialized-field rules ===
@dataclass(frozen=True)
class Leaf:
    v: int


@dataclass
class Holder:
    a: object
    b: object


h = Holder(Leaf(1), None)
h.b = h
assert repr(h) == 'Holder(a=Leaf(v=1), b=...)'


# `eq=False` never reads the fields, so an uninitialized one is only felt by repr.
@dataclass(eq=False)
class NoEqPartial:
    a: int
    b: int

    def __init__(self, a: int) -> None:
        self.a = a


assert not (NoEqPartial(1) == NoEqPartial(1)), 'eq=False compares by identity, never touching fields'
try:
    repr(NoEqPartial(1))
    assert False, 'expected repr to read the uninitialized field'
except AttributeError as e:
    assert str(e) == "'NoEqPartial' object has no attribute 'b'"


# === Nested frozen dataclasses stay equal and hashable ===
@dataclass(frozen=True)
class Outer:
    i: Leaf


assert Outer(Leaf(1)) == Outer(Leaf(1))
assert hash(Outer(Leaf(1))) == hash(Outer(Leaf(1)))
assert len({Outer(Leaf(1)), Outer(Leaf(1))}) == 1


# === __dataclass_params__ records the options on the class ===
# The decorator writes it beside `__dataclass_fields__`, as CPython does; the
# eight options Monty refuses unless left at their default read as constants.
params = Point.__dataclass_params__
assert type(params).__name__ == '_DataclassParams'
assert params.frozen is True
assert params.eq is True
assert params.init is True
assert params.repr is True
assert params.match_args is True
assert params.order is False
assert params.unsafe_hash is False
assert params.kw_only is False
assert params.slots is False
assert params.weakref_slot is False
assert repr(params) == (
    '_DataclassParams(init=True,repr=True,eq=True,order=False,unsafe_hash=False,'
    'frozen=True,match_args=True,kw_only=False,slots=False,weakref_slot=False)'
)

# `eq=False` is recorded too, and instances read it through their class.
assert NoEqPartial.__dataclass_params__.eq is False
assert NoEqPartial.__dataclass_params__.frozen is False
assert NoEqPartial(1).__dataclass_params__ is NoEqPartial.__dataclass_params__

try:
    params.nope
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'_DataclassParams' object has no attribute 'nope'"


# === The params report the options, they do not carry them ===
# The class acts on what it was decorated with, so lending it another class's
# params changes what you read back and nothing else.
@dataclass
class Borrower:
    x: int


Borrower.__dataclass_params__ = Point.__dataclass_params__
assert Borrower.__dataclass_params__.frozen is True
b = Borrower(1)
b.x = 2
assert b.x == 2, 'the borrowed params must not freeze the class'
try:
    hash(b)
    assert False, 'the borrowed params must not make the class hashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'Borrower'"

# A plain class has no params, just as it has no fields.


class PlainOptions:
    pass


assert not hasattr(PlainOptions, '__dataclass_params__')


# === dataclass(...) is a value, so the decorator can be stored and reused ===
frozen = dataclass(frozen=True)


@frozen
class First:
    x: int


@frozen
class Second:
    y: int


assert First(1).x == 1
assert Second(2).y == 2


# The stored decorator is CPython's `def wrap(cls)`, so the class binds by
# keyword too, and its arity errors name that closure rather than `dataclass`.
class ByKeyword:
    x: int


assert frozen(cls=ByKeyword) is ByKeyword
assert ByKeyword.__dataclass_params__.frozen is True

try:
    frozen()
    assert False, 'expected a missing class to raise'
except TypeError as e:
    assert str(e) == "dataclass.<locals>.wrap() missing 1 required positional argument: 'cls'"

try:
    frozen(First, Second)
    assert False, 'expected a second positional argument to raise'
except TypeError as e:
    assert str(e) == 'dataclass.<locals>.wrap() takes 1 positional argument but 2 were given'

try:
    frozen(cls=First, nope=1)
    assert False, 'expected an unknown keyword to raise'
except TypeError as e:
    assert str(e) == "dataclass.<locals>.wrap() got an unexpected keyword argument 'nope'"

assert First.__dataclass_params__.frozen is True
assert Second.__dataclass_params__.frozen is True
try:
    Second(2).y = 3
    assert False, 'expected a frozen assignment to raise'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'y'"


# === Options are read for truthiness, as CPython's pure-Python decorator does ===
@dataclass(frozen=1, eq='yes')
class Truthy:
    x: int


assert Truthy(1) == Truthy(1)
assert hash(Truthy(1)) == hash((1,))
try:
    Truthy(1).x = 2
    assert False, 'expected a frozen assignment to raise'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'x'"


# === A frozen dataclass holding an unhashable field reports that field's type ===
@dataclass(frozen=True)
class Boxed:
    v: object


assert hash(Boxed(1)) == hash((1,))
try:
    hash(Boxed([1, 2]))
    assert False, 'expected an unhashable field to raise'
except TypeError as e:
    assert str(e) == "unhashable type: 'list'"


# === Re-decorating replaces both the fields and the options ===
@dataclass
class Rebound:
    x: int


try:
    hash(Rebound(1))
    assert False, 'a default dataclass is unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'Rebound'"

Rebound = dataclass(frozen=True)(Rebound)
assert Rebound.__dataclass_params__.frozen is True
assert list(Rebound.__dataclass_fields__) == ['x']
