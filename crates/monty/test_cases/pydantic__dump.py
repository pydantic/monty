# /// monty
# cpython-site-packages = true
# ///
from collections import namedtuple
from dataclasses import dataclass, field
from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal
from typing import Any, Literal

from pydantic import TypeAdapter

Any_ = TypeAdapter(Any)


def error(f):
    try:
        f()
    except ValueError as exc:
        return str(exc)
    assert False, 'expected an error'


# === Python mode keeps values and container types ===
items = [1, 'a', None, 2.5, True]
assert Any_.dump_python(items) == items
assert Any_.dump_python(items) is not items
assert Any_.dump_python((1, 2)) == (1, 2)
assert Any_.dump_python({1}) == {1}
assert Any_.dump_python(frozenset({1})) == frozenset({1})
assert type(Any_.dump_python(frozenset({1}))) is frozenset
assert Any_.dump_python(b'ab') == b'ab'
assert Any_.dump_python(Decimal('1.50')) == Decimal('1.50')
assert Any_.dump_python(datetime(2020, 1, 1, tzinfo=timezone.utc)) == datetime(2020, 1, 1, tzinfo=timezone.utc)
assert Any_.dump_python(float('inf')) == float('inf')
assert Any_.dump_python(10**30) == 10**30
assert TypeAdapter(int).dump_python(1, mode='xml') == 1
text = 'hello'
assert TypeAdapter(str).dump_python(text) is text


class Plain:
    pass


plain = Plain()
assert Any_.dump_python(plain) is plain
assert Any_.dump_python(len) is len

# === JSON mode: JSON-compatible Python objects ===
assert Any_.dump_python((1, 2), mode='json') == [1, 2]
assert Any_.dump_python({1}, mode='json') == [1]
assert Any_.dump_python(frozenset({'a'}), mode='json') == ['a']
assert Any_.dump_python(b'ab', mode='json') == 'ab'
# a non-finite float is `None` by inference and itself under a `float` hint
assert Any_.dump_python(float('inf'), mode='json') is None
assert Any_.dump_python([float('nan')], mode='json') == [None]
assert TypeAdapter(float).dump_python(float('inf'), mode='json') == float('inf')
assert TypeAdapter(list[float]).dump_python([float('inf')], mode='json') == [float('inf')]
assert TypeAdapter(float | int).dump_python(float('inf'), mode='json') == float('inf')
assert Any_.dump_python(10**30, mode='json') == 10**30
assert Any_.dump_python(datetime(2020, 1, 1), mode='json') == '2020-01-01T00:00:00'
assert (
    TypeAdapter(datetime).dump_python(
        datetime(2020, 1, 1, 1, 2, 3, 4, tzinfo=timezone(timedelta(hours=-5, minutes=-30))), mode='json'
    )
    == '2020-01-01T01:02:03.000004-05:30'
)
assert Any_.dump_python(date(2020, 1, 1), mode='json') == '2020-01-01'
assert Any_.dump_python(time(1, 2, 3, 4, tzinfo=timezone.utc), mode='json') == '01:02:03.000004Z'
assert Any_.dump_python(
    [timedelta(days=400, hours=1, microseconds=5), timedelta(0), timedelta(seconds=-1), timedelta(days=-1, seconds=1)],
    mode='json',
) == ['P1Y35DT1H0.000005S', 'PT0S', '-PT1S', '-PT23H59M59S']
assert Any_.dump_python(
    [Decimal('1.50'), Decimal('1E+1'), Decimal('-0'), Decimal('NaN'), Decimal('Infinity')], mode='json'
) == ['1.50', '1E+1', '-0', 'NaN', 'Infinity']
assert error(lambda: Any_.dump_python(b'\xff', mode='json')) == (
    "'utf-8' codec can't decode byte 0xff in position 0: invalid utf-8"
)
# the class in the message is `<class '__main__.Plain'>` on pydantic and `<class 'Plain'>` on Monty
assert error(lambda: Any_.dump_python(plain, mode='json')).startswith('Unable to serialize unknown type: <class ')
assert error(lambda: Any_.dump_python([plain], mode='json')).endswith("Plain'>")
assert error(lambda: Any_.dump_python(int, mode='json')) == "Unable to serialize unknown type: <class 'type'>"
assert error(lambda: Any_.dump_python(len, mode='json')) == (
    "Unable to serialize unknown type: <class 'builtin_function_or_method'>"
)

# === JSON mode keys are strings ===
assert Any_.dump_python(
    {1: 1, None: 3, 1.5: 4, 'a': 5, b'b': 6, date(2020, 1, 1): 7, Decimal('1.5'): 8, (1, 2): 9}, mode='json'
) == {'1': 1, 'None': 3, '1.5': 4, 'a': 5, 'b': 6, '2020-01-01': 7, '1.5': 8, '1,2': 9}
assert Any_.dump_python({True: 1, False: 2}, mode='json') == {'true': 1, 'false': 2}
assert Any_.dump_python({float('inf'): 1}, mode='json') == {'None': 1}
assert TypeAdapter(dict[float, int]).dump_python({float('inf'): 1}, mode='json') == {'inf': 1}
assert TypeAdapter(dict[float, int]).dump_json({float('nan'): 1, float('-inf'): 2}) == b'{"nan":1,"-inf":2}'
assert TypeAdapter(dict[int, int]).dump_python({1: 1}, mode='json') == {'1': 1}
assert TypeAdapter(dict[float, int]).dump_python({1.0: 1, 2.5: 2}, mode='json') == {'1.0': 1, '2.5': 2}
assert TypeAdapter(dict[int, int]).dump_python({1: 1}) == {1: 1}
assert TypeAdapter(dict[date, int]).dump_python({date(2020, 1, 1): 1}) == {date(2020, 1, 1): 1}

# === dump_json ===
assert Any_.dump_json(1) == b'1'
assert Any_.dump_json([1.5e300, 1e-7, 123456789.0, 0.1 + 0.2, -0.0, 1e21, 1e16, 1e15]) == (
    b'[1.5e+300,1e-7,123456789.0,0.30000000000000004,-0.0,1e+21,1e+16,1000000000000000.0]'
)
assert Any_.dump_json([float('nan'), float('inf'), float('-inf')]) == b'[null,null,null]'
assert Any_.dump_json(10**30) == b'1000000000000000000000000000000'
assert Any_.dump_json({'a': [1, None, True, 'x']}) == b'{"a":[1,null,true,"x"]}'
assert Any_.dump_json(date(2020, 1, 1)) == b'"2020-01-01"'
assert Any_.dump_json(b'\xc3\xa9') == b'"\xc3\xa9"'
assert Any_.dump_json('é\n\x01"\\') == b'"\xc3\xa9\\n\\u0001\\"\\\\"'
assert Any_.dump_json('é\n\x01"\\', ensure_ascii=True) == b'"\\u00e9\\n\\u0001\\"\\\\"'
assert Any_.dump_json('😀', ensure_ascii=True) == b'"\\ud83d\\ude00"'
assert Any_.dump_json({'é': '\x7f\u2028'}, ensure_ascii=True) == b'{"\\u00e9":"\x7f\\u2028"}'
assert Any_.dump_json({'é': '\x7f\u2028'}) == b'{"\xc3\xa9":"\x7f\xe2\x80\xa8"}'
assert Any_.dump_json([Decimal('1.50'), Decimal('NaN')]) == b'["1.50","NaN"]'
assert Any_.dump_json({1: 1, None: 3, 1.5: 4, b'b': 6, date(2020, 1, 1): 7}) == (
    b'{"1":1,"None":3,"1.5":4,"b":6,"2020-01-01":7}'
)
assert Any_.dump_json({(1, 2): 9}) == b'{"1,2":9}'
assert TypeAdapter(dict[bool, int]).dump_json({True: 1}) == b'{"true":1}'
assert TypeAdapter(set[int]).dump_json({1}) == b'[1]'
assert TypeAdapter(list[int]).dump_json([1, [2]], indent=4) == b'[\n    1,\n    [\n        2\n    ]\n]'
assert Any_.dump_json([[], {}], indent=2) == b'[\n  [],\n  {}\n]'
assert Any_.dump_json([1], indent=0) == b'[\n1\n]'
assert error(lambda: Any_.dump_json(b'\xff')) == (
    'Error serializing to JSON: invalid utf-8 sequence of 1 bytes from index 0'
)
assert error(lambda: Any_.dump_json(plain)).endswith("Plain'>")
cyclic = []
cyclic.append(cyclic)
assert error(lambda: Any_.dump_json(cyclic)) == (
    'Error serializing to JSON: ValueError: Circular reference detected (id repeated)'
)
try:
    Any_.dump_json([1], indent='x')
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'indent': 'str' object cannot be interpreted as an integer"
try:
    Any_.dump_json([1], indent=-1)
    assert False, 'expected an OverflowError'
except OverflowError as exc:
    assert str(exc) == "can't convert negative int to unsigned"


# === Dataclasses: a mapping of fields, in field order ===
@dataclass
class Foo:
    x: int
    y: str | None = None
    z: list[int] = field(default_factory=list)


FooAdapter = TypeAdapter(Foo)
assert FooAdapter.dump_python(Foo(1, z=[1])) == {'x': 1, 'y': None, 'z': [1]}
assert FooAdapter.dump_python(Foo(1), mode='json') == {'x': 1, 'y': None, 'z': []}
assert FooAdapter.dump_json(Foo(1, z=[1])) == b'{"x":1,"y":null,"z":[1]}'
assert FooAdapter.dump_json(Foo(1, z=[1]), indent=2) == b'{\n  "x": 1,\n  "y": null,\n  "z": [\n    1\n  ]\n}'
assert Any_.dump_python(Foo(1)) == {'x': 1, 'y': None, 'z': []}
assert Any_.dump_json(Foo(1)) == b'{"x":1,"y":null,"z":[]}'
assert TypeAdapter(list[Foo]).dump_python([Foo(1), Foo(2, 'b')]) == [
    {'x': 1, 'y': None, 'z': []},
    {'x': 2, 'y': 'b', 'z': []},
]
assert TypeAdapter(Foo | int).dump_python(Foo(1)) == {'x': 1, 'y': None, 'z': []}
assert TypeAdapter(Foo | int).dump_python(3) == 3
assert TypeAdapter(Foo | int).dump_json(Foo(1)) == b'{"x":1,"y":null,"z":[]}'
# a value of another type than the field's falls back to its own type
assert FooAdapter.dump_python({'x': 1}) == {'x': 1}
assert FooAdapter.dump_json({'x': 1}) == b'{"x":1}'
assert TypeAdapter(int).dump_python('x') == 'x'
assert TypeAdapter(int).dump_python('x', warnings=False) == 'x'
assert TypeAdapter(int).dump_json('x') == b'"x"'
assert TypeAdapter(int).dump_python(True) is True
assert TypeAdapter(float).dump_python(1) == 1
assert TypeAdapter(None).dump_python(1) == 1
assert TypeAdapter(list[int]).dump_python((1, 2)) == (1, 2)
assert TypeAdapter(int | str).dump_python(1.5) == 1.5
assert TypeAdapter(int | None).dump_python(None) is None
assert TypeAdapter(Literal['a', 1]).dump_python('a') == 'a'
assert TypeAdapter(Literal['a', 1]).dump_python('b') == 'b'
assert TypeAdapter(Literal['a', 1]).dump_json(1) == b'1'
assert TypeAdapter(int).dump_python(1, round_trip=True) == 1
assert TypeAdapter(int).dump_python(1, by_alias=True) == 1

# === exclude_none applies to a dataclass's typed fields only ===
assert FooAdapter.dump_python(Foo(1), exclude_none=True) == {'x': 1, 'z': []}
assert FooAdapter.dump_json(Foo(1), exclude_none=True) == b'{"x":1,"z":[]}'
assert TypeAdapter(list[Foo]).dump_python([Foo(1)], exclude_none=True) == [{'x': 1, 'z': []}]
assert Any_.dump_python(Foo(1), exclude_none=True) == {'x': 1, 'y': None, 'z': []}
assert TypeAdapter(dict[str, Any]).dump_python({'a': None, 'b': 1}, exclude_none=True) == {'a': None, 'b': 1}
assert TypeAdapter(list[Any]).dump_python([None, 1], exclude_none=True) == [None, 1]
assert Any_.dump_python({'a': {'b': None}, 'c': [None]}, exclude_none=True) == {'a': {'b': None}, 'c': [None]}

# === namedtuple and deque ===
Point = namedtuple('Point', ['x', 'y'])
assert Any_.dump_python(Point(1, 2)) == (1, 2)
assert type(Any_.dump_python(Point(1, 2))) is tuple
assert TypeAdapter(Point).dump_python(Point(1, 2)) == (1, 2)
assert TypeAdapter(Point).dump_python(Point(1, 2), mode='json') == [1, 2]
assert TypeAdapter(Point).dump_json(Point(1, 2)) == b'[1,2]'
assert TypeAdapter(Point).dump_python(Point(1, None), exclude_none=True) == (1, None)

# === Arguments ===
try:
    Any_.dump_python(1, exclude_none=1)
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'exclude_none': 'int' object is not an instance of 'bool'"
try:
    Any_.dump_json(1, ensure_ascii=1)
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'ensure_ascii': 'int' object is not an instance of 'bool'"
try:
    Any_.dump_json(1, round_trip=3)
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'round_trip': 'int' object is not an instance of 'bool'"
try:
    Any_.dump_python(1, mode=3)
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'mode': 'int' object is not an instance of 'str'"
try:
    Any_.dump_json()
    assert False, 'expected a TypeError'
except TypeError as exc:
    assert str(exc) == "TypeAdapter.dump_json() missing 1 required positional argument: 'instance'"
