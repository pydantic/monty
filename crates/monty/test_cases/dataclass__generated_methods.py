from dataclasses import FrozenInstanceError, dataclass, field, is_dataclass


@dataclass
class P:
    x: int
    y: int = 2


@dataclass(frozen=True)
class F:
    x: int


# === The generated dunders are functions on the class ===
assert type(P.__init__).__name__ == 'function'
assert type(P.__repr__).__name__ == 'function'
assert type(P.__eq__).__name__ == 'function'
assert type(F.__hash__).__name__ == 'function'
assert type(F.__setattr__).__name__ == 'function'
assert type(F.__delattr__).__name__ == 'function'
# `eq=True` without `frozen` makes instances unhashable by writing `None`.
assert P.__hash__ is None
assert P.__init__ is P.__init__

# === Calling them explicitly ===
p = P(1)
assert P.__repr__(p) == 'P(x=1, y=2)'
assert p.__repr__() == 'P(x=1, y=2)'
assert P.__eq__(p, P(1)) is True
assert P.__eq__(p, P(2)) is False
assert P.__eq__(p, p) is True
assert P.__eq__(p, 5) is NotImplemented
assert p.__eq__(P(1, 2)) is True
assert F.__hash__(F(1)) == hash(F(1))
assert F(1).__hash__() == hash((1,))
assert P.__init__(p, 7) is None
assert p == P(7, 2)
p.__init__(8, y=9)
assert p == P(8, 9)

f = F(1)
try:
    F.__setattr__(f, 'x', 2)
    assert False, 'expected FrozenInstanceError'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'x'"
try:
    F.__delattr__(f, 'x')
    assert False, 'expected FrozenInstanceError'
except FrozenInstanceError as e:
    assert str(e) == "cannot delete field 'x'"
try:
    setattr(f, 'other', 2)
    assert False, 'expected FrozenInstanceError'
except FrozenInstanceError as e:
    assert str(e) == "cannot assign to field 'other'"


# === Explicit calls bind like the generated `def` ===
assert P.__eq__(self=P(1), other=P(1)) is True
P.__init__(self=p, x=3)
assert p == P(3)
for call, message in [
    (lambda: P.__eq__(p), "P.__eq__() missing 1 required positional argument: 'other'"),
    (lambda: P.__repr__(), "P.__repr__() missing 1 required positional argument: 'self'"),
    (lambda: P.__init__(), "P.__init__() missing 2 required positional arguments: 'self' and 'x'"),
    (lambda: P.__eq__(p, p, p), 'P.__eq__() takes 2 positional arguments but 3 were given'),
    (lambda: P.__init__(p, 1, 2, 3), 'P.__init__() takes from 2 to 3 positional arguments but 4 were given'),
    (lambda: P.__init__(p, z=1), "P.__init__() got an unexpected keyword argument 'z'"),
    (lambda: P(1, x=2), "P.__init__() got multiple values for argument 'x'"),
    (lambda: P(1, self=2), "P.__init__() got multiple values for argument 'self'"),
    (lambda: F.__setattr__(f, 'x'), "F.__setattr__() missing 1 required positional argument: 'value'"),
]:
    try:
        call()
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == message


# === A field named `self` renames `__init__`'s own parameter ===
@dataclass
class S:
    self: int


s = S(self=1)
S.__init__(s, self=5)
assert s == S(5)
S.__init__(__dataclass_self__=s, self=6)
assert s.self == 6
try:
    S.__init__()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "S.__init__() missing 2 required positional arguments: '__dataclass_self__' and 'self'"


# === A recursive repr through an explicit call ===
@dataclass
class Node:
    child: object = None


n = Node()
n.child = n
assert repr(n) == 'Node(child=...)'
assert n.__repr__() == 'Node(child=...)'


# === The class body's own dunders are kept ===
@dataclass
class Body:
    x: int

    def __init__(self, x):
        self.x = x * 10

    def __repr__(self):
        return 'Body!'

    def __eq__(self, other):
        return 'eq!'


assert Body(1).x == 10
assert repr(Body(1)) == 'Body!'
assert (Body(1) == Body(2)) == 'eq!'
# The `__hash__ = None` the body `__eq__` implied is replaced by the generated one.
assert Body.__hash__ is None


@dataclass(frozen=True)
class OwnHash:
    x: int

    def __hash__(self):
        return 7


assert hash(OwnHash(1)) == 7


@dataclass(eq=False)
class NoEq:
    x: int


assert NoEq(1) != NoEq(1)
a = NoEq(1)
assert a == a
assert hash(a) == hash(a)

# === Rebinding the metadata after decoration changes nothing ===
# The methods were built from the decoration, as CPython's are.
P.__dataclass_params__ = F.__dataclass_params__
q = P(1)
q.x = 5
assert q == P(5)
try:
    hash(q)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unhashable type: 'P'"

P.__dataclass_fields__ = {}
assert is_dataclass(P)
assert P(3) == P(3, 2)
assert repr(P(3)) == 'P(x=3, y=2)'
P.__dataclass_fields__ = 5
assert is_dataclass(P)
assert P(4).x == 4


@dataclass
class Late:
    a: list[str] = field(default_factory=list)


def late_hook(self):
    self.a.append('hook')


Late.__post_init__ = late_hook
assert Late().a == []


@dataclass
class Hooked:
    a: int

    def __post_init__(self):
        self.b = self.a + 1


assert Hooked(1).b == 2


# A hook replaced after decoration is the one called: CPython looks it up per call.
def replaced_hook(self):
    self.b = 'replaced'


Hooked.__post_init__ = replaced_hook
assert Hooked(1).b == 'replaced'


# === Re-decorating keeps the methods the first decoration made ===
@dataclass
class Again:
    x: int


assert dataclass(frozen=True)(Again) is Again
try:
    Again(1)
    assert False, 'expected FrozenInstanceError'
except FrozenInstanceError as e:
    # The first `__init__` assigns through the frozen `__setattr__` added second.
    assert str(e) == "cannot assign to field 'x'"

try:
    dataclass(frozen=True)(F)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Cannot overwrite attribute __setattr__ in class F'

# === A frozen class cannot bring its own __setattr__ ===
try:

    @dataclass(frozen=True)
    class OwnSetattr:
        x: int

        def __setattr__(self, name, value):
            pass

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Cannot overwrite attribute __setattr__ in class OwnSetattr'
