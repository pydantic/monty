from typing import ClassVar


# === Ordered __annotations__ holding evaluated objects (PEP 649) ===
class C:
    x: int
    y: str = 'hi'
    z = 5  # no annotation -> not a field
    cv: ClassVar[int] = 0


assert list(C.__annotations__.keys()) == ['x', 'y', 'cv']
assert C.__annotations__['x'] is int
assert C.__annotations__['y'] is str
assert C.__annotations__['cv'] == ClassVar[int]
assert 'z' not in C.__annotations__, 'unannotated class var is not in __annotations__'

# === Annotated-with-value is also a real class variable ===
assert C.y == 'hi'
assert C.z == 5

# === Evaluated once and cached: the same dict every time, mutations included ===
assert C.__annotations__ is C.__annotations__
C.__annotations__['extra'] = float
assert C.__annotations__['extra'] is float


# === Parameterized types evaluate like any other expression ===
class Container:
    items: list[int]
    mapping: dict[str, int]
    table: dict[str, list['Container']]


assert Container.__annotations__['items'] == list[int]
assert Container.__annotations__['mapping'] == dict[str, int]
assert Container.__annotations__['table'] == dict[str, list['Container']]


# === Class-body names are visible to the class's own annotations ===
class Scoped:
    Alias = int
    a: Alias
    b: 'Alias'


assert Scoped.__annotations__['a'] is int
assert Scoped.__annotations__['b'] == 'Alias'


# === Lazy: a forward reference only fails when __annotations__ is read ===
class Node:
    parent: Parent


try:
    Node.__annotations__
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Parent' is not defined"


class Parent:
    pass


assert Node.__annotations__['parent'] is Parent


# === Assigning __annotations__ replaces the evaluated dict ===
class Replaced:
    a: int


Replaced.__annotations__ = {'b': str}
assert Replaced.__annotations__ == {'b': str}


# === Empty class: __annotations__ is an empty dict, also cached ===
class E:
    p = 1


assert E.__annotations__ == {}
assert E.__annotations__ is E.__annotations__

# === Accessible via type(instance) too ===
c = C()
assert type(c).__annotations__['x'] is int


# === What __annotations__ unlocks: a transformer discovering its own fields ===
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
