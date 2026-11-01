from dataclasses import dataclass, field
from decimal import Decimal
from typing import Annotated

from pydantic import ConfigDict, Field, TypeAdapter, ValidationError


def errors(adapter, value, **kwargs):
    try:
        adapter.validate_python(value, **kwargs)
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


def json_errors(adapter, document):
    try:
        adapter.validate_json(document)
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


# === Field() returns a FieldInfo ===
assert repr(Field(gt=0)) == 'FieldInfo(annotation=NoneType, required=True, metadata=[Gt(gt=0)])'
assert repr(Field(1, alias='x', description='d')) == (
    "FieldInfo(annotation=NoneType, required=False, default=1, alias='x', alias_priority=2, description='d')"
)
assert repr(Field(gt=0, le=1, min_length=1, pattern='x', strict=True, multiple_of=2, max_length=3, ge=0, lt=5)) == (
    'FieldInfo(annotation=NoneType, required=True, metadata=[Strict(strict=True), Gt(gt=0), Ge(ge=0), Lt(lt=5), '
    "Le(le=1), MultipleOf(multiple_of=2), MinLen(min_length=1), MaxLen(max_length=3), _PydanticGeneralMetadata(pattern='x')])"
)
assert repr(
    Field(
        json_schema_extra={'a': 1},
        examples=[1],
        title='T',
        exclude=True,
        frozen=True,
        default_factory=list,
        validation_alias='v',
        serialization_alias='s',
    )
) == (
    'FieldInfo(annotation=NoneType, required=False, default_factory=list, alias_priority=2, '
    "validation_alias='v', serialization_alias='s', title='T', examples=[1], exclude=True, "
    "json_schema_extra={'a': 1}, frozen=True)"
)
assert repr(Field(None)) == 'FieldInfo(annotation=NoneType, required=False, default=None)'
assert Field().is_required()
assert not Field(None).is_required(), 'a None default is a default'
assert not Field(default_factory=dict).is_required(), 'a factory is a default'
assert Field(1).default == 1
assert Field(alias='a').alias == 'a'
assert Field(gt=0).title is None
assert Field(title='T', description='D', examples=[1, 2]).description == 'D'
try:
    Field(1, default_factory=list)
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == 'cannot specify both default and default_factory'

# === ConfigDict is a plain dict ===
assert ConfigDict(strict=True, title='T') == {'strict': True, 'title': 'T'}
assert ConfigDict() == {}

# === Numeric bounds: pydantic-core's check order is multiple_of, le, lt, ge, gt ===
Bounded = TypeAdapter(Annotated[int, Field(gt=0, le=10, multiple_of=2)])
assert Bounded.validate_python(4) == 4
assert Bounded.validate_python('6') == 6
assert errors(Bounded, 11) == [
    {'type': 'multiple_of', 'loc': (), 'msg': 'Input should be a multiple of 2', 'input': 11, 'ctx': {'multiple_of': 2}}
]
assert errors(Bounded, 12) == [
    {
        'type': 'less_than_equal',
        'loc': (),
        'msg': 'Input should be less than or equal to 10',
        'input': 12,
        'ctx': {'le': 10},
    }
]
assert errors(Bounded, 0) == [
    {'type': 'greater_than', 'loc': (), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]
assert errors(TypeAdapter(Annotated[int, Field(lt=5, ge=1)]), 5) == [
    {'type': 'less_than', 'loc': (), 'msg': 'Input should be less than 5', 'input': 5, 'ctx': {'lt': 5}}
]
assert errors(TypeAdapter(Annotated[int, Field(lt=5, ge=1)]), 0) == [
    {
        'type': 'greater_than_equal',
        'loc': (),
        'msg': 'Input should be greater than or equal to 1',
        'input': 0,
        'ctx': {'ge': 1},
    }
]
# an int bound on a float field is kept as a float, and prints without the `.0`
assert errors(TypeAdapter(Annotated[float, Field(gt=1)]), 0.5) == [
    {'type': 'greater_than', 'loc': (), 'msg': 'Input should be greater than 1', 'input': 0.5, 'ctx': {'gt': 1.0}}
]
assert errors(TypeAdapter(Annotated[float, Field(ge=1.5)]), 1.0) == [
    {
        'type': 'greater_than_equal',
        'loc': (),
        'msg': 'Input should be greater than or equal to 1.5',
        'input': 1.0,
        'ctx': {'ge': 1.5},
    }
]
FloatStep = TypeAdapter(Annotated[float, Field(multiple_of=0.1)])
assert FloatStep.validate_python(0.3) == 0.3
assert errors(FloatStep, 0.35) == [
    {
        'type': 'multiple_of',
        'loc': (),
        'msg': 'Input should be a multiple of 0.1',
        'input': 0.35,
        'ctx': {'multiple_of': 0.1},
    }
]
DecimalStep = TypeAdapter(Annotated[Decimal, Field(multiple_of=Decimal('0.5'), le=10, gt=1)])
assert DecimalStep.validate_python('2.5') == Decimal('2.5')
assert errors(DecimalStep, Decimal('0.7')) == [
    {
        'type': 'multiple_of',
        'loc': (),
        'msg': 'Input should be a multiple of 0.5',
        'input': Decimal('0.7'),
        'ctx': {'multiple_of': Decimal('0.5')},
    }
]
assert errors(DecimalStep, 1) == [
    {
        'type': 'greater_than',
        'loc': (),
        'msg': 'Input should be greater than 1',
        'input': 1,
        'ctx': {'gt': Decimal('1')},
    }
]
# a bound of the wrong kind is refused when the adapter is built
for bad in [Field(gt='a'), Field(ge=1.5), Field(multiple_of=2.5)]:
    try:
        TypeAdapter(Annotated[int, bad])
        assert False, 'expected a build error'
    except Exception as exc:
        assert str(exc).endswith('must be coercible to an integer')
# JSON mode applies the same bounds
assert json_errors(Bounded, '0') == [
    {'type': 'greater_than', 'loc': (), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]
assert Bounded.validate_json('8') == 8

# === String rules: strip, length, pattern, then case ===
Short = TypeAdapter(Annotated[str, Field(min_length=2, max_length=3, pattern='^a')])
assert Short.validate_python('ab') == 'ab'
assert errors(Short, 'a') == [
    {
        'type': 'string_too_short',
        'loc': (),
        'msg': 'String should have at least 2 characters',
        'input': 'a',
        'ctx': {'min_length': 2},
    }
]
assert errors(Short, 'abcd') == [
    {
        'type': 'string_too_long',
        'loc': (),
        'msg': 'String should have at most 3 characters',
        'input': 'abcd',
        'ctx': {'max_length': 3},
    }
]
assert errors(Short, 'bcd') == [
    {
        'type': 'string_pattern_mismatch',
        'loc': (),
        'msg': "String should match pattern '^a'",
        'input': 'bcd',
        'ctx': {'pattern': '^a'},
    }
]
assert errors(TypeAdapter(Annotated[str, Field(min_length=1)]), '') == [
    {
        'type': 'string_too_short',
        'loc': (),
        'msg': 'String should have at least 1 character',
        'input': '',
        'ctx': {'min_length': 1},
    }
]
assert json_errors(Short, '"bcd"') == [
    {
        'type': 'string_pattern_mismatch',
        'loc': (),
        'msg': "String should match pattern '^a'",
        'input': 'bcd',
        'ctx': {'pattern': '^a'},
    }
]
# the pattern is searched, not fully matched; length counts characters
assert TypeAdapter(Annotated[str, Field(pattern='b')]).validate_python('abc') == 'abc'
assert TypeAdapter(Annotated[str, Field(max_length=2)]).validate_python('éé') == 'éé'
Strict = TypeAdapter(Annotated[str, Field(strict=True)])
assert errors(Strict, b'x') == [
    {'type': 'string_type', 'loc': (), 'msg': 'Input should be a valid string', 'input': b'x'}
]

# === Lengths of bytes and containers, checked after the items validate ===
assert errors(TypeAdapter(Annotated[bytes, Field(max_length=1)]), b'ab') == [
    {
        'type': 'bytes_too_long',
        'loc': (),
        'msg': 'Data should have at most 1 byte',
        'input': b'ab',
        'ctx': {'max_length': 1},
    }
]
assert errors(TypeAdapter(Annotated[bytes, Field(min_length=3)]), b'ab') == [
    {
        'type': 'bytes_too_short',
        'loc': (),
        'msg': 'Data should have at least 3 bytes',
        'input': b'ab',
        'ctx': {'min_length': 3},
    }
]
Items = TypeAdapter(Annotated[list[int], Field(min_length=2, max_length=3)])
assert Items.validate_python(['1', 2]) == [1, 2]
assert errors(Items, [1, 2, 3, 4]) == [
    {
        'type': 'too_long',
        'loc': (),
        'msg': 'List should have at most 3 items after validation, not 4',
        'input': [1, 2, 3, 4],
        'ctx': {'field_type': 'List', 'max_length': 3, 'actual_length': 4},
    }
]
assert errors(Items, [1]) == [
    {
        'type': 'too_short',
        'loc': (),
        'msg': 'List should have at least 2 items after validation, not 1',
        'input': [1],
        'ctx': {'field_type': 'List', 'min_length': 2, 'actual_length': 1},
    }
]
assert json_errors(Items, '[1]') == [
    {
        'type': 'too_short',
        'loc': (),
        'msg': 'List should have at least 2 items after validation, not 1',
        'input': [1],
        'ctx': {'field_type': 'List', 'min_length': 2, 'actual_length': 1},
    }
]
assert errors(TypeAdapter(Annotated[dict[str, int], Field(max_length=1)]), {'a': 1, 'b': 2}) == [
    {
        'type': 'too_long',
        'loc': (),
        'msg': 'Dictionary should have at most 1 item after validation, not 2',
        'input': {'a': 1, 'b': 2},
        'ctx': {'field_type': 'Dictionary', 'max_length': 1, 'actual_length': 2},
    }
]
assert errors(TypeAdapter(Annotated[tuple[int, ...], Field(max_length=1)]), (1, 2)) == [
    {
        'type': 'too_long',
        'loc': (),
        'msg': 'Tuple should have at most 1 item after validation, not 2',
        'input': (1, 2),
        'ctx': {'field_type': 'Tuple', 'max_length': 1, 'actual_length': 2},
    }
]
assert errors(TypeAdapter(Annotated[set[int], Field(max_length=1)]), {1, 2}) == [
    {
        'type': 'too_long',
        'loc': (),
        'msg': 'Set should have at most 1 item after validation, not more',
        'input': {1, 2},
        'ctx': {'field_type': 'Set', 'max_length': 1, 'actual_length': None},
    }
]

# === Constraints reach through Optional and into list items; strict applies to the field alone ===
Opt = TypeAdapter(Annotated[int | None, Field(gt=0)])
assert Opt.validate_python(None) is None
assert errors(Opt, 0) == [
    {'type': 'greater_than', 'loc': (), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]
assert errors(TypeAdapter(list[Annotated[int, Field(gt=0)]]), [1, 0]) == [
    {'type': 'greater_than', 'loc': (1,), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]
StrictList = TypeAdapter(Annotated[list[int], Field(strict=True)])
assert StrictList.validate_python(['1']) == [1], 'the items stay lax'
assert errors(StrictList, (1,)) == [
    {'type': 'list_type', 'loc': (), 'msg': 'Input should be a valid list', 'input': (1,)}
]
assert errors(TypeAdapter(Annotated[int, Field(strict=True)]), '1') == [
    {'type': 'int_type', 'loc': (), 'msg': 'Input should be a valid integer', 'input': '1'}
]
# the error title names the constrained type
try:
    Bounded.validate_python('x')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'constrained-int'
try:
    Opt.validate_python('x')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'nullable[constrained-int]'
try:
    Items.validate_python('x')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'list[int]'

# === ConfigDict on a TypeAdapter ===
assert errors(TypeAdapter(int, config=ConfigDict(strict=True)), '1') == [
    {'type': 'int_type', 'loc': (), 'msg': 'Input should be a valid integer', 'input': '1'}
]
# the call's strict and a field's strict both override the config
assert TypeAdapter(int, config=ConfigDict(strict=True)).validate_python('3', strict=False) == 3
assert TypeAdapter(Annotated[int, Field(strict=False)], config=ConfigDict(strict=True)).validate_python('3') == 3
Lowered = TypeAdapter(str, config=ConfigDict(str_to_lower=True, str_strip_whitespace=True))
assert Lowered.validate_python('  AbC ') == 'abc'
assert Lowered.validate_json('" X "') == 'x'
assert TypeAdapter(str, config=ConfigDict(str_to_upper=True)).validate_python('ab') == 'AB'
assert errors(TypeAdapter(str, config=ConfigDict(str_min_length=3)), 'ab') == [
    {
        'type': 'string_too_short',
        'loc': (),
        'msg': 'String should have at least 3 characters',
        'input': 'ab',
        'ctx': {'min_length': 3},
    }
]
# a field's length bound beats the config's
assert (
    TypeAdapter(Annotated[str, Field(max_length=5)], config=ConfigDict(str_max_length=2)).validate_python('abcd')
    == 'abcd'
)
Coerced = TypeAdapter(str, config=ConfigDict(coerce_numbers_to_str=True))
assert Coerced.validate_python(12) == '12'
assert Coerced.validate_python(1.5) == '1.5'
assert Coerced.validate_python(Decimal('1.50')) == '1.50'
assert Coerced.validate_json('12') == '12'
assert errors(Coerced, True) == [
    {'type': 'string_type', 'loc': (), 'msg': 'Input should be a valid string', 'input': True}
]
try:
    TypeAdapter(int, config=ConfigDict(title='Thing')).validate_python('x')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'Thing'
    assert str(exc).startswith('1 validation error for Thing\n')
assert TypeAdapter(int, config=ConfigDict(unknown_key=1)).validate_python(1) == 1, 'an unknown key is ignored'


# === Field() as a dataclass default ===
@dataclass
class Order:
    id: int = Field(default=1, gt=0)
    customer: str = Field(alias='Customer', default='x', serialization_alias='Cust')
    internal: int = Field(default=0, exclude=True)
    tags: list[str] = Field(default_factory=list, max_length=2)


OrderAdapter = TypeAdapter(Order)
assert OrderAdapter.validate_python({}) == Order(1, 'x', 0, [])
assert OrderAdapter.validate_python({'Customer': 'y', 'id': '5'}) == Order(5, 'y', 0, [])
assert OrderAdapter.validate_python({'customer': 'y'}) == Order(1, 'x', 0, []), (
    'without populate_by_name the name is ignored'
)
assert errors(OrderAdapter, {'id': 0}) == [
    {'type': 'greater_than', 'loc': ('id',), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]
assert errors(OrderAdapter, {'Customer': 1}) == [
    {'type': 'string_type', 'loc': ('Customer',), 'msg': 'Input should be a valid string', 'input': 1}
]
assert errors(OrderAdapter, {'tags': ['a', 'b', 'c']}) == [
    {
        'type': 'too_long',
        'loc': ('tags',),
        'msg': 'List should have at most 2 items after validation, not 3',
        'input': ['a', 'b', 'c'],
        'ctx': {'field_type': 'List', 'max_length': 2, 'actual_length': 3},
    }
]
assert OrderAdapter.validate_json('{"Customer": "z", "tags": ["a"]}') == Order(1, 'z', 0, ['a'])
order = OrderAdapter.validate_python({'id': 2})
assert OrderAdapter.dump_python(order) == {'id': 2, 'customer': 'x', 'tags': []}
assert OrderAdapter.dump_python(order, by_alias=True) == {'id': 2, 'Cust': 'x', 'tags': []}
assert OrderAdapter.dump_json(order, by_alias=True) == b'{"id":2,"Cust":"x","tags":[]}'
assert OrderAdapter.dump_python(order, mode='json') == {'id': 2, 'customer': 'x', 'tags': []}
# a dataclass carries its own config, so the adapter refuses one
try:
    TypeAdapter(Order, config=ConfigDict(strict=True))
    assert False, 'expected a PydanticUserError'
except Exception as exc:
    assert type(exc).__name__ == 'PydanticUserError'
    assert str(exc).startswith('Cannot use `config` when the type is a BaseModel, dataclass or TypedDict.')


@dataclass
class Named:
    a: int = Field(alias='A')
    b: Annotated[int, Field(gt=0)] = Field(le=10)
    __pydantic_config__ = ConfigDict(populate_by_name=True)


NamedAdapter = TypeAdapter(Named)
assert NamedAdapter.validate_python({'A': 1, 'b': 5}) == Named(1, 5)
assert NamedAdapter.validate_python({'a': 1, 'b': 5}) == Named(1, 5)
assert NamedAdapter.validate_python({'a': 1, 'A': 2, 'b': 5}) == Named(2, 5), 'the alias wins over the name'
assert NamedAdapter.validate_python({'A': 2, 'a': 1, 'b': 5}) == Named(2, 5)
assert errors(NamedAdapter, {'b': 5}) == [
    {'type': 'missing', 'loc': ('A',), 'msg': 'Field required', 'input': {'b': 5}}
]
# a Field on the default and one in the hint both apply
assert errors(NamedAdapter, {'A': 1, 'b': 0}) == [
    {'type': 'greater_than', 'loc': ('b',), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]
assert errors(NamedAdapter, {'A': 1, 'b': 11}) == [
    {
        'type': 'less_than_equal',
        'loc': ('b',),
        'msg': 'Input should be less than or equal to 10',
        'input': 11,
        'ctx': {'le': 10},
    }
]


@dataclass
class Closed:
    a: int = 0
    __pydantic_config__ = ConfigDict(extra='forbid')


assert TypeAdapter(Closed).validate_python({'a': 1}) == Closed(1)
assert errors(TypeAdapter(Closed), {'a': 1, 'zz': 2, 'yy': 3}) == [
    {'type': 'unexpected_keyword_argument', 'loc': ('zz',), 'msg': 'Unexpected keyword argument', 'input': 2},
    {'type': 'unexpected_keyword_argument', 'loc': ('yy',), 'msg': 'Unexpected keyword argument', 'input': 3},
]
assert json_errors(TypeAdapter(Closed), '{"zz": 2}') == [
    {'type': 'unexpected_keyword_argument', 'loc': ('zz',), 'msg': 'Unexpected keyword argument', 'input': 2}
]


@dataclass
class Loose:
    a: int = 0
    __pydantic_config__ = ConfigDict(extra='allow')


assert TypeAdapter(Loose).validate_python({'a': 1, 'z': 2}) == Loose(1), 'a dataclass has nowhere to keep extras'


@dataclass
class Shouting:
    name: str
    tags: list[str] = field(default_factory=list)
    __pydantic_config__ = ConfigDict(str_to_upper=True)


assert TypeAdapter(Shouting).validate_python({'name': 'ab', 'tags': ['c']}) == Shouting('AB', ['C'])
assert TypeAdapter(list[Shouting]).validate_python([{'name': 'x'}]) == [Shouting('X', [])], (
    'the class config applies when nested'
)


@dataclass
class Exact:
    name: str
    __pydantic_config__ = ConfigDict(strict=True)


# a strict dataclass accepts only an instance from Python, and a lax `str` is rejected under it
assert TypeAdapter(Exact).validate_python(Exact('x')) == Exact('x')
assert TypeAdapter(Exact).validate_json('{"name": "x"}') == Exact('x')
assert errors(TypeAdapter(Exact), {'name': 'x'}) == [
    {
        'type': 'dataclass_exact_type',
        'loc': (),
        'msg': 'Input should be an instance of Exact',
        'input': {'name': 'x'},
        'ctx': {'class_name': 'Exact'},
    }
]
assert errors(TypeAdapter(list[Exact]), [{'name': 'x'}]) == [
    {
        'type': 'dataclass_exact_type',
        'loc': (0,),
        'msg': 'Input should be an instance of Exact',
        'input': {'name': 'x'},
        'ctx': {'class_name': 'Exact'},
    }
]
assert json_errors(TypeAdapter(Exact), '{"name": 1}') == [
    {'type': 'string_type', 'loc': ('name',), 'msg': 'Input should be a valid string', 'input': 1}
]
