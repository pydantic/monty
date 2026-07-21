# Monty stringizes class annotations unconditionally; the `__future__` import
# makes CPython do the same, so these asserts hold on both. Without it CPython
# 3.14 stores evaluated objects (PEP 649).
#
# Stringization is a known temporary divergence (see limitations/typing.md). If
# Monty ever matches PEP 649, the import goes away and every `== 'int'`-style
# assert becomes an identity check; the key/order asserts stay.
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
# Both unparse the expression, so spacing and line breaks are discarded.
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
    # f-strings carry their own quote flags, so they normalize separately.
    d: f"int"  # fmt: skip
    # A single quote inside keeps the double quotes: escape-minimizing wins.
    e: "it's"  # fmt: skip


assert Quoted.__annotations__['a'] == "'int'"
assert Quoted.__annotations__['b'] == "'int'"
assert Quoted.__annotations__['c'] == "dict[str, 'Foo']"
assert Quoted.__annotations__['d'] == "f'int'"
assert Quoted.__annotations__['e'] == '"it\'s"'


# === Literals are rebuilt canonically, not echoed part-by-part ===
# Concatenated parts merge and a raw prefix folds into the value. A `u` prefix
# survives, though — canonical is not the same as bare.
class Literals:
    a: 'foo' 'bar'  # fmt: skip
    b: f"x" "y"  # fmt: skip
    c: r'raw\d'
    d: "a" r"b\n"  # fmt: skip
    e: u'uni'  # fmt: skip
    f: """triple"""  # fmt: skip
    g: f'pre{1}post'
    h: dict[str, 'A' 'B']  # fmt: skip


assert Literals.__annotations__['a'] == "'foobar'"
assert Literals.__annotations__['b'] == "f'xy'"
assert Literals.__annotations__['c'] == "'raw\\\\d'"
assert Literals.__annotations__['d'] == "'ab\\\\n'"
assert Literals.__annotations__['e'] == "u'uni'"
assert Literals.__annotations__['f'] == "'triple'"
assert Literals.__annotations__['g'] == "f'pre{1}post'"
assert Literals.__annotations__['h'] == "dict[str, 'AB']"


# === bytes literals canonicalize the same way, minus the `u` prefix ===
class ByteLiterals:
    a: b'foo'
    b: b'foo' b'bar'  # fmt: skip
    c: rb'raw\d'
    d: b"""triple"""  # fmt: skip


assert ByteLiterals.__annotations__['a'] == "b'foo'"
assert ByteLiterals.__annotations__['b'] == "b'foobar'"
assert ByteLiterals.__annotations__['c'] == "b'raw\\\\d'"
assert ByteLiterals.__annotations__['d'] == "b'triple'"


# === Empty class: __annotations__ is an empty dict ===
class E:
    p = 1


assert E.__annotations__ == {}

# === Accessible via type(instance) too ===
c = C()
assert type(c).__annotations__['x'] == 'int'


# === What __annotations__ unlocks: a transformer discovering its own fields ===
# It reads only the keys and their order, never the values, so stringization
# cannot affect it. CPython's `@dataclass` does inspect values, but falls back
# to matching `ClassVar`/`InitVar` textually when they are strings.
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
