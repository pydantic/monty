# /// monty
# call-external = true
# ///
# The `dataclasses` module functions: `fields`, `asdict`, `astuple` and
# `replace`, plus the `__replace__` every dataclass gets and `copy.replace()`.
import copy
import time
from dataclasses import InitVar, asdict, astuple, dataclass, field, fields, replace


@dataclass
class Point:
    x: int
    y: int = 0


@dataclass
class Line:
    start: Point
    end: Point
    tags: list[object] = field(default_factory=list)


# === fields() takes a class or an instance, and returns the Field objects ===
assert [f.name for f in fields(Point)] == ['x', 'y']
assert fields(Point(1)) == fields(Point)
assert fields(Point)[0] is Point.__dataclass_fields__['x']
assert type(fields(Point)) is tuple
for bad in [Point(1).x, 5, int]:
    try:
        fields(bad)
        assert False, 'expected a TypeError'
    except TypeError as e:
        assert str(e) == 'must be called with a dataclass type or instance'

# === asdict() recurses into dataclasses, lists, tuples and dicts ===
line = Line(Point(1, 2), Point(3), [Point(4), (Point(5), 'x'), {'k': Point(6)}])
assert asdict(Point(1, 2)) == {'x': 1, 'y': 2}
assert asdict(line) == {
    'start': {'x': 1, 'y': 2},
    'end': {'x': 3, 'y': 0},
    'tags': [{'x': 4, 'y': 0}, ({'x': 5, 'y': 0}, 'x'), {'k': {'x': 6, 'y': 0}}],
}
# Containers are copies, not the instance's own.
tags = [1, [2]]
copied = asdict(Line(Point(0), Point(0), tags))['tags']
assert copied == tags
assert copied is not tags
assert copied[1] is not tags[1]

# === astuple() is the same walk, building tuples ===
assert astuple(Point(1, 2)) == (1, 2)
assert astuple(line) == ((1, 2), (3, 0), [(4, 0), ((5, 0), 'x'), {'k': (6, 0)}])

# === A custom factory receives a list ===
assert asdict(Point(1, 2), dict_factory=list) == [('x', 1), ('y', 2)]
assert astuple(Point(1, 2), tuple_factory=list) == [1, 2]
assert asdict(Line(Point(1), Point(2)), dict_factory=lambda pairs: len(pairs)) == 3

# === Both refuse a class, or anything that is not a dataclass instance ===
for func, name in [(asdict, 'asdict'), (astuple, 'astuple')]:
    for bad in [Point, 5, [Point(1)]]:
        try:
            func(bad)
            assert False, 'expected a TypeError'
        except TypeError as e:
            assert str(e) == f'{name}() should be called on dataclass instances'


# === InitVars are not reported by fields() or asdict() ===
@dataclass
class WithInitVar:
    x: int
    scale: InitVar[int] = 1

    def __post_init__(self, scale):
        self.x *= scale


assert [f.name for f in fields(WithInitVar)] == ['x']
assert asdict(WithInitVar(2, 3)) == {'x': 6}


# === An init=False field without a value is an AttributeError ===
@dataclass
class Unset:
    x: int
    later: int = field(init=False)


try:
    asdict(Unset(1))
    assert False, 'expected an AttributeError'
except AttributeError as e:
    assert str(e) == "'Unset' object has no attribute 'later'"

# === replace() rebuilds the instance through its constructor ===
p = Point(1, 2)
q = replace(p, y=5)
assert q == Point(1, 5)
assert p == Point(1, 2)
assert replace(p) == p
assert replace(p) is not p


@dataclass(frozen=True)
class Frozen:
    a: int
    b: list[int] = field(default_factory=list)


f = Frozen(1)
g = replace(f, a=2)
assert g == Frozen(2, [])
# Carried-over fields are the same objects, not copies.
assert g.b is f.b

# === replace() runs __post_init__ and requires InitVars without a default ===
assert replace(WithInitVar(2, 3)).x == 6
assert replace(WithInitVar(2, 3), scale=2).x == 12


@dataclass
class NeedsInitVar:
    x: int
    scale: InitVar[int]

    def __post_init__(self, scale):
        self.x *= scale


try:
    replace(NeedsInitVar(1, 2))
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "InitVar 'scale' must be specified with replace()"
assert replace(NeedsInitVar(1, 2), scale=3).x == 6


# === replace() refuses init=False fields, and unknown names reach __init__ ===
@dataclass
class Computed:
    x: int
    double: int = field(init=False)

    def __post_init__(self):
        self.double = self.x * 2


assert replace(Computed(2), x=3).double == 6
try:
    replace(Computed(2), double=1)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == 'field double is declared with init=False, it cannot be specified with replace()'
try:
    replace(Point(1), z=1)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "Point.__init__() got an unexpected keyword argument 'z'"

# === replace() argument errors ===
try:
    replace(Point)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == 'replace() should be called on dataclass instances'
try:
    replace()
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "replace() missing 1 required positional argument: 'obj'"


# === Every dataclass gets __replace__, one shared function ===
assert Point(1, 2).__replace__(x=5) == Point(5, 2)
assert Point.__replace__(Point(1, 2), y=7) == Point(1, 7)
assert Point.__replace__ is Computed.__replace__
assert repr(Point.__replace__).startswith('<function _replace at 0x')
try:
    Point.__replace__(5)
    assert False, 'expected an AttributeError'
except AttributeError as e:
    assert str(e) == "'int' object has no attribute '__dataclass_fields__'"
try:
    Point.__replace__()
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "_replace() missing 1 required positional argument: 'self'"


# === A body __replace__ is kept ===
@dataclass
class OwnReplace:
    x: int

    def __replace__(self, **changes):
        return ('own', changes)


assert OwnReplace(1).__replace__(x=2) == ('own', {'x': 2})


# === copy.replace() calls the class's __replace__ ===
assert copy.replace(Point(1, 2), y=9) == Point(1, 9)
assert copy.replace(OwnReplace(1), x=3) == ('own', {'x': 3})
assert copy.replace(Computed(2), x=4).double == 8
try:
    copy.replace(Point(1), z=1)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "Point.__init__() got an unexpected keyword argument 'z'"
try:
    copy.replace(Point(1), 2)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == 'replace() takes 1 positional argument but 2 were given'


# === replace() lets the class's own __init__ call the host ===
@dataclass(init=False)
class Sleepy:
    a: int

    def __init__(self, a):
        time.sleep(0)
        self.a = a


assert replace(Sleepy(1), a=2).a == 2
assert copy.replace(Sleepy(1), a=3).a == 3
assert Sleepy(1).__replace__(a=4).a == 4


class NoReplace:
    pass


class NoneReplace:
    __replace__ = None


for unsupported, type_name in [
    (5, 'int'),
    ([1], 'list'),
    (None, 'NoneType'),
    (NoReplace(), 'NoReplace'),
    (NoneReplace(), 'NoneReplace'),
]:
    try:
        copy.replace(unsupported)
        assert False, 'expected a TypeError'
    except TypeError as e:
        assert str(e) == f'replace() does not support {type_name} objects'
