import dataclasses
from typing import Annotated

import pydantic.dataclasses
from pydantic import ConfigDict, Field, TypeAdapter, ValidationError
from pydantic.dataclasses import dataclass, is_pydantic_dataclass


def errors(fn):
    try:
        fn()
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


# === Construction validates, positionally and by keyword ===
@dataclass
class Point:
    x: int
    y: str = 'a'
    tags: list[int] = dataclasses.field(default_factory=list)

    def __post_init__(self):
        self.total = self.x + len(self.tags)


assert Point('1', 'b', ['2']) == Point(1, 'b', [2])
assert Point(x='1').y == 'a'
assert Point(1, tags=[1, 2]).total == 3
assert repr(Point(1)) == "Point(x=1, y='a', tags=[])"
assert Point(1) == Point(1)
assert Point(1) != Point(2)
assert Point(1, w=2) == Point(1), 'an unknown keyword is dropped'
assert dataclasses.is_dataclass(Point)
assert is_pydantic_dataclass(Point)
assert not is_pydantic_dataclass(int)
assert pydantic.dataclasses.is_pydantic_dataclass(Point)
assert Point.__pydantic_config__ == {}
assert hasattr(Point, '__pydantic_validator__')
p = Point(1)
p.x = 'unchecked'
assert p.x == 'unchecked', 'assignment is not validated'

# positional errors are located by index, keyword ones by name
assert errors(lambda: Point('a')) == [
    {
        'type': 'int_parsing',
        'loc': (0,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'a',
    }
]
assert errors(lambda: Point(x='a')) == [
    {
        'type': 'int_parsing',
        'loc': ('x',),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'a',
    }
]
assert errors(lambda: Point(1, 'b', [], 4, 5)) == [
    {'type': 'unexpected_positional_argument', 'loc': (3,), 'msg': 'Unexpected positional argument', 'input': 4},
    {'type': 'unexpected_positional_argument', 'loc': (4,), 'msg': 'Unexpected positional argument', 'input': 5},
]
assert errors(lambda: Point(1, x=2)) == [
    {'type': 'multiple_argument_values', 'loc': ('x',), 'msg': 'Got multiple values for argument', 'input': 2}
]
missing = errors(lambda: Point())
assert [(e['type'], e['loc'], e['msg']) for e in missing] == [('missing', ('x',), 'Field required')]
try:
    Point('a')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'Point'

# an explicit __init__ call validates too
q = Point(1)
Point.__init__(q, '7', tags=[1])
assert (q.x, q.tags, q.total) == (7, [1], 8)

# === The class works with TypeAdapter and the stdlib module ===
PointAdapter = TypeAdapter(Point)
assert PointAdapter.validate_python({'x': '3'}) == Point(3)
assert PointAdapter.validate_json('{"x": 3, "tags": ["4"]}') == Point(3, 'a', [4])
assert PointAdapter.validate_python(Point(1)) == Point(1)
assert PointAdapter.dump_python(Point(1)) == {'x': 1, 'y': 'a', 'tags': []}
assert PointAdapter.dump_json(Point(1)) == b'{"x":1,"y":"a","tags":[]}'
assert errors(lambda: PointAdapter.validate_python({'x': 'a'})) == [
    {
        'type': 'int_parsing',
        'loc': ('x',),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'a',
    }
]


# === Options: frozen, config, Field ===
@dataclass(frozen=True, config=ConfigDict(str_to_upper=True, extra='forbid'))
class Frozen:
    name: str
    n: Annotated[int, Field(gt=0)] = Field(default=1, alias='N')


assert Frozen('ab', N=2) == Frozen('AB', 2)
assert Frozen.__pydantic_config__ == {'str_to_upper': True, 'extra': 'forbid'}
assert hash(Frozen('ab')) == hash(Frozen('ab'))
try:
    Frozen('ab').name = 'x'
    assert False, 'expected FrozenInstanceError'
except dataclasses.FrozenInstanceError as exc:
    assert str(exc) == "cannot assign to field 'name'"
assert errors(lambda: Frozen('ab', n=2)) == [
    {'type': 'unexpected_keyword_argument', 'loc': ('n',), 'msg': 'Unexpected keyword argument', 'input': 2}
]
assert errors(lambda: Frozen('ab', q=1)) == [
    {'type': 'unexpected_keyword_argument', 'loc': ('q',), 'msg': 'Unexpected keyword argument', 'input': 1}
]
assert errors(lambda: Frozen('ab', N=0)) == [
    {'type': 'greater_than', 'loc': ('N',), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]

# a configured decorator can be kept and applied later
strict_dataclass = dataclass(config=ConfigDict(strict=True))


@strict_dataclass
class Exact:
    n: int


assert Exact(1).n == 1
assert errors(lambda: Exact('1')) == [
    {'type': 'int_type', 'loc': (0,), 'msg': 'Input should be a valid integer', 'input': '1'}
]
assert errors(lambda: TypeAdapter(Exact).validate_python({'n': '1'})) == [
    {
        'type': 'dataclass_exact_type',
        'loc': (),
        'msg': 'Input should be an instance of Exact',
        'input': {'n': '1'},
        'ctx': {'class_name': 'Exact'},
    }
]


@dataclass(validate_on_init=True)
class Tolerant:
    n: int


assert Tolerant('1').n == 1, 'validate_on_init is accepted and ignored'

try:
    dataclass(init=True)
    assert False, 'expected an AssertionError'
except AssertionError as exc:
    assert str(exc) == 'pydantic.dataclasses.dataclass only supports init=False'


# === Nesting ===
@dataclass
class Outer:
    inner: Point
    items: list[Point] = dataclasses.field(default_factory=list)


assert Outer({'x': 1}, [{'x': 2}]) == Outer(Point(1), [Point(2)])
assert Outer(Point(1)).items == []
assert errors(lambda: Outer({'x': 'a'})) == [
    {
        'type': 'int_parsing',
        'loc': (0, 'x'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'a',
    }
]
assert errors(lambda: Outer(inner={'x': 'a'})) == [
    {
        'type': 'int_parsing',
        'loc': ('inner', 'x'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'a',
    }
]
assert TypeAdapter(Outer).validate_json('{"inner": {"x": 1}}') == Outer(Point(1))
