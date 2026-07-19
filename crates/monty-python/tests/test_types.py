from __future__ import annotations

import datetime
import pathlib
import re
import zoneinfo
from typing import NamedTuple

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import MontyConversionError, MontyRuntimeError


def test_none_input(monty_run: RunMonty):
    assert monty_run('x is None', inputs={'x': None}) is True


def test_none_output(monty_run: RunMonty):
    assert monty_run('None') is None


def test_bool_true(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': True})
    assert result is True
    assert type(result) is bool


def test_bool_false(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': False})
    assert result is False
    assert type(result) is bool


def test_int(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': 42}) == snapshot(42)
    assert monty_run('x', inputs={'x': -100}) == snapshot(-100)
    assert monty_run('x', inputs={'x': 0}) == snapshot(0)


def test_float(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': 3.14}) == snapshot(3.14)
    assert monty_run('x', inputs={'x': -2.5}) == snapshot(-2.5)
    assert monty_run('x', inputs={'x': 0.0}) == snapshot(0.0)


def test_string(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': 'hello'}) == snapshot('hello')
    assert monty_run('x', inputs={'x': ''}) == snapshot('')
    assert monty_run('x', inputs={'x': 'unicode: éè'}) == snapshot('unicode: éè')


def test_bytes(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': b'hello'}) == snapshot(b'hello')
    assert monty_run('x', inputs={'x': b''}) == snapshot(b'')
    assert monty_run('x', inputs={'x': b'\x00\x01\x02'}) == snapshot(b'\x00\x01\x02')


def test_list(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': [1, 2, 3]}) == snapshot([1, 2, 3])
    assert monty_run('x', inputs={'x': []}) == snapshot([])
    assert monty_run('x', inputs={'x': ['a', 'b']}) == snapshot(['a', 'b'])


def test_tuple(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': (1, 2, 3)}) == snapshot((1, 2, 3))
    assert monty_run('x', inputs={'x': ()}) == snapshot(())
    assert monty_run('x', inputs={'x': ('a',)}) == snapshot(('a',))


def test_dict(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': {'a': 1, 'b': 2}}) == snapshot({'a': 1, 'b': 2})
    assert monty_run('x', inputs={'x': {}}) == snapshot({})


def test_set(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': {1, 2, 3}}) == snapshot({1, 2, 3})
    assert monty_run('x', inputs={'x': set()}) == snapshot(set())


def test_frozenset(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': frozenset([1, 2, 3])}) == snapshot(frozenset({1, 2, 3}))
    assert monty_run('x', inputs={'x': frozenset()}) == snapshot(frozenset())


def test_ellipsis_input(monty_run: RunMonty):
    assert monty_run('x is ...', inputs={'x': ...}) is True


def test_ellipsis_output(monty_run: RunMonty):
    assert monty_run('...') is ...


def test_nested_list(monty_run: RunMonty):
    nested = [[1, 2], [3, [4, 5]]]
    assert monty_run('x', inputs={'x': nested}) == snapshot([[1, 2], [3, [4, 5]]])


def test_nested_dict(monty_run: RunMonty):
    nested = {'a': {'b': {'c': 1}}}
    assert monty_run('x', inputs={'x': nested}) == snapshot({'a': {'b': {'c': 1}}})


def test_mixed_nested(monty_run: RunMonty):
    mixed = {'list': [1, 2], 'tuple': (3, 4), 'nested': {'set': {5, 6}}}
    result = monty_run('x', inputs={'x': mixed})
    assert result['list'] == snapshot([1, 2])
    assert result['tuple'] == snapshot((3, 4))
    assert result['nested']['set'] == snapshot({5, 6})


def test_list_output(monty_run: RunMonty):
    assert monty_run('[1, 2, 3]') == snapshot([1, 2, 3])


def test_dict_output(monty_run: RunMonty):
    assert monty_run("{'a': 1, 'b': 2}") == snapshot({'a': 1, 'b': 2})


def test_tuple_output(monty_run: RunMonty):
    assert monty_run('(1, 2, 3)') == snapshot((1, 2, 3))


def test_set_output(monty_run: RunMonty):
    assert monty_run('{1, 2, 3}') == snapshot({1, 2, 3})


def test_type_object_output(monty_run: RunMonty):
    """A type object returned from the sandbox reconstructs as the matching host
    class; modeled stdlib types resolve from their real module (`Path` → `PurePosixPath`)."""
    code = """
import datetime, re
from pathlib import Path
[
    int, str, type, type(None), type(...), type(iter([])),
    type(Path('/x')), Path,
    datetime.datetime, datetime.date, datetime.timedelta, datetime.timezone,
    type(re.compile('a')), type(re.match('a', 'a')),
]
"""
    # Type objects have no `__eq__` override, so `==` compares them by identity.
    assert monty_run(code) == [
        int,
        str,
        type,
        type(None),
        type(...),
        type(iter([])),
        pathlib.PurePosixPath,
        pathlib.PurePosixPath,
        datetime.datetime,
        datetime.date,
        datetime.timedelta,
        datetime.timezone,
        re.Pattern,
        re.Match,
    ]


def test_type_object_input_roundtrip(monty_run: RunMonty):
    """A type object passed in as an input is preserved as a type (not degraded to
    a callable) and round-trips back out by identity."""
    types: list[type[object]] = [
        int,
        str,
        type,
        bool,
        type(None),
        type(...),
        type(iter([])),
        datetime.datetime,
        datetime.date,
        datetime.timedelta,
        datetime.timezone,
        pathlib.PurePosixPath,
        pathlib.PurePath,
        pathlib.PosixPath,
        re.Pattern,
        re.Match,
    ]
    for ty in types:
        # The pathlib family all collapses to a single Monty path type, which
        # re-emerges as PurePosixPath; everything else round-trips by identity.
        expected: type[object] = pathlib.PurePosixPath if issubclass(ty, pathlib.PurePath) else ty
        assert monty_run('x', inputs={'x': ty}) is expected


def test_type_object_input_isinstance(monty_run: RunMonty):
    """A type object passed in is usable as the second argument to `isinstance`
    inside the sandbox."""
    assert monty_run('(isinstance(5, t), isinstance("a", t))', inputs={'t': int}) == snapshot((True, False))


def test_unmodeled_class_input_becomes_callable(monty_run: RunMonty):
    """A host class Monty has no `Type` for still degrades to a callable
    function at the boundary (unchanged behavior)."""

    class Foo:
        pass

    assert monty_run('(type(x).__name__, repr(x))', inputs={'x': Foo}) == snapshot(
        ('function', "<function 'Foo' external>")
    )


def test_spoofed_builtin_type_not_recognized(monty_run: RunMonty):
    """Type detection is by identity, not name/module strings: a class that
    forges `__name__`/`__module__` to impersonate `int` is not treated as the
    builtin and degrades to a callable."""

    class FakeInt:
        pass

    FakeInt.__name__ = 'int'
    FakeInt.__qualname__ = 'int'
    FakeInt.__module__ = 'builtins'

    assert monty_run('type(x).__name__', inputs={'x': FakeInt}) == snapshot('function')


def test_date_input_roundtrip(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': datetime.date(2024, 1, 15)})
    assert (type(result).__name__, repr(result)) == snapshot(('date', 'datetime.date(2024, 1, 15)'))


def test_datetime_input_roundtrip(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)})
    assert (type(result).__name__, repr(result)) == snapshot(
        ('datetime', 'datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)')
    )


def test_aware_datetime_input_roundtrip(monty_run: RunMonty):
    result = monty_run(
        'x', inputs={'x': datetime.datetime(2024, 1, 15, 10, 30, 5, 123456, tzinfo=datetime.timezone.utc)}
    )
    assert (type(result).__name__, repr(result)) == snapshot(
        ('datetime', 'datetime.datetime(2024, 1, 15, 10, 30, 5, 123456, tzinfo=datetime.timezone.utc)')
    )


def test_timedelta_input_roundtrip(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': datetime.timedelta(days=-1, seconds=3661, microseconds=42)})
    assert (type(result).__name__, repr(result)) == snapshot(
        ('timedelta', 'datetime.timedelta(days=-1, seconds=3661, microseconds=42)')
    )


def test_timezone_input_roundtrip(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': datetime.timezone(datetime.timedelta(hours=2))})
    assert (type(result).__name__, repr(result)) == snapshot(
        ('timezone', 'datetime.timezone(datetime.timedelta(seconds=7200))')
    )


def test_named_timezone_input_roundtrip(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': datetime.timezone(datetime.timedelta(hours=2), 'PLUS2')})
    assert (type(result).__name__, repr(result)) == snapshot(
        ('timezone', "datetime.timezone(datetime.timedelta(seconds=7200), 'PLUS2')")
    )


# === Exception types ===


def test_exception_input(monty_run: RunMonty):
    exc = ValueError('test error')
    result = monty_run('x', inputs={'x': exc})
    assert isinstance(result, ValueError)
    assert str(result) == snapshot('test error')


def test_exception_output(monty_run: RunMonty):
    result = monty_run('ValueError("created")')
    assert isinstance(result, ValueError)
    assert str(result) == snapshot('created')


@pytest.mark.parametrize('exc_class', [ValueError, TypeError, RuntimeError, AttributeError], ids=repr)
def test_exception_roundtrip(monty_run: RunMonty, exc_class: type[Exception]):
    exc = exc_class('message')
    result = monty_run('x', inputs={'x': exc})
    assert type(result) is exc_class
    assert str(result) == snapshot('message')


def test_exception_subclass_input(monty_run: RunMonty):
    """Custom exception subtypes are converted to their nearest supported base."""

    class MyError(ValueError):
        pass

    exc = MyError('custom')
    result = monty_run('x', inputs={'x': exc})
    # Custom exception becomes ValueError (nearest supported type)
    assert type(result) is ValueError
    assert str(result) == snapshot('custom')


# === Subtype coercion ===
# Monty converts Python subclasses to their base types since it doesn't
# have Python's class system.


def test_int_subclass_input(monty_run: RunMonty):
    class MyInt(int):
        pass

    result = monty_run('x', inputs={'x': MyInt(42)})
    assert type(result) is int
    assert result == snapshot(42)


def test_str_subclass_input(monty_run: RunMonty):
    class MyStr(str):
        pass

    result = monty_run('x', inputs={'x': MyStr('hello')})
    assert type(result) is str
    assert result == snapshot('hello')


def test_list_subclass_input(monty_run: RunMonty):
    class MyList(list[int]):
        pass

    result = monty_run('x', inputs={'x': MyList([1, 2, 3])})
    assert type(result) is list
    assert result == snapshot([1, 2, 3])


def test_dict_subclass_input(monty_run: RunMonty):
    class MyDict(dict[str, int]):
        pass

    result = monty_run('x', inputs={'x': MyDict({'a': 1})})
    assert type(result) is dict
    assert result == snapshot({'a': 1})


def test_tuple_subclass_input(monty_run: RunMonty):
    class MyTuple(tuple[int, ...]):
        pass

    result = monty_run('x', inputs={'x': MyTuple((1, 2))})
    assert type(result) is tuple
    assert result == snapshot((1, 2))


def test_set_subclass_input(monty_run: RunMonty):
    class MySet(set[int]):
        pass

    result = monty_run('x', inputs={'x': MySet({1, 2})})
    assert type(result) is set
    assert result == snapshot({1, 2})


def test_bool_preserves_type(monty_run: RunMonty):
    """Bool is a subclass of int but should be preserved as bool."""
    result = monty_run('x', inputs={'x': True})
    assert type(result) is bool
    assert result is True


def test_return_int(monty_run: RunMonty):
    assert monty_run('x = 4\ntype(x)') is int
    assert monty_run('int') is int


def test_return_exception(monty_run: RunMonty):
    assert monty_run('x = ValueError()\ntype(x)') is ValueError
    assert monty_run('ValueError') is ValueError


def test_return_builtin(monty_run: RunMonty):
    assert monty_run('len') is len


# === BigInt (arbitrary precision integers) ===


def test_bigint_input(monty_run: RunMonty):
    """Passing a large integer (> i64::MAX) as input."""
    big = 2**100
    result = monty_run('x', inputs={'x': big})
    assert result == big
    assert type(result) is int


def test_bigint_output(monty_run: RunMonty):
    """Returning a large integer computed inside Monty."""
    result = monty_run('2**100')
    assert result == 2**100
    assert type(result) is int


def test_bigint_negative_input(monty_run: RunMonty):
    """Passing a large negative integer as input."""
    big_neg = -(2**100)
    result = monty_run('x', inputs={'x': big_neg})
    assert result == big_neg
    assert type(result) is int


def test_int_overflow_to_bigint(monty_run: RunMonty):
    """Small int input that overflows to bigint during computation."""
    max_i64 = 9223372036854775807
    result = monty_run('x + 1', inputs={'x': max_i64})
    assert result == max_i64 + 1
    assert type(result) is int


def test_bigint_arithmetic(monty_run: RunMonty):
    """BigInt arithmetic operations."""
    big = 2**100
    result = monty_run('x * 2 + y', inputs={'x': big, 'y': big})
    assert result == big * 2 + big
    assert type(result) is int


def test_bigint_comparison(monty_run: RunMonty):
    """Comparing bigints with regular ints."""
    big = 2**100
    assert monty_run('x > y', inputs={'x': big, 'y': 42}) is True
    assert monty_run('x > y', inputs={'x': 42, 'y': big}) is False


def test_bigint_in_collection(monty_run: RunMonty):
    """BigInts inside collections."""
    big = 2**100
    result = monty_run('x', inputs={'x': [big, 42, big * 2]})
    assert result == [big, 42, big * 2]
    assert type(result[0]) is int


def test_bigint_as_dict_key(monty_run: RunMonty):
    """BigInt as dictionary key."""
    big = 2**100
    result = monty_run('x', inputs={'x': {big: 'value'}})
    assert result == {big: 'value'}
    assert big in result


def test_bigint_hash_consistency_small_values(monty_run: RunMonty):
    """Hash of small values computed as BigInt must match regular int hash.

    This is critical for dict key lookups: inserting with int and looking up
    with a computed BigInt (or vice versa) must work correctly.
    """
    # Value 42 computed via BigInt arithmetic
    big = 2**100
    computed_42 = monty_run('(x - x) + 42', inputs={'x': big})

    # Hash must match
    assert hash(computed_42) == hash(42), 'hash of computed int must match literal'

    # Dict lookup must work both ways
    d = {42: 'value'}
    assert d[computed_42] == 'value', 'lookup with computed bigint finds int key'

    d2 = {computed_42: 'value'}
    assert d2[42] == 'value', 'lookup with int finds computed bigint key'


def test_bigint_hash_consistency_boundary(monty_run: RunMonty):
    """Hash consistency at i64 boundary values."""
    max_i64 = 9223372036854775807

    # Compute MAX_I64 via BigInt arithmetic
    computed_max = monty_run('(x - 1)', inputs={'x': max_i64 + 1})

    assert hash(computed_max) == hash(max_i64), 'hash at MAX_I64 boundary must match'


def test_bigint_hash_consistency_large_values(monty_run: RunMonty):
    """Equal large BigInts must hash the same."""
    big1 = 2**100
    big2 = 2**100

    # Verify they hash the same in Python first
    assert hash(big1) == hash(big2), 'precondition: equal bigints hash same in Python'

    # Verify hashes match after round-trip through Monty
    result1 = monty_run('x', inputs={'x': big1})
    result2 = monty_run('x', inputs={'x': big2})

    assert hash(result1) == hash(result2), 'equal bigints from Monty must hash same'

    # Dict lookup must work
    d = {result1: 'value'}
    assert d[result2] == 'value', 'lookup with equal bigint works'


# === NamedTuple output ===


def test_namedtuple_sys_version_info(monty_run: RunMonty):
    """sys.version_info returns a proper namedtuple with attribute access."""
    result = monty_run('import sys; sys.version_info')

    # Should have named attribute access
    assert hasattr(result, 'major')
    assert hasattr(result, 'minor')
    assert hasattr(result, 'micro')
    assert hasattr(result, 'releaselevel')
    assert hasattr(result, 'serial')

    # Values should match Monty's Python version (3.14)
    assert result.major == snapshot(3)
    assert result.minor == snapshot(14)
    assert result.micro == snapshot(0)
    assert result.releaselevel == snapshot('final')
    assert result.serial == snapshot(0)


def test_namedtuple_sys_version_info_index_access(monty_run: RunMonty):
    """sys.version_info supports both index and attribute access."""
    result = monty_run('import sys; sys.version_info')

    # Index access should work
    assert result[0] == result.major
    assert result[1] == result.minor
    assert result[2] == result.micro


def test_namedtuple_sys_version_info_tuple_comparison(monty_run: RunMonty):
    """sys.version_info can be compared to tuples."""
    result = monty_run('import sys; (sys.version_info.major, sys.version_info.minor, sys.version_info.micro)')
    assert result == snapshot((3, 14, 0))


# === User-defined NamedTuple input ===


def test_namedtuple_custom_input_attribute_access(monty_run: RunMonty):
    """User-defined NamedTuple with custom field names can be accessed by attribute."""

    class Person(NamedTuple):
        name: str
        age: int

    assert monty_run('p.name', inputs={'p': Person(name='Alice', age=30)}) == snapshot('Alice')
    assert monty_run('p.age', inputs={'p': Person(name='Alice', age=30)}) == snapshot(30)


def test_namedtuple_custom_input_index_access(monty_run: RunMonty):
    """User-defined NamedTuple supports both attribute and index access."""

    class Point(NamedTuple):
        x: int
        y: int

    assert monty_run('p[0] + p[1]', inputs={'p': Point(x=10, y=20)}) == snapshot(30)


def test_namedtuple_custom_input_multiple_fields(monty_run: RunMonty):
    """NamedTuple with multiple custom field names works correctly."""

    class Config(NamedTuple):
        host: str
        port: int
        debug: bool
        timeout: float

    config = Config(host='localhost', port=8080, debug=True, timeout=30.0)
    assert monty_run("f'{c.host}:{c.port}'", inputs={'c': config}) == snapshot('localhost:8080')
    assert monty_run('c.debug', inputs={'c': config}) is True


def test_namedtuple_custom_input_repr(monty_run: RunMonty):
    """User-defined NamedTuple has correct repr with fully-qualified type name."""

    class Item(NamedTuple):
        name: str
        price: float

    result = monty_run('repr(item)', inputs={'item': Item(name='widget', price=9.99)})
    # Monty uses the full qualified name (module.ClassName) for the type
    assert result == snapshot("test_types.Item(name='widget', price=9.99)")


def test_namedtuple_custom_input_len(monty_run: RunMonty):
    """User-defined NamedTuple supports len()."""

    class Triple(NamedTuple):
        a: int
        b: int
        c: int

    assert monty_run('len(t)', inputs={'t': Triple(a=1, b=2, c=3)}) == snapshot(3)


def test_namedtuple_custom_input_roundtrip(monty_run: RunMonty):
    """User-defined NamedTuple can be passed through and returned."""

    class Pair(NamedTuple):
        first: int
        second: int

    result = monty_run('p', inputs={'p': Pair(first=1, second=2)})
    # Returns a namedtuple-like object (not the same Python class)
    assert result[0] == snapshot(1)
    assert result[1] == snapshot(2)
    assert result.first == snapshot(1)
    assert result.second == snapshot(2)


def test_namedtuple_custom_missing_attr_error(monty_run: RunMonty):
    """Accessing non-existent attribute on custom NamedTuple raises AttributeError."""

    class Simple(NamedTuple):
        value: int

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('s.nonexistent', inputs={'s': Simple(value=42)})
    # Monty uses the full qualified name (module.ClassName) for the type
    assert "AttributeError: 'test_types.Simple' object has no attribute 'nonexistent'" in str(exc_info.value)


# === Unsupported type conversion ===


def test_unsupported_type_raises_type_error(monty_run: RunMonty):
    """Passing an unsupported type raises a MontyConversionError."""
    with pytest.raises(MontyConversionError, match='Cannot convert') as exc_info:
        monty_run('x', inputs={'x': re.compile('foo')})
    assert isinstance(exc_info.value.exception(), TypeError)


# === Callable/function input ===


def test_callable_input(monty_run: RunMonty):
    """Functions passed as input are converted to MontyObject::Function with name."""
    # Function objects are output as their name string
    assert monty_run('x', inputs={'x': len}) == snapshot('len')


def test_lambda_input(monty_run: RunMonty):
    """Lambda functions are converted with name '<lambda>'."""
    assert monty_run('x', inputs={'x': lambda: None}) == snapshot('<lambda>')


# === Timezone edge cases ===


def test_utc_timezone_input_roundtrip(monty_run: RunMonty):
    """datetime.timezone.utc singleton roundtrips correctly."""
    result = monty_run('x', inputs={'x': datetime.timezone.utc})
    assert result == datetime.timezone.utc
    assert repr(result) == snapshot('datetime.timezone.utc')


def test_negative_timezone_offset(monty_run: RunMonty):
    """Negative timezone offsets roundtrip correctly."""
    tz = datetime.timezone(datetime.timedelta(hours=-5))
    result = monty_run('x', inputs={'x': tz})
    assert repr(result) == snapshot('datetime.timezone(datetime.timedelta(days=-1, seconds=68400))')


def test_aware_datetime_fixed_offset_roundtrip(monty_run: RunMonty):
    """Datetime with non-UTC fixed offset roundtrips correctly."""
    tz = datetime.timezone(datetime.timedelta(hours=5, minutes=30))
    dt = datetime.datetime(2024, 6, 15, 14, 30, 0, tzinfo=tz)
    result = monty_run('x', inputs={'x': dt})
    assert (type(result).__name__, repr(result)) == snapshot(
        (
            'datetime',
            'datetime.datetime(2024, 6, 15, 14, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=19800)))',
        )
    )


def test_aware_datetime_named_timezone_roundtrip(monty_run: RunMonty):
    """Datetime with a named timezone roundtrips correctly."""
    tz = datetime.timezone(datetime.timedelta(hours=-5), 'EST')
    dt = datetime.datetime(2024, 12, 25, 8, 0, 0, tzinfo=tz)
    result = monty_run('x', inputs={'x': dt})
    assert (type(result).__name__, repr(result)) == snapshot(
        (
            'datetime',
            "datetime.datetime(2024, 12, 25, 8, 0, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=68400), 'EST'))",
        )
    )


# === zoneinfo timezone conversion ===


def test_zoneinfo_datetime_summer(monty_run: RunMonty):
    """Datetime with zoneinfo.ZoneInfo tzinfo converts with DST offset."""
    tz = zoneinfo.ZoneInfo('America/New_York')
    dt = datetime.datetime(2024, 6, 15, 12, 0, 0, tzinfo=tz)
    result = monty_run('x', inputs={'x': dt})
    # Summer: EDT = UTC-4
    assert result.utcoffset() == snapshot(datetime.timedelta(days=-1, seconds=72000))
    assert result.tzname() == snapshot('EDT')


def test_zoneinfo_datetime_winter(monty_run: RunMonty):
    """Datetime with zoneinfo.ZoneInfo tzinfo converts with standard offset."""
    tz = zoneinfo.ZoneInfo('America/New_York')
    dt = datetime.datetime(2024, 1, 15, 12, 0, 0, tzinfo=tz)
    result = monty_run('x', inputs={'x': dt})
    # Winter: EST = UTC-5
    assert result.utcoffset() == snapshot(datetime.timedelta(days=-1, seconds=68400))
    assert result.tzname() == snapshot('EST')


def test_zoneinfo_datetime_utc(monty_run: RunMonty):
    """Datetime with zoneinfo UTC converts correctly."""
    tz = zoneinfo.ZoneInfo('UTC')
    dt = datetime.datetime(2024, 6, 15, 12, 0, 0, tzinfo=tz)
    result = monty_run('x', inputs={'x': dt})
    assert result.utcoffset() == snapshot(datetime.timedelta(0))
    assert result.tzname() == snapshot('UTC')


def test_zoneinfo_datetime_positive_offset(monty_run: RunMonty):
    """Datetime with a positive-offset zoneinfo timezone."""
    tz = zoneinfo.ZoneInfo('Asia/Kolkata')
    dt = datetime.datetime(2024, 6, 15, 12, 0, 0, tzinfo=tz)
    result = monty_run('x', inputs={'x': dt})
    # IST = UTC+5:30
    assert result.utcoffset() == snapshot(datetime.timedelta(seconds=19800))
    assert result.tzname() == snapshot('IST')


def test_zoneinfo_datetime_preserves_fields(monty_run: RunMonty):
    """All datetime fields are preserved when converting with zoneinfo tzinfo."""
    tz = zoneinfo.ZoneInfo('Europe/London')
    dt = datetime.datetime(2024, 7, 20, 15, 45, 30, 123456, tzinfo=tz)
    result = monty_run('x', inputs={'x': dt})
    assert (result.year, result.month, result.day) == snapshot((2024, 7, 20))
    assert (result.hour, result.minute, result.second) == snapshot((15, 45, 30))
    assert result.microsecond == snapshot(123456)


def test_zoneinfo_standalone_raises_type_error(monty_run: RunMonty):
    """Standalone ZoneInfo objects (without a datetime) are not convertible."""
    tz = zoneinfo.ZoneInfo('America/New_York')
    with pytest.raises(MontyConversionError, match='Cannot convert') as exc_info:
        monty_run('x', inputs={'x': tz})
    assert isinstance(exc_info.value.exception(), TypeError)


# === Timedelta edge cases ===


def test_timedelta_zero(monty_run: RunMonty):
    """Zero timedelta roundtrips correctly."""
    result = monty_run('x', inputs={'x': datetime.timedelta(0)})
    assert (type(result).__name__, repr(result)) == snapshot(('timedelta', 'datetime.timedelta(0)'))


def test_timedelta_days_only(monty_run: RunMonty):
    """Timedelta with only days component."""
    result = monty_run('x', inputs={'x': datetime.timedelta(days=30)})
    assert (type(result).__name__, repr(result)) == snapshot(('timedelta', 'datetime.timedelta(days=30)'))


# === Path conversion ===


def test_path_input_roundtrip(monty_run: RunMonty):
    """pathlib.PurePosixPath input roundtrips correctly."""
    result = monty_run('x', inputs={'x': pathlib.PurePosixPath('/usr/local/bin')})
    assert type(result).__name__ == snapshot('PurePosixPath')
    assert str(result) == snapshot('/usr/local/bin')


def test_posix_path_input(monty_run: RunMonty):
    """pathlib.PosixPath (subclass of PurePosixPath) is accepted."""
    result = monty_run('x', inputs={'x': pathlib.PosixPath('/tmp')})
    # PosixPath is converted via PurePosixPath
    assert type(result).__name__ == snapshot('PurePosixPath')
    assert str(result) == snapshot('/tmp')


# === Additional subclass coercion ===


def test_float_subclass_input(monty_run: RunMonty):
    class MyFloat(float):
        pass

    result = monty_run('x', inputs={'x': MyFloat(3.14)})
    assert type(result) is float
    assert result == snapshot(3.14)


def test_bytes_subclass_input(monty_run: RunMonty):
    class MyBytes(bytes):
        pass

    result = monty_run('x', inputs={'x': MyBytes(b'hello')})
    assert type(result) is bytes
    assert result == snapshot(b'hello')


def test_frozenset_subclass_input(monty_run: RunMonty):
    class MyFrozenSet(frozenset[int]):
        pass

    result = monty_run('x', inputs={'x': MyFrozenSet([1, 2, 3])})
    assert type(result) is frozenset
    assert result == snapshot(frozenset({1, 2, 3}))
