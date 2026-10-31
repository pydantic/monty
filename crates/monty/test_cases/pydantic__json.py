# /// monty
# cpython-site-packages = true
# ///
from collections import namedtuple
from dataclasses import dataclass, field
from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal
from typing import Any, Literal

from pydantic import TypeAdapter, ValidationError


def errors(adapter, value, **kwargs):
    try:
        adapter.validate_json(value, **kwargs)
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


def invalid(adapter, value):
    error = errors(adapter, value)
    assert len(error) == 1
    assert error[0]['type'] == 'json_invalid'
    assert error[0]['input'] == value
    assert error[0]['msg'] == 'Invalid JSON: ' + error[0]['ctx']['error']
    return error[0]['ctx']['error']


UTC = timezone.utc

# === The document ===
Int = TypeAdapter(int)
assert Int.validate_json('1') == 1
assert Int.validate_json(' 1 ') == 1
assert Int.validate_json('\n1\n') == 1
assert Int.validate_json(b'2') == 2
assert Int.validate_json('123456789012345678901234567890') == 123456789012345678901234567890
assert errors(Int, 1) == [
    {'type': 'json_type', 'loc': (), 'msg': 'JSON input should be string, bytes or bytearray', 'input': 1}
]
assert errors(Int, None)[0]['type'] == 'json_type'
assert errors(Int, [1])[0]['type'] == 'json_type'
try:
    Int.validate_json(1)
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert str(exc) == (
        '1 validation error for int\n'
        '  JSON input should be string, bytes or bytearray [type=json_type, input_value=1, input_type=int]\n'
        '    For further information visit https://errors.pydantic.dev/2.13/v/json_type'
    )

# === Documents that are not JSON ===
assert invalid(Int, '') == 'EOF while parsing a value at line 1 column 0'
assert invalid(Int, '  ') == 'EOF while parsing a value at line 1 column 2'
assert invalid(Int, 'x') == 'expected value at line 1 column 1'
assert invalid(Int, '1 2') == 'trailing characters at line 1 column 3'
assert invalid(Int, '1\x00') == 'trailing characters at line 1 column 2'
assert invalid(Int, '[1,') == 'EOF while parsing a value at line 1 column 3'
assert invalid(Int, '{"a": }') == 'expected value at line 1 column 7'
assert invalid(Int, '"abc') == 'EOF while parsing a string at line 1 column 4'
assert invalid(Int, 'nul') == 'EOF while parsing a value at line 1 column 3'
assert invalid(Int, '1.') == 'EOF while parsing a value at line 1 column 2'
assert invalid(Int, '01') == 'invalid number at line 1 column 2'
assert invalid(Int, '[1 2]') == 'expected `,` or `]` at line 1 column 4'
assert invalid(Int, '{"a" 1}') == 'expected `:` at line 1 column 6'
assert invalid(Int, '{1: 2}') == 'key must be a string at line 1 column 2'
assert invalid(Int, "'a'") == 'expected value at line 1 column 1'
assert invalid(Int, '[1,]') == 'trailing comma at line 1 column 4'
assert invalid(Int, '"\\ud800"') == 'unexpected end of hex escape at line 1 column 8'
assert invalid(Int, '"\x01"') == 'control character (\\u0000-\\u001F) found while parsing a string at line 1 column 2'
assert invalid(Int, '﻿1') == 'expected value at line 1 column 1'
assert invalid(Int, b'\xff') == 'expected value at line 1 column 1'
assert invalid(Int, b'\xef\xbb\xbf1') == 'expected value at line 1 column 1'
assert invalid(TypeAdapter(str), b'"\xff"') == 'invalid unicode code point at line 1 column 3'
assert TypeAdapter(str).validate_json(b'"\xc3\xa9"') == 'é'
try:
    Int.validate_json('x')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert str(exc) == (
        '1 validation error for int\n'
        "  Invalid JSON: expected value at line 1 column 1 [type=json_invalid, input_value='x', input_type=str]\n"
        '    For further information visit https://errors.pydantic.dev/2.13/v/json_invalid'
    )
# the whole document is parsed: a syntax error after the failing value wins
List = TypeAdapter(list[int])
assert invalid(List, '[1, 2') == 'EOF while parsing a list at line 1 column 5'
assert invalid(List, '[1, 2] x') == 'trailing characters at line 1 column 8'
assert invalid(List, '["x", 2') == 'EOF while parsing a list at line 1 column 7'
assert invalid(TypeAdapter(int | str), '[1, 2') == 'EOF while parsing a list at line 1 column 5'
assert invalid(TypeAdapter(list[int | str]), '[1, [2, 3') == 'EOF while parsing a list at line 1 column 9'
assert invalid(TypeAdapter(Any), '{"a": 1') == 'EOF while parsing an object at line 1 column 7'
assert invalid(TypeAdapter(str), '"abc') == 'EOF while parsing a string at line 1 column 4'


# nesting beyond 200 non-empty containers is a parse error, reported at the first item inside
def depth(value):
    count = 0
    while isinstance(value, list) and value:
        value = value[0]
        count += 1
    return count


deep = '[' * 201 + ']' * 201
assert depth(TypeAdapter(Any).validate_json(deep)) == 200
assert depth(TypeAdapter(list[Any]).validate_json(deep)) == 200
assert errors(Int, deep)[0]['type'] == 'int_type'
assert invalid(TypeAdapter(Any), '[' * 201 + '1' + ']' * 201) == 'recursion limit exceeded at line 1 column 202'
assert invalid(Int, '[' * 201 + '1' + ']' * 201) == 'recursion limit exceeded at line 1 column 202'
assert invalid(TypeAdapter(list[list[Any]] | int), '[ ' * 201 + '1' + ']' * 201) == (
    'recursion limit exceeded at line 1 column 403'
)
assert invalid(TypeAdapter(Any), '{"a":' * 201 + '1' + '}' * 201) == 'recursion limit exceeded at line 1 column 1006'
assert depth(TypeAdapter(Any).validate_json('[' * 200 + '1' + ']' * 200)) == 200

# === Scalars: JSON mode coercions ===
assert Int.validate_json('"3"') == 3
assert Int.validate_json('" 1 "') == 1
assert Int.validate_json('"1_000"') == 1000
assert Int.validate_json('1.0') == 1
assert Int.validate_json('1E2') == 100
assert Int.validate_json('-0') == 0
assert Int.validate_json('true') == 1
assert errors(Int, '"x"') == [
    {
        'type': 'int_parsing',
        'loc': (),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    }
]
assert errors(Int, '"1e2"')[0]['type'] == 'int_parsing'
assert errors(Int, '1.5') == [
    {
        'type': 'int_from_float',
        'loc': (),
        'msg': 'Input should be a valid integer, got a number with a fractional part',
        'input': 1.5,
    }
]
assert errors(Int, '1.5e0')[0]['type'] == 'int_from_float'
assert errors(Int, 'NaN')[0]['type'] == 'finite_number'
assert errors(Int, 'Infinity')[0]['type'] == 'finite_number'
assert errors(Int, 'null') == [{'type': 'int_type', 'loc': (), 'msg': 'Input should be a valid integer', 'input': None}]
assert errors(Int, '[1]')[0] == {'type': 'int_type', 'loc': (), 'msg': 'Input should be a valid integer', 'input': [1]}
assert errors(Int, '{"a": 1}')[0]['input'] == {'a': 1}
assert errors(Int, '"' + 'a' * 60 + '"')[0]['input'] == 'a' * 60
assert errors(Int, '"1"', strict=True)[0]['type'] == 'int_type'
assert errors(Int, '1.0', strict=True)[0]['type'] == 'int_type'
assert errors(Int, 'true', strict=True)[0]['type'] == 'int_type'
assert Int.validate_json('1', strict=True) == 1
assert Int.validate_json('1', strict=None) == 1
assert Int.validate_json('1', strict=False) == 1

Float = TypeAdapter(float)
assert Float.validate_json('1') == 1.0
assert Float.validate_json('1.5') == 1.5
assert Float.validate_json('"1.5"') == 1.5
assert Float.validate_json('true') == 1.0
assert Float.validate_json('1e3') == 1000.0
assert Float.validate_json('1E400') == float('inf')
assert Float.validate_json('123456789012345678901234567890') == 1.2345678901234568e29
assert Float.validate_json('Infinity') == float('inf')
assert Float.validate_json('-Infinity') == float('-inf')
assert Float.validate_json('NaN') != Float.validate_json('NaN')
assert Float.validate_json('1', strict=True) == 1.0
assert errors(Float, '"x"') == [
    {
        'type': 'float_parsing',
        'loc': (),
        'msg': 'Input should be a valid number, unable to parse string as a number',
        'input': 'x',
    }
]
assert Float.validate_json('"nan"') != Float.validate_json('"nan"')
assert errors(Float, 'null') == [
    {'type': 'float_type', 'loc': (), 'msg': 'Input should be a valid number', 'input': None}
]
assert errors(Float, '"1.5"', strict=True)[0]['type'] == 'float_type'
assert errors(Float, 'true', strict=True)[0]['type'] == 'float_type'

Bool = TypeAdapter(bool)
assert Bool.validate_json('true') is True
assert Bool.validate_json('false') is False
assert Bool.validate_json('"yes"') is True
assert Bool.validate_json('"1"') is True
assert Bool.validate_json('1') is True
assert Bool.validate_json('0.0') is False
assert Bool.validate_json('1.0') is True
assert errors(Bool, '2') == [
    {
        'type': 'bool_parsing',
        'loc': (),
        'msg': 'Input should be a valid boolean, unable to interpret input',
        'input': 2,
    }
]
assert errors(Bool, '"x"')[0]['type'] == 'bool_parsing'
assert errors(Bool, '1.5') == [{'type': 'bool_type', 'loc': (), 'msg': 'Input should be a valid boolean', 'input': 1.5}]
assert errors(Bool, '0.5')[0]['type'] == 'bool_type'
assert errors(Bool, '1e400')[0]['type'] == 'bool_type'
assert errors(Bool, 'NaN')[0]['type'] == 'bool_type'
assert errors(Bool, 'null')[0]['type'] == 'bool_type'
assert errors(Bool, '123456789012345678901234567890')[0]['type'] == 'bool_type'
assert errors(Bool, '1', strict=True)[0]['type'] == 'bool_type'

Str = TypeAdapter(str)
assert Str.validate_json('"a"') == 'a'
assert Str.validate_json('"\\u00e9\\n"') == 'é\n'
assert Str.validate_json('"\\ud83d\\ude00"') == '😀'
assert errors(Str, '1') == [{'type': 'string_type', 'loc': (), 'msg': 'Input should be a valid string', 'input': 1}]
assert errors(Str, 'null')[0]['type'] == 'string_type'
assert errors(Str, '["a"]')[0]['input'] == ['a']

Bytes = TypeAdapter(bytes)
assert Bytes.validate_json('"abc"') == b'abc'
assert Bytes.validate_json('"\\u00e9"') == b'\xc3\xa9'
assert errors(Bytes, '1') == [{'type': 'bytes_type', 'loc': (), 'msg': 'Input should be a valid bytes', 'input': 1}]

NoneAdapter = TypeAdapter(None)
assert NoneAdapter.validate_json('null') is None
assert errors(NoneAdapter, '1') == [{'type': 'none_required', 'loc': (), 'msg': 'Input should be null', 'input': 1}]
assert errors(NoneAdapter, '"null"')[0]['type'] == 'none_required'
assert TypeAdapter(int | None).validate_json('null') is None
assert TypeAdapter(int | None).validate_json('"1"') == 1
assert errors(TypeAdapter(int | None), '"x"')[0]['type'] == 'int_parsing'

assert TypeAdapter(Any).validate_json('{"a": [1, 2.5, "x", null, true]}') == {'a': [1, 2.5, 'x', None, True]}
assert TypeAdapter(list[Any]).validate_json('[1, "a", {"b": [null]}]') == [1, 'a', {'b': [None]}]

# === Temporal types and Decimal ===
DateTime = TypeAdapter(datetime)
assert DateTime.validate_json('"2020-01-02T03:04:05Z"') == datetime(2020, 1, 2, 3, 4, 5, tzinfo=UTC)
assert DateTime.validate_json('"2020-01-02"') == datetime(2020, 1, 2)
assert DateTime.validate_json('1577836800') == datetime(2020, 1, 1, tzinfo=UTC)
assert DateTime.validate_json('1577836800.5') == datetime(2020, 1, 1, 0, 0, 0, 500000, tzinfo=UTC)
assert errors(DateTime, '"x"') == [
    {
        'type': 'datetime_from_date_parsing',
        'loc': (),
        'msg': 'Input should be a valid datetime or date, input is too short',
        'input': 'x',
        'ctx': {'error': 'input is too short'},
    }
]
assert errors(DateTime, 'true') == [
    {'type': 'datetime_type', 'loc': (), 'msg': 'Input should be a valid datetime', 'input': True}
]
assert errors(DateTime, 'NaN')[0]['msg'] == 'Input should be a valid datetime, NaN values not permitted'
assert errors(DateTime, '1577836800', strict=True)[0]['type'] == 'datetime_type'
assert errors(DateTime, '"2020-01-02"', strict=True) == [
    {
        'type': 'datetime_parsing',
        'loc': (),
        'msg': 'Input should be a valid datetime, invalid datetime separator, expected `T`, `t`, `_` or space',
        'input': '2020-01-02',
        'ctx': {'error': 'invalid datetime separator, expected `T`, `t`, `_` or space'},
    }
]
Date = TypeAdapter(date)
assert Date.validate_json('"2020-01-02"') == date(2020, 1, 2)
assert Date.validate_json('"2020-01-02T00:00:00"') == date(2020, 1, 2)
assert Date.validate_json('1577836800') == date(2020, 1, 1)
assert errors(Date, '"2020-01-02T01:00:00"')[0]['type'] == 'date_from_datetime_inexact'
assert errors(Date, '"x"')[0]['type'] == 'date_from_datetime_parsing'
assert errors(Date, 'null')[0]['type'] == 'date_type'
Time = TypeAdapter(time)
assert Time.validate_json('"01:02:03"') == time(1, 2, 3)
assert Time.validate_json('3600') == time(1, 0, tzinfo=UTC)
assert errors(Time, '"x"')[0]['type'] == 'time_parsing'
assert errors(Time, 'null')[0]['type'] == 'time_type'
TimeDelta = TypeAdapter(timedelta)
assert TimeDelta.validate_json('"P1DT1H"') == timedelta(days=1, hours=1)
assert TimeDelta.validate_json('90') == timedelta(seconds=90)
assert TimeDelta.validate_json('1.5') == timedelta(seconds=1, microseconds=500000)
assert errors(TimeDelta, '"x"')[0]['type'] == 'time_delta_parsing'
assert errors(TimeDelta, '1e400')[0]['type'] == 'time_delta_parsing'
assert errors(TimeDelta, '90', strict=True)[0]['type'] == 'time_delta_type'
Dec = TypeAdapter(Decimal)
assert Dec.validate_json('"1.5"') == Decimal('1.5')
assert Dec.validate_json('1.5') == Decimal('1.5')
assert Dec.validate_json('1') == Decimal('1')
assert Dec.validate_json('-0.0') == Decimal('-0.0')
# a float goes through its shortest text, so the exponent form is normalised
assert str(Dec.validate_json('1e2')) == '100'
assert str(Dec.validate_json('1.5e3')) == '1500'
assert str(Dec.validate_json('1e20')) == '100000000000000000000'
assert str(Dec.validate_json('1e-7')) == '1E-7'
assert str(Dec.validate_json('100.0')) == '100'
assert str(Dec.validate_json('0.1')) == '0.1'
assert Dec.validate_json('123456789012345678901234567890') == Decimal('123456789012345678901234567890')
assert errors(Dec, '"x"') == [
    {'type': 'decimal_parsing', 'loc': (), 'msg': 'Input should be a valid decimal', 'input': 'x'}
]
assert errors(Dec, '"inf"')[0]['type'] == 'finite_number'
assert errors(Dec, 'Infinity')[0]['type'] == 'finite_number'
assert errors(Dec, 'NaN')[0]['type'] == 'finite_number'
assert errors(Dec, 'null')[0]['type'] == 'decimal_type'
assert errors(Dec, 'true')[0]['type'] == 'decimal_type'

# === Containers ===
assert List.validate_json('[1, "2", 3.0, true]') == [1, 2, 3, 1]
assert List.validate_json('[]') == []
assert errors(List, '[1, "x", null]') == [
    {
        'type': 'int_parsing',
        'loc': (1,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    },
    {'type': 'int_type', 'loc': (2,), 'msg': 'Input should be a valid integer', 'input': None},
]
assert errors(List, '[1, "x", 2, "y"]')[1]['loc'] == (3,)
assert errors(List, '[1, [2]]')[0] == {
    'type': 'int_type',
    'loc': (1,),
    'msg': 'Input should be a valid integer',
    'input': [2],
}
assert errors(List, '{"a": 1}') == [
    {'type': 'list_type', 'loc': (), 'msg': 'Input should be a valid array', 'input': {'a': 1}}
]
assert errors(List, '"[1]"')[0]['type'] == 'list_type'
try:
    List.validate_json('[1, "x", null]')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.error_count() == 2
    assert str(exc) == (
        '2 validation errors for list[int]\n'
        '1\n'
        "  Input should be a valid integer, unable to parse string as an integer [type=int_parsing, input_value='x', input_type=str]\n"
        '    For further information visit https://errors.pydantic.dev/2.13/v/int_parsing\n'
        '2\n'
        '  Input should be a valid integer [type=int_type, input_value=None, input_type=NoneType]\n'
        '    For further information visit https://errors.pydantic.dev/2.13/v/int_type'
    )
assert TypeAdapter(list[list[int]]).validate_json('[[1], [2, 3]]') == [[1], [2, 3]]
Pair = TypeAdapter(tuple[int, str])
assert Pair.validate_json('[1, "a"]') == (1, 'a')
assert errors(Pair, '[1, "a", 2]') == [
    {
        'type': 'too_long',
        'loc': (),
        'msg': 'Tuple should have at most 2 items after validation, not 3',
        'input': [1, 'a', 2],
        'ctx': {'field_type': 'Tuple', 'max_length': 2, 'actual_length': 3},
    }
]
assert errors(Pair, '[1]') == [{'type': 'missing', 'loc': (1,), 'msg': 'Field required', 'input': [1]}]
assert errors(TypeAdapter(tuple[int, int, int]), '[1]')[1]['loc'] == (2,)
assert errors(TypeAdapter(list[tuple[int, str]]), '[[1, "a", 2], [1]]') == [
    {
        'type': 'too_long',
        'loc': (0,),
        'msg': 'Tuple should have at most 2 items after validation, not 3',
        'input': [1, 'a', 2],
        'ctx': {'field_type': 'Tuple', 'max_length': 2, 'actual_length': 3},
    },
    {'type': 'missing', 'loc': (1, 1), 'msg': 'Field required', 'input': [1]},
]
assert TypeAdapter(tuple[int, ...]).validate_json('[1, 2]') == (1, 2)
assert errors(TypeAdapter(tuple[int, ...]), '[1, "x", 2.5]')[0]['loc'] == (1,)
assert errors(TypeAdapter(tuple[int, ...]), '[1, "x", 2.5]')[1]['loc'] == (2,)
assert TypeAdapter(set[int]).validate_json('[1, "1", 1.0, 2]') == {1, 2}
assert TypeAdapter(frozenset[str]).validate_json('["a"]') == frozenset({'a'})
assert errors(TypeAdapter(tuple[int, ...]), '1')[0]['type'] == 'tuple_type'
assert errors(TypeAdapter(set[int]), '{}')[0]['type'] == 'set_type'
assert errors(TypeAdapter(frozenset[int]), '"a"')[0]['type'] == 'frozen_set_type'

Dict = TypeAdapter(dict[str, int])
assert Dict.validate_json('{"a": 1, "b": "2"}') == {'a': 1, 'b': 2}
assert Dict.validate_json('{}') == {}
assert Dict.validate_json('{"a\\nb": 1, "\\u00e9": 2}') == {'a\nb': 1, 'é': 2}
# JSON allows a key twice: the last value wins
assert Dict.validate_json('{"a": 1, "a": 2}') == {'a': 2}
assert errors(Dict, '{"a": "x", "a": 2}')[0]['loc'] == ('a',)
assert errors(Dict, '{"a": "x"}') == [
    {
        'type': 'int_parsing',
        'loc': ('a',),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    }
]
assert errors(Dict, '[1]') == [{'type': 'dict_type', 'loc': (), 'msg': 'Input should be an object', 'input': [1]}]
# keys are text, coerced by the key type in lax and strict mode alike
IntKeys = TypeAdapter(dict[int, list[int]])
assert IntKeys.validate_json('{"1": [1], "2": ["3"]}') == {1: [1], 2: [3]}
assert IntKeys.validate_json('{"1": [1]}', strict=True) == {1: [1]}
assert errors(IntKeys, '{"x": [1]}') == [
    {
        'type': 'int_parsing',
        'loc': ('x', '[key]'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    }
]
assert errors(IntKeys, '{"1": ["x"]}')[0]['loc'] == ('1', 0)
assert TypeAdapter(dict[bool, int]).validate_json('{"true": 1}') == {True: 1}
assert TypeAdapter(dict[float, int]).validate_json('{"1.5": 1}') == {1.5: 1}
assert TypeAdapter(dict[date, int]).validate_json('{"2020-01-01": 1}') == {date(2020, 1, 1): 1}
assert errors(TypeAdapter(dict[date, int]), '{"x": 1}')[0]['type'] == 'date_from_datetime_parsing'
assert errors(TypeAdapter(dict[str, list[dict[str, int]]]), '{"a": [{"b": 1}, {"c": "x"}]}')[0]['loc'] == ('a', 1, 'c')
assert errors(TypeAdapter(dict[str, tuple[int, str]]), '{"a": [1, "a", 2]}')[0]['loc'] == ('a',)

# === Unions: each member reads the same tokens; a JSON string is not an exact str ===
IntOrStr = TypeAdapter(int | str)
assert IntOrStr.validate_json('1') == 1
assert IntOrStr.validate_json('"1"') == '1'
assert IntOrStr.validate_json('"a"') == 'a'
assert IntOrStr.validate_json('1.0') == 1
assert errors(IntOrStr, 'null') == [
    {'type': 'int_type', 'loc': ('int',), 'msg': 'Input should be a valid integer', 'input': None},
    {'type': 'string_type', 'loc': ('str',), 'msg': 'Input should be a valid string', 'input': None},
]
assert errors(IntOrStr, '[1]')[1]['input'] == [1]
Mixed = TypeAdapter(list[int] | dict[str, int] | str)
assert Mixed.validate_json('[1, "2"]') == [1, 2]
assert Mixed.validate_json('{"a": 1}') == {'a': 1}
assert Mixed.validate_json('"x"') == 'x'
assert errors(Mixed, '[1, "x"]') == [
    {
        'type': 'int_parsing',
        'loc': ('list[int]', 1),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    },
    {'type': 'dict_type', 'loc': ('dict[str,int]',), 'msg': 'Input should be an object', 'input': [1, 'x']},
    {'type': 'string_type', 'loc': ('str',), 'msg': 'Input should be a valid string', 'input': [1, 'x']},
]
assert TypeAdapter(datetime | str).validate_json('"2020-01-01T00:00:00"') == datetime(2020, 1, 1)
assert TypeAdapter(datetime | str).validate_json('"hello"') == 'hello'
assert TypeAdapter(str | datetime).validate_json('"2020-01-01T00:00:00"') == '2020-01-01T00:00:00'
assert TypeAdapter(float | int).validate_json('1') == 1
assert type(TypeAdapter(float | int).validate_json('1')) is int
assert type(TypeAdapter(float | int).validate_json('1.0')) is float
assert TypeAdapter(bool | int).validate_json('1') == 1
assert type(TypeAdapter(bool | int).validate_json('1')) is int
assert TypeAdapter(bool | int).validate_json('true') is True

# === Literal ===
Lit = TypeAdapter(Literal['a', 1, True, None])
assert Lit.validate_json('"a"') == 'a'
assert Lit.validate_json('1') == 1
assert Lit.validate_json('true') is True
assert Lit.validate_json('null') is None
assert Lit.validate_json('1.0') is True
assert errors(Lit, '"b"') == [
    {
        'type': 'literal_error',
        'loc': (),
        'msg': "Input should be 'a', 1, True or None",
        'input': 'b',
        'ctx': {'expected': "'a', 1, True or None"},
    }
]
assert errors(Lit, '[1]')[0]['input'] == [1]
assert errors(TypeAdapter(Literal[1]), 'NaN')[0]['type'] == 'literal_error'


# === Dataclasses: fields arrive in document order, results and errors in field order ===
@dataclass
class Foo:
    x: int
    y: str = 'a'
    z: list[int] = field(default_factory=list)


FooAdapter = TypeAdapter(Foo)
assert FooAdapter.validate_json('{"x": 1}') == Foo(1)
assert FooAdapter.validate_json('{"z": [1, "2"], "y": "b", "x": "3", "q": 5}') == Foo(3, 'b', [1, 2])
assert FooAdapter.validate_json('{"x": 1}', strict=True) == Foo(1)
assert FooAdapter.validate_json('{"x": 1, "x": 2}') == Foo(2)
assert FooAdapter.validate_json('{"x": "bad", "x": 2}') == Foo(2)
assert errors(FooAdapter, '{"x": 2, "x": "bad"}')[0]['input'] == 'bad'
assert errors(FooAdapter, '{"y": 1}') == [
    {'type': 'missing', 'loc': ('x',), 'msg': 'Field required', 'input': {'y': 1}},
    {'type': 'string_type', 'loc': ('y',), 'msg': 'Input should be a valid string', 'input': 1},
]
assert errors(FooAdapter, '{}') == [{'type': 'missing', 'loc': ('x',), 'msg': 'Field required', 'input': {}}]
assert errors(FooAdapter, '{"x": "bad", "z": [1, "q"]}') == [
    {
        'type': 'int_parsing',
        'loc': ('x',),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'bad',
    },
    {
        'type': 'int_parsing',
        'loc': ('z', 1),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'q',
    },
]
assert errors(FooAdapter, '[1]') == [
    {
        'type': 'dataclass_type',
        'loc': (),
        'msg': 'Input should be an object',
        'input': [1],
        'ctx': {'class_name': 'Foo'},
    }
]
assert errors(FooAdapter, 'null')[0]['type'] == 'dataclass_type'
assert invalid(FooAdapter, '{"x": 1, "y": "a", "z": [1, 2}') == 'expected `,` or `]` at line 1 column 30'
assert invalid(FooAdapter, '{"y": 5') == 'EOF while parsing an object at line 1 column 7'
try:
    FooAdapter.validate_json('{}')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert str(exc) == (
        '1 validation error for Foo\nx\n  Field required [type=missing, input_value={}, input_type=dict]\n'
        '    For further information visit https://errors.pydantic.dev/2.13/v/missing'
    )


@dataclass
class Node:
    value: int
    children: list['Node'] = field(default_factory=list)


NodeAdapter = TypeAdapter(Node)
assert NodeAdapter.validate_json('{"value": 1, "children": [{"value": 2, "children": [{"value": "3"}]}]}') == Node(
    1, [Node(2, [Node(3)])]
)
assert errors(NodeAdapter, '{"value": 1, "children": [{"value": "x"}]}')[0]['loc'] == ('children', 0, 'value')
assert TypeAdapter(list[Foo]).validate_json('[{"x": 1, "y": "b"}, {"x": "2"}]') == [Foo(1, 'b'), Foo(2)]
assert TypeAdapter(Foo | int).validate_json('{"x": 1}') == Foo(1)
assert TypeAdapter(Foo | int).validate_json('"3"') == 3
assert errors(TypeAdapter(Foo | int), '{"x": "q"}') == [
    {
        'type': 'int_parsing',
        'loc': ('Foo', 'x'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'q',
    },
    {'type': 'int_type', 'loc': ('int',), 'msg': 'Input should be a valid integer', 'input': {'x': 'q'}},
]
assert TypeAdapter(list[Foo | int]).validate_json('[{"x": 1}, 2, {"x": "3"}]') == [Foo(1), 2, Foo(3)]
assert errors(TypeAdapter(list[Foo | int]), '[{"x": 1}, {"x": "q"}, "z", 4]') == [
    {
        'type': 'int_parsing',
        'loc': (1, 'Foo', 'x'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'q',
    },
    {'type': 'int_type', 'loc': (1, 'int'), 'msg': 'Input should be a valid integer', 'input': {'x': 'q'}},
    {
        'type': 'dataclass_type',
        'loc': (2, 'Foo'),
        'msg': 'Input should be an object',
        'input': 'z',
        'ctx': {'class_name': 'Foo'},
    },
    {
        'type': 'int_parsing',
        'loc': (2, 'int'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'z',
    },
]

# === namedtuple ===
Point = namedtuple('Point', ['x', 'y'])
PointAdapter = TypeAdapter(Point)
assert PointAdapter.validate_json('[1, 2]') == Point(1, 2)
assert PointAdapter.validate_json('{"x": 1, "y": 2}') == Point(1, 2)
assert errors(PointAdapter, '[1]') == [
    {'type': 'missing_argument', 'loc': ('y',), 'msg': 'Missing required argument', 'input': [1]}
]
assert errors(PointAdapter, '1') == [
    {'type': 'arguments_type', 'loc': (), 'msg': 'Arguments must be an array or an object', 'input': 1}
]
assert errors(PointAdapter, '[1, 2, 3]') == [
    {'type': 'unexpected_positional_argument', 'loc': (2,), 'msg': 'Unexpected positional argument', 'input': 3}
]
assert errors(PointAdapter, '{"x": 1, "z": 2}') == [
    {'type': 'missing_argument', 'loc': ('y',), 'msg': 'Missing required argument', 'input': {'x': 1, 'z': 2}},
    {'type': 'unexpected_keyword_argument', 'loc': ('z',), 'msg': 'Unexpected keyword argument', 'input': 2},
]

# === Arguments ===
try:
    Int.validate_json()
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "TypeAdapter.validate_json() missing 1 required positional argument: 'data'"
try:
    Int.validate_json('1', strict='x')
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'strict': 'str' object is not an instance of 'bool'"
try:
    Int.validate_json('1', strict=1)
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'strict': 'int' object is not an instance of 'bool'"
try:
    Int.validate_json('1', foo=1)
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "TypeAdapter.validate_json() got an unexpected keyword argument 'foo'"
