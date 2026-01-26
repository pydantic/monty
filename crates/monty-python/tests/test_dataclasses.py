from dataclasses import (
    FrozenInstanceError,
    asdict,
    astuple,
    dataclass,
    fields,
    is_dataclass,
)
from typing import NoReturn

import pytest
from inline_snapshot import snapshot

import pydantic_monty


@dataclass
class Person:
    name: str
    age: int


def test_dataclass_input():
    """Dataclass instances are converted and returned as MontyDataclass."""

    m = pydantic_monty.Monty('x', inputs=['x'])
    m.register_dataclass(Person)
    result = m.run(inputs={'x': Person(name='Alice', age=30)})
    assert result.name == snapshot('Alice')
    assert result.age == snapshot(30)
    assert is_dataclass(result)
    assert isinstance(result, Person)
    assert asdict(result) == snapshot({'name': 'Alice', 'age': 30})
    assert repr(result) == snapshot("Person(name='Alice', age=30)")


def test_dataclass_unknown():
    """Dataclass instances are converted and returned as MontyDataclass."""

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Alice', age=30)})
    assert result.name == snapshot('Alice')
    assert result.age == snapshot(30)
    assert is_dataclass(result)
    assert not isinstance(result, Person)
    assert asdict(result) == snapshot({'name': 'Alice', 'age': 30})  # pyright: ignore[reportArgumentType]
    assert repr(result) == snapshot("<Unknown Dataclass Person(name='Alice', age=30)>")


@dataclass(frozen=True)
class Point:
    x: int
    y: int


def test_dataclass_frozen():
    """Frozen dataclasses are converted like regular dataclasses."""

    m = pydantic_monty.Monty('p', inputs=['p'], dataclass_registry=[Point])
    result = m.run(inputs={'p': Point(x=10, y=20)})
    assert isinstance(result, Point)
    assert result.x == snapshot(10)
    assert result.y == snapshot(20)
    assert repr(result) == snapshot('Point(x=10, y=20)')


@dataclass
class Address:
    city: str
    zip_code: str


@dataclass
class PersonAddress:
    name: str
    address: Address


def test_dataclass_nested():
    """Nested dataclasses are recursively converted."""

    m = pydantic_monty.Monty('x', inputs=['x'])
    m.register_dataclass(Address)
    m.register_dataclass(PersonAddress)
    result = m.run(inputs={'x': PersonAddress(name='Bob', address=Address(city='NYC', zip_code='10001'))})
    assert isinstance(result, PersonAddress)
    assert result.name == snapshot('Bob')
    assert isinstance(result.address, Address)
    assert result.address.city == snapshot('NYC')
    assert result.address.zip_code == snapshot('10001')


def test_dataclass_nested_unknown():
    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': PersonAddress(name='Bob', address=Address(city='NYC', zip_code='10001'))})
    assert not isinstance(result, PersonAddress)
    assert result.name == snapshot('Bob')
    assert not isinstance(result.address, Address)
    assert result.address.city == snapshot('NYC')
    assert result.address.zip_code == snapshot('10001')


def test_dataclass_with_list_field():
    """Dataclasses with list fields are properly converted."""

    @dataclass
    class Container:
        items: list[int]

    m = pydantic_monty.Monty('x', inputs=['x'])
    m.register_dataclass(Container)
    result = m.run(inputs={'x': Container(items=[1, 2, 3])})
    assert result.items == snapshot([1, 2, 3])


def test_dataclass_with_dict_field():
    """Dataclasses with dict fields are properly converted."""

    @dataclass
    class Config:
        settings: dict[str, int]

    m = pydantic_monty.Monty('x', inputs=['x'])
    m.register_dataclass(Config)
    result = m.run(inputs={'x': Config(settings={'a': 1, 'b': 2})})
    assert result.settings == snapshot({'a': 1, 'b': 2})


def test_dataclass_empty():
    """Empty dataclass (no fields) has empty repr."""

    @dataclass
    class Empty:
        pass

    m = pydantic_monty.Monty('x', inputs=['x'])
    m.register_dataclass(Empty)
    result = m.run(inputs={'x': Empty()})
    assert repr(result) == snapshot('test_dataclass_empty.<locals>.Empty()')


def test_dataclass_type_raises():
    """Dataclass type (not instance) should raise TypeError."""

    @dataclass
    class MyClass:
        value: int

    m = pydantic_monty.Monty('x', inputs=['x'])
    m.register_dataclass(MyClass)
    with pytest.raises(TypeError, match='Cannot convert type to Monty value'):
        m.run(inputs={'x': MyClass})


# === Field access ===


def test_dataclass_field_access():
    """Access individual fields of a dataclass."""

    @dataclass
    class Person:
        name: str
        age: int

    m = pydantic_monty.Monty('x.name', inputs=['x'])
    assert m.run(inputs={'x': Person(name='Alice', age=30)}) == snapshot('Alice')

    m = pydantic_monty.Monty('x.age', inputs=['x'])
    assert m.run(inputs={'x': Person(name='Alice', age=30)}) == snapshot(30)


def test_dataclass_field_access_nested():
    """Access fields of nested dataclasses."""

    m = pydantic_monty.Monty('x.address.city', inputs=['x'])
    result = m.run(inputs={'x': PersonAddress(name='Bob', address=Address(city='NYC', zip_code='10001'))})
    assert result == snapshot('NYC')


def test_dataclass_field_in_expression():
    """Use dataclass fields in expressions."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p.x + p.y', inputs=['p'])
    assert m.run(inputs={'p': Point(x=10, y=20)}) == snapshot(30)


def test_dataclass_field_access_missing():
    """Accessing a non-existent field raises AttributeError."""

    @dataclass
    class Person:
        name: str

    m = pydantic_monty.Monty('x.age', inputs=['x'])
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(inputs={'x': Person(name='Alice')})
    assert isinstance(exc_info.value.exception(), AttributeError)


# === Repr ===


def test_dataclass_repr():
    """Repr of dataclass shows ClassName(field=value, ...) format."""

    @dataclass
    class Person:
        name: str
        age: int

    m = pydantic_monty.Monty('repr(x)', inputs=['x'])
    assert m.run(inputs={'x': Person(name='Alice', age=30)}) == snapshot("Person(name='Alice', age=30)")


def test_dataclass_repr_frozen():
    """Repr of frozen dataclass shows same format."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('repr(p)', inputs=['p'])
    assert m.run(inputs={'p': Point(x=10, y=20)}) == snapshot('Point(x=10, y=20)')


def test_dataclass_repr_nested():
    """Repr of nested dataclass shows nested repr."""

    @dataclass
    class Inner:
        value: int

    @dataclass
    class Outer:
        inner: Inner

    m = pydantic_monty.Monty('repr(x)', inputs=['x'])
    assert m.run(inputs={'x': Outer(inner=Inner(value=42))}) == snapshot('Outer(inner=Inner(value=42))')


def test_dataclass_repr_empty():
    """Repr of empty dataclass shows ClassName()."""

    @dataclass
    class Empty:
        pass

    m = pydantic_monty.Monty('repr(x)', inputs=['x'])
    m.register_dataclass(Empty)
    assert m.run(inputs={'x': Empty()}) == snapshot('Empty()')


# === Setattr ===


def test_dataclass_setattr_mutable_unknown():
    """Setting attributes on mutable dataclass works."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    # Modify existing field
    result.x = 100
    assert result.x == snapshot(100)
    assert repr(result) == snapshot('<Unknown Dataclass Point(x=100, y=20)>')

    # Add new attribute (not in repr since not a declared field)
    result.z = 30
    assert result.z == snapshot(30)
    assert repr(result) == snapshot('<Unknown Dataclass Point(x=100, y=20)>')


def test_dataclass_setattr_frozen():
    """Setting attributes on frozen dataclass raises FrozenInstanceError."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    # FrozenInstanceError is raised (which is a subclass of AttributeError)
    with pytest.raises(FrozenInstanceError, match="cannot assign to field 'x'"):
        result.x = 100

    with pytest.raises(FrozenInstanceError, match="cannot assign to field 'z'"):
        result.z = 30


def test_frozen_instance_error_is_attribute_error():
    """FrozenInstanceError can be caught as AttributeError."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    # Can catch with AttributeError (parent class)
    with pytest.raises(AttributeError):
        result.x = 100

    # Verify it's actually FrozenInstanceError
    try:
        result.y = 200
    except AttributeError as e:
        assert isinstance(e, FrozenInstanceError)


def test_frozen_instance_error_message():
    """FrozenInstanceError has correct message format."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    with pytest.raises(FrozenInstanceError) as exc_info:
        result.x = 100
    assert exc_info.value.args[0] == snapshot("cannot assign to field 'x'")


def test_frozen_instance_error_from_monty_code():
    """FrozenInstanceError raised by Monty code is properly converted."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    # Monty code that tries to modify a frozen dataclass
    code = """
p.x = 100
"""
    m = pydantic_monty.Monty(code, inputs=['p'])

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(inputs={'p': Point(x=10, y=20)})
    inner = exc_info.value.exception()
    assert isinstance(inner, FrozenInstanceError)
    assert inner.args[0] == snapshot("cannot assign to field 'x'")


def test_frozen_instance_error_from_monty_caught_as_attribute_error():
    """FrozenInstanceError from Monty can be caught as AttributeError."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    code = 'p.x = 100'
    m = pydantic_monty.Monty(code, inputs=['p'])

    # Wrapped in MontyRuntimeError, but inner exception is FrozenInstanceError
    # which is a subclass of AttributeError
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(inputs={'p': Point(x=10, y=20)})
    inner = exc_info.value.exception()
    assert isinstance(inner, AttributeError)
    assert isinstance(inner, FrozenInstanceError)


def test_frozen_instance_error_from_external_function():
    """FrozenInstanceError from external function is properly converted."""
    code = """
try:
    fail()
except FrozenInstanceError:
    caught = 'frozen'
except AttributeError:
    caught = 'attr'
caught
"""
    m = pydantic_monty.Monty(code, external_functions=['fail'])

    def fail() -> NoReturn:
        raise FrozenInstanceError('cannot assign to field')

    # Monty should catch it as FrozenInstanceError specifically
    result = m.run(external_functions={'fail': fail})
    assert result == snapshot('frozen')


def test_frozen_instance_error_from_external_function_propagates():
    """FrozenInstanceError from external function propagates to Python."""
    m = pydantic_monty.Monty('fail()', external_functions=['fail'])

    def fail() -> NoReturn:
        raise FrozenInstanceError('test frozen error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, FrozenInstanceError)
    assert inner.args[0] == snapshot('test frozen error')


# === Equality ===


def test_dataclass_equality_same():
    """Equal dataclasses compare equal."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})
    assert a == b


def test_dataclass_equality_different_values():
    """Dataclasses with different values compare not equal."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=30)})
    assert a != b


def test_dataclass_equality_different_types():
    """Dataclasses of different types compare not equal."""

    @dataclass
    class Point:
        x: int
        y: int

    @dataclass
    class Vector:
        x: int
        y: int

    m = pydantic_monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Vector(x=10, y=20)})
    assert a != b


def test_dataclass_equality_with_other_type():
    """Dataclass compared to non-dataclass returns False."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})
    assert result != {'x': 10, 'y': 20}
    assert result != (10, 20)
    assert result != 'Point(x=10, y=20)'


# === Hashing ===


def test_dataclass_hash_frozen():
    """Frozen dataclasses are hashable."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    h = hash(result)
    assert isinstance(h, int)
    # Hash is consistent
    assert hash(result) == h


def test_dataclass_hash_frozen_equal_values():
    """Equal frozen dataclasses have equal hashes."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})

    assert hash(a) == hash(b)


def test_dataclass_hash_mutable_raises():
    """Mutable dataclasses are not hashable."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    with pytest.raises(TypeError, match="unhashable type: 'UnknownDataclass'"):
        hash(result)


def test_dataclass_hash_in_set():
    """Frozen dataclasses can be used in sets."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('(a, b, c)', inputs=['a', 'b', 'c'])
    a, b, c = m.run(
        inputs={
            'a': Point(x=10, y=20),
            'b': Point(x=10, y=20),  # duplicate
            'c': Point(x=30, y=40),
        }
    )

    s = {a, b, c}
    assert len(s) == snapshot(2)


def test_dataclass_hash_as_dict_key():
    """Frozen dataclasses can be used as dict keys."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})

    d = {a: 'first'}
    assert d[b] == snapshot('first')


# === dataclasses module compatibility ===


def test_dataclass_is_dataclass():
    """is_dataclass() returns True for returned dataclasses."""

    @dataclass
    class Person:
        name: str
        age: int

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Alice', age=30)})
    assert is_dataclass(result) is True


def test_dataclass_fields():
    """fields() returns Field objects for returned dataclasses."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    fs = fields(result)
    assert len(fs) == snapshot(2)
    assert fs[0].name == snapshot('x')
    assert fs[1].name == snapshot('y')
    # Type is inferred from value
    assert fs[0].type is int
    assert fs[1].type is int


def test_dataclass_fields_string():
    """fields() returns correct type for string fields."""

    @dataclass
    class Person:
        name: str

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Person(name='Alice')})

    fs = fields(result)
    assert fs[0].name == snapshot('name')
    assert fs[0].type is str


def test_dataclass_asdict():
    """asdict() converts returned dataclass to dict."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    d = asdict(result)
    assert d == snapshot({'x': 10, 'y': 20})


def test_dataclass_asdict_nested():
    """asdict() recursively converts nested dataclasses."""

    @dataclass
    class Inner:
        value: int

    @dataclass
    class Outer:
        inner: Inner

    m = pydantic_monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Outer(inner=Inner(value=42))})

    d = asdict(result)
    assert d == snapshot({'inner': {'value': 42}})


def test_dataclass_astuple():
    """astuple() converts returned dataclass to tuple."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    t = astuple(result)
    assert t == snapshot((10, 20))


def test_dataclass_dataclass_fields_attr():
    """__dataclass_fields__ attribute is accessible."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    df = result.__dataclass_fields__
    assert 'x' in df
    assert 'y' in df
    assert df['x'].name == snapshot('x')
    assert df['y'].name == snapshot('y')


def test_dataclass_params_frozen():
    """__dataclass_params__.frozen reflects frozen status."""

    @dataclass(frozen=True)
    class FrozenPoint:
        x: int
        y: int

    @dataclass
    class MutablePoint:
        x: int
        y: int

    m = pydantic_monty.Monty('(f, m)', inputs=['f', 'm'])
    frozen, mutable = m.run(inputs={'f': FrozenPoint(x=1, y=2), 'm': MutablePoint(x=3, y=4)})

    assert frozen.__dataclass_params__.frozen is True
    assert mutable.__dataclass_params__.frozen is False


def test_dataclass_params_attributes():
    """__dataclass_params__ has expected attributes."""

    @dataclass
    class Point:
        x: int
        y: int

    m = pydantic_monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    params = result.__dataclass_params__
    assert params.init is True
    assert params.repr is True
    assert params.eq is True
    assert params.order is False
    assert params.frozen is False


def test_repeat_dataclass_name():
    """Two classes with the same name are distinguished because we use id, not name."""

    def create_point():
        @dataclass
        class Point:
            x: int
            y: int

        return Point

    point_cls2 = create_point()
    m = pydantic_monty.Monty('a, b', inputs=['a', 'b'], dataclass_registry=[Point, point_cls2])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': point_cls2(x=30, y=40)})
    assert isinstance(a, Point)
    assert isinstance(b, point_cls2)
