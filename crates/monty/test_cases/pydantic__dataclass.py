# /// monty
# cpython-site-packages = true
# ///
from collections import namedtuple
from dataclasses import dataclass, field
from typing import Any, Optional

from pydantic import TypeAdapter, ValidationError


def errors(adapter, value, **kwargs):
    try:
        adapter.validate_python(value, **kwargs)
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


@dataclass
class Foo:
    x: int
    y: str = 'a'
    z: list[int] = field(default_factory=list)


# === A dict input builds a new instance ===
FooAdapter = TypeAdapter(Foo)
assert FooAdapter.validate_python({'x': 1}) == Foo(1)
assert FooAdapter.validate_python({'x': '1', 'y': 'b', 'z': ['1', 2]}) == Foo(1, 'b', [1, 2])
built = FooAdapter.validate_python({'x': 1})
assert isinstance(built, Foo)
assert built.z == []
assert built.z is not FooAdapter.validate_python({'x': 1}).z
# unknown keys are ignored
assert FooAdapter.validate_python({'x': 1, 'q': 2}) == Foo(1)
assert errors(FooAdapter, {'y': 1, 'q': 3}) == [
    {'type': 'missing', 'loc': ('x',), 'msg': 'Field required', 'input': {'y': 1, 'q': 3}},
    {'type': 'string_type', 'loc': ('y',), 'msg': 'Input should be a valid string', 'input': 1},
]
assert errors(FooAdapter, {}) == [{'type': 'missing', 'loc': ('x',), 'msg': 'Field required', 'input': {}}]
assert errors(FooAdapter, {'x': 'bad'}) == [
    {
        'type': 'int_parsing',
        'loc': ('x',),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'bad',
    }
]
assert errors(FooAdapter, {'x': 1, 'z': [1, 'q']})[0]['loc'] == ('z', 1)
assert errors(FooAdapter, 5) == [
    {
        'type': 'dataclass_type',
        'loc': (),
        'msg': 'Input should be a dictionary or an instance of Foo',
        'input': 5,
        'ctx': {'class_name': 'Foo'},
    }
]
assert errors(FooAdapter, [1])[0]['type'] == 'dataclass_type'
assert errors(FooAdapter, None)[0]['type'] == 'dataclass_type'
try:
    FooAdapter.validate_python({})
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'Foo'
    assert str(exc) == (
        '1 validation error for Foo\nx\n  Field required [type=missing, input_value={}, input_type=dict]\n'
        '    For further information visit https://errors.pydantic.dev/2.13/v/missing'
    )

# === An instance passes through unchanged ===
foo = Foo(2)
assert FooAdapter.validate_python(foo) is foo
assert FooAdapter.validate_python(foo, strict=True) is foo
assert errors(FooAdapter, {'x': 1}, strict=True) == [
    {
        'type': 'dataclass_exact_type',
        'loc': (),
        'msg': 'Input should be an instance of Foo',
        'input': {'x': 1},
        'ctx': {'class_name': 'Foo'},
    }
]


# === __post_init__ runs on the built instance ===
@dataclass
class WithPost:
    x: int

    def __post_init__(self):
        self.x += 1


assert TypeAdapter(WithPost).validate_python({'x': 1}).x == 2


@dataclass
class Checked:
    x: int

    def __post_init__(self):
        if self.x < 0:
            raise ValueError('negative')


assert TypeAdapter(Checked).validate_python({'x': 1}) == Checked(1)
try:
    TypeAdapter(Checked).validate_python({'x': -1})
    assert False, 'expected an error'
except ValueError as exc:
    assert isinstance(exc, ValidationError) or str(exc) == 'negative'


# === Recursive and nested dataclasses ===
@dataclass
class Node:
    value: int
    children: list['Node'] = field(default_factory=list)
    parent: Optional['Node'] = None


NodeAdapter = TypeAdapter(Node)
tree = NodeAdapter.validate_python(
    {'value': 1, 'children': [{'value': 2, 'children': [{'value': '3'}]}], 'parent': {'value': 0}}
)
assert tree == Node(1, [Node(2, [Node(3)])], Node(0))
assert errors(NodeAdapter, {'value': 1, 'children': [{'value': 'x'}]}) == [
    {
        'type': 'int_parsing',
        'loc': ('children', 0, 'value'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    }
]
assert errors(NodeAdapter, {'value': 1, 'parent': 5})[0]['loc'] == ('parent',)
# a dict that contains itself is caught where the walk meets it again
cyclic = {'value': 1, 'children': []}
cyclic['children'].append(cyclic)
cyclic_error = errors(NodeAdapter, cyclic)[0]
assert (cyclic_error['type'], cyclic_error['loc'], cyclic_error['msg']) == (
    'recursion_loop',
    ('children', 0),
    'Recursion error - cyclic reference detected',
)
cyclic['children'].clear()
assert TypeAdapter(list[Foo]).validate_python([{'x': 1}, {'x': 2, 'y': 'b'}]) == [Foo(1), Foo(2, 'b')]
assert TypeAdapter(dict[str, Foo]).validate_python({'a': {'x': 1}}) == {'a': Foo(1)}
assert TypeAdapter(Foo | None).validate_python(None) is None
assert TypeAdapter(Foo | int).validate_python({'x': 1}) == Foo(1)
assert TypeAdapter(Foo | int).validate_python('3') == 3
assert errors(TypeAdapter(Foo | int), {'x': 'q'}) == [
    {
        'type': 'int_parsing',
        'loc': ('Foo', 'x'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'q',
    },
    {'type': 'int_type', 'loc': ('int',), 'msg': 'Input should be a valid integer', 'input': {'x': 'q'}},
]


# === Fields typed with Any or without coercion ===
@dataclass
class Loose:
    data: Any
    tags: list = field(default_factory=list)


assert TypeAdapter(Loose).validate_python({'data': {'k': [1]}, 'tags': (1, 'a')}) == Loose({'k': [1]}, [1, 'a'])

# === namedtuple ===
Point = namedtuple('Point', ['x', 'y'])
PointAdapter = TypeAdapter(Point)
assert PointAdapter.validate_python((1, 2)) == Point(1, 2)
assert PointAdapter.validate_python([1, 2]) == Point(1, 2)
assert PointAdapter.validate_python({'x': 1, 'y': 2}) == Point(1, 2)
assert PointAdapter.validate_python(Point(1, 2)) == Point(1, 2)
assert type(PointAdapter.validate_python([1, 2])) is Point
# namedtuple fields are untyped, so values pass through
assert PointAdapter.validate_python(('a', None)) == Point('a', None)
assert errors(PointAdapter, (1,)) == [
    {'type': 'missing_argument', 'loc': ('y',), 'msg': 'Missing required argument', 'input': (1,)}
]
assert errors(PointAdapter, (1, 2, 3)) == [
    {'type': 'unexpected_positional_argument', 'loc': (2,), 'msg': 'Unexpected positional argument', 'input': 3}
]
assert errors(PointAdapter, {'x': 1, 'y': 2, 'z': 3}) == [
    {'type': 'unexpected_keyword_argument', 'loc': ('z',), 'msg': 'Unexpected keyword argument', 'input': 3}
]
assert errors(PointAdapter, {'x': 1}) == [
    {'type': 'missing_argument', 'loc': ('y',), 'msg': 'Missing required argument', 'input': {'x': 1}}
]
assert errors(PointAdapter, 5) == [
    {'type': 'arguments_type', 'loc': (), 'msg': 'Arguments must be a tuple, list or a dictionary', 'input': 5}
]
try:
    PointAdapter.validate_python((1,))
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'call[Point]'
Defaulted = namedtuple('Defaulted', ['a', 'b'], defaults=[2])
assert TypeAdapter(Defaulted).validate_python((1,)) == Defaulted(1, 2)
assert TypeAdapter(Defaulted).validate_python({'a': 1}) == Defaulted(1, 2)
assert TypeAdapter(list[Point]).validate_python([(1, 2), {'x': 3, 'y': 4}]) == [Point(1, 2), Point(3, 4)]
