from collections import namedtuple
from dataclasses import dataclass, field
from datetime import date, datetime, time, timedelta
from decimal import Decimal
from typing import Annotated, Any, Literal, Optional

from pydantic import ConfigDict, Field, TypeAdapter

# === scalars ===
assert TypeAdapter(int).json_schema() == {'type': 'integer'}
assert TypeAdapter(float).json_schema() == {'type': 'number'}
assert TypeAdapter(str).json_schema() == {'type': 'string'}
assert TypeAdapter(bool).json_schema() == {'type': 'boolean'}
assert TypeAdapter(bytes).json_schema() == {'format': 'binary', 'type': 'string'}
assert TypeAdapter(None).json_schema() == {'type': 'null'}
assert TypeAdapter(Any).json_schema() == {}
assert TypeAdapter(datetime).json_schema() == {'format': 'date-time', 'type': 'string'}
assert TypeAdapter(date).json_schema() == {'format': 'date', 'type': 'string'}
assert TypeAdapter(time).json_schema() == {'format': 'time', 'type': 'string'}
assert TypeAdapter(timedelta).json_schema() == {'format': 'duration', 'type': 'string'}
decimal_pattern = '^(?!^[-+.]*$)[+-]?0*\\d*\\.?\\d*$'
assert TypeAdapter(Decimal).json_schema() == {
    'anyOf': [{'type': 'number'}, {'pattern': decimal_pattern, 'type': 'string'}]
}

# === containers ===
assert TypeAdapter(list[int]).json_schema() == {'items': {'type': 'integer'}, 'type': 'array'}
assert TypeAdapter(set[str]).json_schema() == {'items': {'type': 'string'}, 'type': 'array', 'uniqueItems': True}
assert TypeAdapter(frozenset[str]).json_schema() == {'items': {'type': 'string'}, 'type': 'array', 'uniqueItems': True}
assert TypeAdapter(tuple[int, str]).json_schema() == {
    'maxItems': 2,
    'minItems': 2,
    'prefixItems': [{'type': 'integer'}, {'type': 'string'}],
    'type': 'array',
}
assert TypeAdapter(tuple[()]).json_schema() == {'maxItems': 0, 'minItems': 0, 'type': 'array'}
assert TypeAdapter(tuple[int, ...]).json_schema() == {'items': {'type': 'integer'}, 'type': 'array'}
assert TypeAdapter(dict).json_schema() == {'additionalProperties': True, 'type': 'object'}
assert TypeAdapter(dict[str, int]).json_schema() == {'additionalProperties': {'type': 'integer'}, 'type': 'object'}
assert TypeAdapter(list).json_schema() == {'items': {}, 'type': 'array'}

# === unions and literals ===
assert TypeAdapter(Optional[int]).json_schema() == {'anyOf': [{'type': 'integer'}, {'type': 'null'}]}
assert TypeAdapter(int | str | None).json_schema() == {
    'anyOf': [{'type': 'integer'}, {'type': 'string'}, {'type': 'null'}]
}
assert TypeAdapter(Literal['a', 'b']).json_schema() == {'enum': ['a', 'b'], 'type': 'string'}
assert TypeAdapter(Literal['a']).json_schema() == {'const': 'a', 'type': 'string'}
assert TypeAdapter(Literal[1, 'a']).json_schema() == {'enum': [1, 'a']}
assert TypeAdapter(Literal[1, 2]).json_schema() == {'enum': [1, 2], 'type': 'integer'}
assert TypeAdapter(Literal[None]).json_schema() == {'const': None, 'type': 'null'}

# === constraints ===
assert TypeAdapter(Annotated[int, Field(gt=0, le=10, multiple_of=2)]).json_schema() == {
    'exclusiveMinimum': 0,
    'maximum': 10,
    'multipleOf': 2,
    'type': 'integer',
}
assert TypeAdapter(Annotated[float, Field(ge=0.5, lt=2.5)]).json_schema() == {
    'exclusiveMaximum': 2.5,
    'minimum': 0.5,
    'type': 'number',
}
assert TypeAdapter(Annotated[Decimal, Field(gt=0, le=10)]).json_schema() == {
    'anyOf': [
        {'exclusiveMinimum': 0.0, 'maximum': 10.0, 'type': 'number'},
        {'pattern': decimal_pattern, 'type': 'string'},
    ]
}
assert TypeAdapter(Annotated[str, Field(min_length=1, max_length=5, pattern='^a')]).json_schema() == {
    'maxLength': 5,
    'minLength': 1,
    'pattern': '^a',
    'type': 'string',
}
assert TypeAdapter(Annotated[list[int], Field(min_length=1, max_length=5)]).json_schema() == {
    'items': {'type': 'integer'},
    'maxItems': 5,
    'minItems': 1,
    'type': 'array',
}
assert TypeAdapter(Annotated[dict[str, int], Field(min_length=1)]).json_schema() == {
    'additionalProperties': {'type': 'integer'},
    'minProperties': 1,
    'type': 'object',
}
assert TypeAdapter(Annotated[bytes, Field(max_length=5)]).json_schema() == {
    'format': 'binary',
    'maxLength': 5,
    'type': 'string',
}
# strictness is a validation setting, not a schema keyword
assert TypeAdapter(Annotated[int, Field(strict=True, gt=1)]).json_schema() == {'exclusiveMinimum': 1, 'type': 'integer'}
assert TypeAdapter(int, config=ConfigDict(strict=True)).json_schema() == {'type': 'integer'}

# === metadata ===
assert TypeAdapter(
    Annotated[int, Field(title='T', description='d', examples=[1, 2], json_schema_extra={'x': 1})]
).json_schema() == {'description': 'd', 'examples': [1, 2], 'title': 'T', 'type': 'integer', 'x': 1}
assert TypeAdapter(Annotated[int, Field(default=5)]).json_schema() == {'default': 5, 'type': 'integer'}
# nested Annotated flattens, the outer metadata winning
assert TypeAdapter(Annotated[Annotated[int, Field(gt=0, title='in')], Field(lt=5, title='out')]).json_schema() == {
    'exclusiveMaximum': 5,
    'exclusiveMinimum': 0,
    'title': 'out',
    'type': 'integer',
}
# metadata on an Optional applies to the union, constraints to its members
assert TypeAdapter(Annotated[Optional[str], Field(min_length=1, title='N')]).json_schema() == {
    'anyOf': [{'minLength': 1, 'type': 'string'}, {'type': 'null'}],
    'title': 'N',
}
assert TypeAdapter(Optional[Annotated[str, Field(title='x', min_length=1)]]).json_schema() == {
    'anyOf': [{'minLength': 1, 'title': 'x', 'type': 'string'}, {'type': 'null'}]
}
assert TypeAdapter(list[Annotated[int, Field(ge=0)]]).json_schema() == {
    'items': {'minimum': 0, 'type': 'integer'},
    'type': 'array',
}
# keys are sorted recursively, except under `properties` and `default`
assert TypeAdapter(
    Annotated[int, Field(json_schema_extra={'z': {'b': 1, 'a': {'d': 1, 'c': 2}}, 'a': 1})]
).json_schema() == {
    'a': 1,
    'type': 'integer',
    'z': {'a': {'c': 2, 'd': 1}, 'b': 1},
}
assert list(TypeAdapter(Annotated[int, Field(json_schema_extra={'z': 1, 'a': 1})]).json_schema()) == ['a', 'type', 'z']


# === dataclasses ===
@dataclass
class User:
    id: int
    name: str = 'x'
    tags: list[str] = field(default_factory=list)
    nick: Annotated[Optional[str], Field(alias='nickName', title='Nick')] = None
    when: datetime = datetime(2020, 1, 1)
    score: Annotated[Optional[int], Field(gt=0)] = None
    extra: dict[str, Any] = Field(default={'b': 1, 'a': {'d': 1, 'c': 2}}, description='dx', examples=[{'b': 1}])


user_properties = {
    'id': {'title': 'Id', 'type': 'integer'},
    'name': {'default': 'x', 'title': 'Name', 'type': 'string'},
    'tags': {'items': {'type': 'string'}, 'title': 'Tags', 'type': 'array'},
    'nickName': {'anyOf': [{'type': 'string'}, {'type': 'null'}], 'default': None, 'title': 'Nick'},
    'when': {'default': '2020-01-01T00:00:00', 'format': 'date-time', 'title': 'When', 'type': 'string'},
    'score': {
        'anyOf': [{'exclusiveMinimum': 0, 'type': 'integer'}, {'type': 'null'}],
        'default': None,
        'title': 'Score',
    },
    'extra': {
        'additionalProperties': True,
        'default': {'b': 1, 'a': {'c': 2, 'd': 1}},
        'description': 'dx',
        'examples': [{'b': 1}],
        'title': 'Extra',
        'type': 'object',
    },
}
user_schema = {'properties': user_properties, 'required': ['id'], 'title': 'User', 'type': 'object'}
schema = TypeAdapter(User).json_schema()
assert schema == user_schema
assert list(schema) == ['properties', 'required', 'title', 'type']
assert list(schema['properties']) == ['id', 'name', 'tags', 'nickName', 'when', 'score', 'extra']
assert list(schema['properties']['extra']['default']) == ['b', 'a']

by_name = TypeAdapter(User).json_schema(by_alias=False)
assert by_name['properties']['nick'] == user_properties['nickName']
assert 'nickName' not in by_name['properties']

# a nested dataclass is a `$ref` into `$defs`
assert TypeAdapter(list[User]).json_schema() == {
    '$defs': {'User': user_schema},
    'items': {'$ref': '#/$defs/User'},
    'type': 'array',
}
assert TypeAdapter(list[User]).json_schema(ref_template='#/components/{model}') == {
    '$defs': {'User': user_schema},
    'items': {'$ref': '#/components/User'},
    'type': 'array',
}
assert TypeAdapter(Optional[User]).json_schema() == {
    '$defs': {'User': user_schema},
    'anyOf': [{'$ref': '#/$defs/User'}, {'type': 'null'}],
}


@dataclass
class Node:
    value: int
    children: list['Node']
    parent: Optional['Node'] = None


# a recursive dataclass is defined once and referenced; a `$ref` field gets no generated title
node_schema = {
    'properties': {
        'value': {'title': 'Value', 'type': 'integer'},
        'children': {'items': {'$ref': '#/$defs/Node'}, 'title': 'Children', 'type': 'array'},
        'parent': {'anyOf': [{'$ref': '#/$defs/Node'}, {'type': 'null'}], 'default': None},
    },
    'required': ['value', 'children'],
    'title': 'Node',
    'type': 'object',
}
assert TypeAdapter(Node).json_schema() == {'$defs': {'Node': node_schema}, '$ref': '#/$defs/Node'}


@dataclass
class Config:
    __pydantic_config__ = ConfigDict(extra='forbid', title='Custom')
    a: int
    b: Annotated[str, Field(exclude=True, serialization_alias='B')] = 'q'


@dataclass
class Outer:
    one: Config
    maybe: Optional[Config] = None
    many: list[Config] | None = None


config_schema = {
    'additionalProperties': False,
    'properties': {'a': {'title': 'A', 'type': 'integer'}, 'b': {'default': 'q', 'title': 'B', 'type': 'string'}},
    'required': ['a'],
    'title': 'Custom',
    'type': 'object',
}
assert TypeAdapter(Config).json_schema() == config_schema
# serialization mode drops excluded fields and uses serialization aliases
assert TypeAdapter(Config).json_schema(mode='serialization') == {
    'additionalProperties': False,
    'properties': {'a': {'title': 'A', 'type': 'integer'}},
    'required': ['a'],
    'title': 'Custom',
    'type': 'object',
}
assert TypeAdapter(Config).json_schema(mode='validation') == config_schema
# `$defs` are keyed by class name, not title
assert TypeAdapter(Outer).json_schema() == {
    '$defs': {'Config': config_schema},
    'properties': {
        'one': {'$ref': '#/$defs/Config'},
        'maybe': {'anyOf': [{'$ref': '#/$defs/Config'}, {'type': 'null'}], 'default': None},
        'many': {
            'anyOf': [{'items': {'$ref': '#/$defs/Config'}, 'type': 'array'}, {'type': 'null'}],
            'default': None,
            'title': 'Many',
        },
    },
    'required': ['one'],
    'title': 'Outer',
    'type': 'object',
}

# === namedtuple ===
Point = namedtuple('Point', ['x', 'y'])
Defaulted = namedtuple('Defaulted', ['a', 'b'], defaults=[2])
assert TypeAdapter(Point).json_schema() == {
    'maxItems': 2,
    'minItems': 2,
    'prefixItems': [{'title': 'X'}, {'title': 'Y'}],
    'type': 'array',
}
assert TypeAdapter(Defaulted).json_schema() == {
    'maxItems': 2,
    'minItems': 1,
    'prefixItems': [{'title': 'A'}, {'default': 2, 'title': 'B'}],
    'type': 'array',
}

# === arguments ===
try:
    TypeAdapter(User).json_schema(by_alias='x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'by_alias': 'str' object is not an instance of 'bool'"
try:
    TypeAdapter(User).json_schema(mode='bogus')
    assert False, 'expected AssertionError'
except AssertionError as exc:
    assert str(exc) == "Expected code to be unreachable, but got: 'bogus'"
try:
    TypeAdapter(list[User]).json_schema(ref_template=1)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'int' object has no attribute 'format'"
