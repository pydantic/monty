# `InitVar[...]` pseudo-fields: `__init__` parameters handed to `__post_init__`
# and never stored. Detected textually, like `ClassVar` (see
# limitations/dataclasses.md).
from dataclasses import InitVar, dataclass, field, fields


# === An InitVar is a parameter, passed to __post_init__, and not stored ===
@dataclass
class Scaled:
    x: int
    factor: InitVar[int]

    def __post_init__(self, factor):
        self.x *= factor


s = Scaled(2, 5)
assert s.x == 10
assert not hasattr(s, 'factor')
assert repr(s) == 'Scaled(x=10)'
assert Scaled(2, 5) == Scaled(10, 1)
assert Scaled(2, factor=3).x == 6

# === It stays in __dataclass_fields__, marked as an InitVar ===
assert list(Scaled.__dataclass_fields__) == ['x', 'factor']
assert repr(Scaled.__dataclass_fields__['factor']._field_type) == '_FIELD_INITVAR'
assert repr(Scaled.__dataclass_fields__['x']._field_type) == '_FIELD'
assert [f.name for f in fields(Scaled)] == ['x']


# === Several InitVars reach __post_init__ positionally, in field order ===
@dataclass
class Several:
    a: InitVar[int]
    b: int
    c: InitVar[str] = 'c'
    seen: list[object] = field(default_factory=list)

    def __post_init__(self, a, c):
        self.seen.append((a, c))


assert Several(1, 2).seen == [(1, 'c')]
assert Several(1, 2, 'z').seen == [(1, 'z')]
assert Several(b=2, a=1, c='k').seen == [(1, 'k')]
assert repr(Several(1, 2)) == "Several(b=2, seen=[(1, 'c')])"


# === Without __post_init__ the InitVars are accepted and dropped ===
@dataclass
class Dropped:
    x: int
    unused: InitVar[int] = 0


assert Dropped(1, 2) == Dropped(1)
# Not stored: the attribute is the class's default, as CPython leaves it.
assert Dropped(1, 2).unused == 0
assert Dropped.unused == 0


# === An InitVar may have a mutable default, but not a factory ===
@dataclass
class MutableDefault:
    items: InitVar[list[int]] = []

    def __post_init__(self, items):
        self.count = len(items)


assert MutableDefault().count == 0
assert MutableDefault([1, 2]).count == 2

try:

    @dataclass
    class FactoryInitVar:
        x: InitVar[list[int]] = field(default_factory=list)

    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == 'field x cannot have a default factory'


# === An InitVar follows the non-default ordering rule like any parameter ===
try:

    @dataclass
    class Ordering:
        a: int = 1
        b: InitVar[int]

    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "non-default argument 'b' follows default argument 'a'"


# === It is a missing argument like any other ===
try:
    Scaled(1)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "Scaled.__init__() missing 1 required positional argument: 'factor'"


# === A keyword-only InitVar ===
@dataclass
class KwInitVar:
    x: int
    scale: InitVar[int] = field(default=1, kw_only=True)

    def __post_init__(self, scale):
        self.x *= scale


assert KwInitVar(2, scale=3).x == 6
assert KwInitVar(2).x == 2
assert KwInitVar.__match_args__ == ('x',)


# === InitVars count in __match_args__, as positional parameters ===
assert Scaled.__match_args__ == ('x', 'factor')
assert Several.__match_args__ == ('a', 'b', 'c', 'seen')


# === An InitVar is left out of the generated eq, hash and ordering ===
@dataclass(frozen=True, order=True)
class Frozen:
    x: int
    tag: InitVar[str] = ''

    def __post_init__(self, tag):
        pass


assert Frozen(1, 'a') == Frozen(1, 'b')
assert hash(Frozen(1, 'a')) == hash((1,))
assert Frozen(1, 'z') < Frozen(2, 'a')
