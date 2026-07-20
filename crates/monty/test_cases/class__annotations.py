# Monty stores class annotations in stringized form, unconditionally. Using
# `from __future__ import annotations` makes CPython do the same, so these
# asserts hold on both interpreters. Without the import, CPython 3.14 would
# store the evaluated objects instead (PEP 649) — a documented divergence.
#
# Stringization is a known temporary divergence (see limitations/typing.md).
# If Monty ever matches PEP 649, this file changes as follows: the `__future__`
# import at the top goes away, and every assert comparing a value to a string
# (`== 'int'`, `== 'list[int]'`, ...) becomes an identity check against the type
# object. The asserts on annotation *keys* and their order stay as they are.
from __future__ import annotations


# === Ordered __annotations__, stringized, excluding unannotated names ===
class C:
    x: int
    y: str = 'hi'
    z = 5  # no annotation -> not a field
    cv: ClassVar[int] = 0


assert list(C.__annotations__.keys()) == ['x', 'y', 'cv']
assert C.__annotations__['x'] == 'int'
assert C.__annotations__['y'] == 'str'
# Parameterized forms Monty cannot evaluate are preserved verbatim as text.
assert C.__annotations__['cv'] == 'ClassVar[int]'
assert 'z' not in C.__annotations__, 'unannotated class var is not in __annotations__'

# === Annotated-with-value is also a real class variable ===
assert C.y == 'hi'
assert C.z == 5


# === Parameterized types that Monty cannot evaluate are fine as strings ===
class Container:
    items: list[int]
    mapping: dict[str, int]


assert Container.__annotations__['items'] == 'list[int]'
assert Container.__annotations__['mapping'] == 'dict[str, int]'


# === Annotations are normalized, not captured as raw source text ===
# Both interpreters stringize by unparsing the expression, so the original
# spacing and line breaks are discarded rather than embedded in the value.
class Spacing:
    a: list [ int ]  # fmt: skip
    b: dict[str,int]  # fmt: skip
    c: dict[
        str,
        int,
    ]


assert Spacing.__annotations__['a'] == 'list[int]'
assert Spacing.__annotations__['b'] == 'dict[str, int]'
assert Spacing.__annotations__['c'] == 'dict[str, int]'


# === String annotations normalize to single quotes, as CPython's does ===
class Quoted:
    a: "int"  # fmt: skip
    b: 'int'
    c: dict[str, "Foo"]  # fmt: skip


assert Quoted.__annotations__['a'] == "'int'"
assert Quoted.__annotations__['b'] == "'int'"
assert Quoted.__annotations__['c'] == "dict[str, 'Foo']"


# === Empty class: __annotations__ is an empty dict ===
class E:
    p = 1


assert E.__annotations__ == {}

# === Accessible via type(instance) too ===
c = C()
assert type(c).__annotations__['x'] == 'int'


# === What __annotations__ unlocks: field discovery driven by the annotations ===
# A class transformer written in sandboxed Python that discovers its own fields
# — the pattern `@dataclass` automates.
#
# Note it reads only `list(cls.__annotations__)` — the keys and their order.
# The annotation *values* are never inspected, so this transformer is unaffected
# by whether they are strings or type objects. CPython's own `@dataclass` is the
# same, save for recognising `ClassVar`/`InitVar`, which it matches textually.
def mini_dataclass(cls):
    fields = list(cls.__annotations__)

    def __init__(self, *args, **kwargs):
        for name, val in zip(fields, args):
            setattr(self, name, val)
        for name, val in kwargs.items():
            setattr(self, name, val)

    def __repr__(self):
        inner = ', '.join(f'{n}={getattr(self, n)!r}' for n in fields)
        return f'{cls.__name__}({inner})'

    cls.__init__ = __init__
    cls.__repr__ = __repr__
    return cls


@mini_dataclass
class Point:
    x: int
    y: int


p = Point(1, 2)
assert p.x == 1
assert p.y == 2
assert repr(p) == 'Point(x=1, y=2)'
assert repr(Point(x=5, y=6)) == 'Point(x=5, y=6)'
