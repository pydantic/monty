import pytest
from inline_snapshot import snapshot

import pydantic_monty


def test_none_input():
    m = pydantic_monty.Monty('x is None', inputs=['x'])
    assert m.run(inputs={'x': None}) is True


def test_none_output():
    m = pydantic_monty.Monty('None')
    assert m.run() is None


def test_bool_true():
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': True})
    assert result is True
    assert type(result) is bool


def test_bool_false():
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': False})
    assert result is False
    assert type(result) is bool


def test_int():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': 42}) == snapshot(42)
    assert m.run(inputs={'x': -100}) == snapshot(-100)
    assert m.run(inputs={'x': 0}) == snapshot(0)


def test_float():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': 3.14}) == snapshot(3.14)
    assert m.run(inputs={'x': -2.5}) == snapshot(-2.5)
    assert m.run(inputs={'x': 0.0}) == snapshot(0.0)


def test_string():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': 'hello'}) == snapshot('hello')
    assert m.run(inputs={'x': ''}) == snapshot('')
    assert m.run(inputs={'x': 'unicode: éè'}) == snapshot('unicode: éè')


def test_bytes():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': b'hello'}) == snapshot(b'hello')
    assert m.run(inputs={'x': b''}) == snapshot(b'')
    assert m.run(inputs={'x': b'\x00\x01\x02'}) == snapshot(b'\x00\x01\x02')


def test_list():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': [1, 2, 3]}) == snapshot([1, 2, 3])
    assert m.run(inputs={'x': []}) == snapshot([])
    assert m.run(inputs={'x': ['a', 'b']}) == snapshot(['a', 'b'])


def test_tuple():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': (1, 2, 3)}) == snapshot((1, 2, 3))
    assert m.run(inputs={'x': ()}) == snapshot(())
    assert m.run(inputs={'x': ('a',)}) == snapshot(('a',))


def test_dict():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': {'a': 1, 'b': 2}}) == snapshot({'a': 1, 'b': 2})
    assert m.run(inputs={'x': {}}) == snapshot({})


def test_set():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': {1, 2, 3}}) == snapshot({1, 2, 3})
    assert m.run(inputs={'x': set()}) == snapshot(set())


def test_frozenset():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': frozenset([1, 2, 3])}) == snapshot(frozenset({1, 2, 3}))
    assert m.run(inputs={'x': frozenset()}) == snapshot(frozenset())


def test_ellipsis_input():
    m = pydantic_monty.Monty('x is ...', inputs=['x'])
    assert m.run(inputs={'x': ...}) is True


def test_ellipsis_output():
    m = pydantic_monty.Monty('...')
    assert m.run() is ...


def test_nested_list():
    m = pydantic_monty.Monty('x', inputs=['x'])
    nested = [[1, 2], [3, [4, 5]]]
    assert m.run(inputs={'x': nested}) == snapshot([[1, 2], [3, [4, 5]]])


def test_nested_dict():
    m = pydantic_monty.Monty('x', inputs=['x'])
    nested = {'a': {'b': {'c': 1}}}
    assert m.run(inputs={'x': nested}) == snapshot({'a': {'b': {'c': 1}}})


def test_mixed_nested():
    m = pydantic_monty.Monty('x', inputs=['x'])
    mixed = {'list': [1, 2], 'tuple': (3, 4), 'nested': {'set': {5, 6}}}
    result = m.run(inputs={'x': mixed})
    assert result['list'] == snapshot([1, 2])
    assert result['tuple'] == snapshot((3, 4))
    assert result['nested']['set'] == snapshot({5, 6})


def test_list_output():
    m = pydantic_monty.Monty('[1, 2, 3]')
    assert m.run() == snapshot([1, 2, 3])


def test_dict_output():
    m = pydantic_monty.Monty("{'a': 1, 'b': 2}")
    assert m.run() == snapshot({'a': 1, 'b': 2})


def test_tuple_output():
    m = pydantic_monty.Monty('(1, 2, 3)')
    assert m.run() == snapshot((1, 2, 3))


def test_set_output():
    m = pydantic_monty.Monty('{1, 2, 3}')
    assert m.run() == snapshot({1, 2, 3})


# === Exception types ===


def test_exception_input():
    m = pydantic_monty.Monty('x', inputs=['x'])
    exc = ValueError('test error')
    result = m.run(inputs={'x': exc})
    assert isinstance(result, ValueError)
    assert str(result) == snapshot('test error')


def test_exception_output():
    m = pydantic_monty.Monty('ValueError("created")')
    result = m.run()
    assert isinstance(result, ValueError)
    assert str(result) == snapshot('created')


@pytest.mark.parametrize('exc_class', [ValueError, TypeError, RuntimeError, AttributeError], ids=repr)
def test_exception_roundtrip(exc_class: type[Exception]):
    m = pydantic_monty.Monty('x', inputs=['x'])
    exc = exc_class('message')
    result = m.run(inputs={'x': exc})
    assert type(result) is exc_class
    assert str(result) == snapshot('message')


def test_exception_subclass_input():
    """Custom exception subtypes are converted to their nearest supported base."""

    class MyError(ValueError):
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    exc = MyError('custom')
    result = m.run(inputs={'x': exc})
    # Custom exception becomes ValueError (nearest supported type)
    assert type(result) is ValueError
    assert str(result) == snapshot('custom')


# === Subtype coercion ===
# Monty converts Python subclasses to their base types since it doesn't
# have Python's class system.


def test_int_subclass_input():
    class MyInt(int):
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyInt(42)})
    assert type(result) is int
    assert result == snapshot(42)


def test_str_subclass_input():
    class MyStr(str):
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyStr('hello')})
    assert type(result) is str
    assert result == snapshot('hello')


def test_list_subclass_input():
    class MyList(list[int]):
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyList([1, 2, 3])})
    assert type(result) is list
    assert result == snapshot([1, 2, 3])


def test_dict_subclass_input():
    class MyDict(dict[str, int]):
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyDict({'a': 1})})
    assert type(result) is dict
    assert result == snapshot({'a': 1})


def test_tuple_subclass_input():
    class MyTuple(tuple[int, ...]):
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyTuple((1, 2))})
    assert type(result) is tuple
    assert result == snapshot((1, 2))


def test_set_subclass_input():
    class MySet(set[int]):
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MySet({1, 2})})
    assert type(result) is set
    assert result == snapshot({1, 2})


def test_bool_preserves_type():
    """Bool is a subclass of int but should be preserved as bool."""
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': True})
    assert type(result) is bool
    assert result is True


def test_return_int():
    m = pydantic_monty.Monty('x = 4\ntype(x)')
    result = m.run()
    assert result is int

    m = pydantic_monty.Monty('int')
    result = m.run()
    assert result is int


def test_return_exception():
    m = pydantic_monty.Monty('x = ValueError()\ntype(x)')
    result = m.run()
    assert result is ValueError

    m = pydantic_monty.Monty('ValueError')
    result = m.run()
    assert result is ValueError


def test_return_builtin():
    m = pydantic_monty.Monty('len')
    result = m.run()
    assert result is len


# === BigInt (arbitrary precision integers) ===


def test_bigint_input():
    """Passing a large integer (> i64::MAX) as input."""
    big = 2**100
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': big})
    assert result == big
    assert type(result) is int


def test_bigint_output():
    """Returning a large integer computed inside Monty."""
    m = pydantic_monty.Monty('2**100')
    result = m.run()
    assert result == 2**100
    assert type(result) is int


def test_bigint_negative_input():
    """Passing a large negative integer as input."""
    big_neg = -(2**100)
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': big_neg})
    assert result == big_neg
    assert type(result) is int


def test_int_overflow_to_bigint():
    """Small int input that overflows to bigint during computation."""
    max_i64 = 9223372036854775807
    m = pydantic_monty.Monty('x + 1', inputs=['x'])
    result = m.run(inputs={'x': max_i64})
    assert result == max_i64 + 1
    assert type(result) is int


def test_bigint_arithmetic():
    """BigInt arithmetic operations."""
    big = 2**100
    m = pydantic_monty.Monty('x * 2 + y', inputs=['x', 'y'])
    result = m.run(inputs={'x': big, 'y': big})
    assert result == big * 2 + big
    assert type(result) is int


def test_bigint_comparison():
    """Comparing bigints with regular ints."""
    big = 2**100
    m = pydantic_monty.Monty('x > y', inputs=['x', 'y'])
    assert m.run(inputs={'x': big, 'y': 42}) is True
    assert m.run(inputs={'x': 42, 'y': big}) is False


def test_bigint_in_collection():
    """BigInts inside collections."""
    big = 2**100
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': [big, 42, big * 2]})
    assert result == [big, 42, big * 2]
    assert type(result[0]) is int


def test_bigint_as_dict_key():
    """BigInt as dictionary key."""
    big = 2**100
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': {big: 'value'}})
    assert result == {big: 'value'}
    assert big in result


def test_bigint_hash_consistency_small_values():
    """Hash of small values computed as BigInt must match regular int hash.

    This is critical for dict key lookups: inserting with int and looking up
    with a computed BigInt (or vice versa) must work correctly.
    """
    # Value 42 computed via BigInt arithmetic
    big = 2**100
    m = pydantic_monty.Monty('(x - x) + 42', inputs=['x'])
    computed_42 = m.run(inputs={'x': big})

    # Hash must match
    assert hash(computed_42) == hash(42), 'hash of computed int must match literal'

    # Dict lookup must work both ways
    d = {42: 'value'}
    assert d[computed_42] == 'value', 'lookup with computed bigint finds int key'

    d2 = {computed_42: 'value'}
    assert d2[42] == 'value', 'lookup with int finds computed bigint key'


def test_bigint_hash_consistency_boundary():
    """Hash consistency at i64 boundary values."""
    max_i64 = 9223372036854775807

    # Compute MAX_I64 via BigInt arithmetic
    m = pydantic_monty.Monty('(x - 1)', inputs=['x'])
    computed_max = m.run(inputs={'x': max_i64 + 1})

    assert hash(computed_max) == hash(max_i64), 'hash at MAX_I64 boundary must match'


def test_bigint_hash_consistency_large_values():
    """Equal large BigInts must hash the same."""
    big1 = 2**100
    big2 = 2**100

    # Verify they hash the same in Python first
    assert hash(big1) == hash(big2), 'precondition: equal bigints hash same in Python'

    # Verify hashes match after round-trip through Monty
    m = pydantic_monty.Monty('x', inputs=['x'])
    result1 = m.run(inputs={'x': big1})
    result2 = m.run(inputs={'x': big2})

    assert hash(result1) == hash(result2), 'equal bigints from Monty must hash same'

    # Dict lookup must work
    d = {result1: 'value'}
    assert d[result2] == 'value', 'lookup with equal bigint works'


# === NamedTuple output ===


def test_namedtuple_sys_version_info():
    """sys.version_info returns a proper namedtuple with attribute access."""
    m = pydantic_monty.Monty('import sys; sys.version_info')
    result = m.run()

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


def test_namedtuple_sys_version_info_index_access():
    """sys.version_info supports both index and attribute access."""
    m = pydantic_monty.Monty('import sys; sys.version_info')
    result = m.run()

    # Index access should work
    assert result[0] == result.major
    assert result[1] == result.minor
    assert result[2] == result.micro


def test_namedtuple_sys_version_info_tuple_comparison():
    """sys.version_info can be compared to tuples."""
    m = pydantic_monty.Monty('import sys; (sys.version_info.major, sys.version_info.minor, sys.version_info.micro)')
    result = m.run()
    assert result == snapshot((3, 14, 0))


# === User-defined NamedTuple input ===


def test_namedtuple_custom_input_attribute_access():
    """User-defined NamedTuple with custom field names can be accessed by attribute."""
    from typing import NamedTuple

    class Person(NamedTuple):
        name: str
        age: int

    m = pydantic_monty.Monty('p.name', inputs=['p'])
    assert m.run(inputs={'p': Person(name='Alice', age=30)}) == snapshot('Alice')

    m = pydantic_monty.Monty('p.age', inputs=['p'])
    assert m.run(inputs={'p': Person(name='Alice', age=30)}) == snapshot(30)


def test_namedtuple_custom_input_index_access():
    """User-defined NamedTuple supports both attribute and index access."""
    from typing import NamedTuple

    class Point(NamedTuple):
        x: int
        y: int

    m = pydantic_monty.Monty('p[0] + p[1]', inputs=['p'])
    assert m.run(inputs={'p': Point(x=10, y=20)}) == snapshot(30)


def test_namedtuple_custom_input_multiple_fields():
    """NamedTuple with multiple custom field names works correctly."""
    from typing import NamedTuple

    class Config(NamedTuple):
        host: str
        port: int
        debug: bool
        timeout: float

    m = pydantic_monty.Monty("f'{c.host}:{c.port}'", inputs=['c'])
    result = m.run(inputs={'c': Config(host='localhost', port=8080, debug=True, timeout=30.0)})
    assert result == snapshot('localhost:8080')

    m = pydantic_monty.Monty('c.debug', inputs=['c'])
    result = m.run(inputs={'c': Config(host='localhost', port=8080, debug=True, timeout=30.0)})
    assert result is True


def test_namedtuple_custom_input_repr():
    """User-defined NamedTuple has correct repr with fully-qualified type name."""
    from typing import NamedTuple

    class Item(NamedTuple):
        name: str
        price: float

    m = pydantic_monty.Monty('repr(item)', inputs=['item'])
    result = m.run(inputs={'item': Item(name='widget', price=9.99)})
    # Monty uses the full qualified name (module.ClassName) for the type
    assert result == snapshot("test_types.Item(name='widget', price=9.99)")


def test_namedtuple_custom_input_len():
    """User-defined NamedTuple supports len()."""
    from typing import NamedTuple

    class Triple(NamedTuple):
        a: int
        b: int
        c: int

    m = pydantic_monty.Monty('len(t)', inputs=['t'])
    assert m.run(inputs={'t': Triple(a=1, b=2, c=3)}) == snapshot(3)


def test_namedtuple_custom_input_roundtrip():
    """User-defined NamedTuple can be passed through and returned."""
    from typing import NamedTuple

    class Pair(NamedTuple):
        first: int
        second: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Pair(first=1, second=2)})
    # Returns a namedtuple-like object (not the same Python class)
    assert result[0] == snapshot(1)
    assert result[1] == snapshot(2)
    assert result.first == snapshot(1)
    assert result.second == snapshot(2)


def test_namedtuple_custom_missing_attr_error():
    """Accessing non-existent attribute on custom NamedTuple raises AttributeError."""
    from typing import NamedTuple

    class Simple(NamedTuple):
        value: int

    m = pydantic_monty.Monty('s.nonexistent', inputs=['s'])
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(inputs={'s': Simple(value=42)})
    # Monty uses the full qualified name (module.ClassName) for the type
    assert "AttributeError: 'test_types.Simple' object has no attribute 'nonexistent'" in str(exc_info.value)
