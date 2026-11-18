import typing
from dataclasses import dataclass, field
from typing import Any, Optional, Union, get_type_hints

NoneType = type(None)


# The CPython harness runs this file in a bare dict, so a class's string forward
# references cannot see module globals there; those cases pass `globalns`.
@dataclass
class Point:
    x: int
    y: int


# === Fields ===
assert get_type_hints(Point) == {'x': int, 'y': int}
assert list(get_type_hints(Point)) == ['x', 'y']


@dataclass
class AllKinds:
    a: int
    b: str
    c: list[int]
    d: dict[str, Point]
    e: int | None
    f: Optional[str]
    g: Union[int, str]
    h: tuple[int, ...]
    i: Any
    j: typing.List
    k: None
    z: Point
    m: type[Point]


assert get_type_hints(AllKinds) == {
    'a': int,
    'b': str,
    'c': list[int],
    'd': dict[str, Point],
    'e': int | None,
    'f': str | None,
    'g': int | str,
    'h': tuple[int, ...],
    'i': Any,
    'j': typing.List,
    'k': NoneType,
    'z': Point,
    'm': type[Point],
}


# === Defaults and field() ===
@dataclass
class Defaults:
    required: int
    with_default: str = 'x'
    optional: Optional[int] = None
    factory: list[str] = field(default_factory=list)
    field_default: float = field(default=1.5)
    tags: dict[str, int] = field(default_factory=dict)


assert get_type_hints(Defaults) == {
    'required': int,
    'with_default': str,
    'optional': int | None,
    'factory': list[str],
    'field_default': float,
    'tags': dict[str, int],
}
d = Defaults(1)
assert d.factory == []
assert d.optional is None


# unannotated class attributes and methods are not fields or hints
@dataclass
class WithExtras:
    x: int
    counter = 0

    def total(self, extra: int) -> int:
        return self.x + extra


assert get_type_hints(WithExtras) == {'x': int}
assert get_type_hints(WithExtras.total) == {'extra': int, 'return': int}
assert get_type_hints(WithExtras(1).total) == {'extra': int, 'return': int}


# === Options ===
@dataclass(frozen=True)
class Frozen:
    a: int
    b: 'str'


assert get_type_hints(Frozen) == {'a': int, 'b': str}


@dataclass(eq=False)
class NoEq:
    a: list[int]


assert get_type_hints(NoEq) == {'a': list[int]}


@dataclass()
class EmptyCall:
    a: bytes


assert get_type_hints(EmptyCall) == {'a': bytes}


@dataclass
class Empty:
    pass


assert get_type_hints(Empty) == {}


# === __post_init__ ===
@dataclass
class PostInit:
    a: int
    doubled: int = 0

    def __post_init__(self) -> None:
        self.doubled = self.a * 2


assert get_type_hints(PostInit) == {'a': int, 'doubled': int}
assert get_type_hints(PostInit.__post_init__) == {'return': NoneType}
assert PostInit(2).doubled == 4


# === String annotations ===
@dataclass
class Strings:
    a: 'int'
    b: 'list[Point]'
    c: list['Point']
    d: 'Optional[Point]' = None
    e: Optional['Point'] = None


assert get_type_hints(Strings, {'Point': Point, 'Optional': Optional}) == {
    'a': int,
    'b': list[Point],
    'c': list[Point],
    'd': Point | None,
    'e': Point | None,
}


# === Self and mutual references ===
@dataclass
class TreeNode:
    value: int
    children: list['TreeNode'] = field(default_factory=list)
    parent: Optional['TreeNode'] = None


assert get_type_hints(TreeNode, {'TreeNode': TreeNode}) == {
    'value': int,
    'children': list[TreeNode],
    'parent': TreeNode | None,
}
root = TreeNode(1, [TreeNode(2)])
assert root.children[0].value == 2


@dataclass
class Author:
    name: str
    books: 'list[Book]'


@dataclass
class Book:
    title: str
    author: 'Author'


ns = {'Author': Author, 'Book': Book}
assert get_type_hints(Author, ns) == {'name': str, 'books': list[Book]}
assert get_type_hints(Book, ns) == {'title': str, 'author': Author}


# a plain annotation naming a later class evaluates when asked
@dataclass
class UsesLater:
    later: LaterData
    many: list[LaterData]


@dataclass
class LaterData:
    x: int


assert get_type_hints(UsesLater) == {'later': LaterData, 'many': list[LaterData]}


# === Names from the class namespace ===
@dataclass
class ClassAliases:
    Alias = int
    plain: Alias = 0
    forward: 'Alias' = 0


assert get_type_hints(ClassAliases) == {'plain': int, 'forward': int}


# === Dataclasses made in functions ===
def make_dataclass_type():
    @dataclass
    class Made:
        a: int
        b: list[str] = field(default_factory=list)

    return Made


Made = make_dataclass_type()
assert get_type_hints(Made) == {'a': int, 'b': list[str]}
assert get_type_hints(make_dataclass_type()) == get_type_hints(Made)


# === Dataclasses as hints ===
def takes_point(p: Point, points: list[Point], maybe: 'Optional[Point]') -> 'Point':
    return p


assert get_type_hints(takes_point) == {'p': Point, 'points': list[Point], 'maybe': Point | None, 'return': Point}

# === Instances have no annotations ===
for value in [Point(1, 2), Frozen(1, 'a'), Empty()]:
    try:
        get_type_hints(value)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f'{value!r} does not have annotations'

# === Hints are unaffected by instances ===
p = Point(1, 2)
p.x = 'not an int'
assert get_type_hints(Point) == {'x': int, 'y': int}
assert get_type_hints(type(p)) == {'x': int, 'y': int}


# === Errors ===
@dataclass
class Undefined:
    a: 'NotDefinedAnywhere'


try:
    get_type_hints(Undefined)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'NotDefinedAnywhere' is not defined"

assert get_type_hints(Undefined, {'NotDefinedAnywhere': int}) == {'a': int}
assert get_type_hints(Undefined, localns={'NotDefinedAnywhere': str}) == {'a': str}
