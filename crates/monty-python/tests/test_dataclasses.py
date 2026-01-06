from dataclasses import dataclass

import pytest
from inline_snapshot import snapshot

import monty


def test_dataclass_input():
    """Dataclass instances are converted and returned as MontyDataclass."""

    @dataclass
    class Person:
        name: str
        age: int

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Alice', age=30)})
    assert result.name == snapshot('Alice')
    assert result.age == snapshot(30)
    assert repr(result) == snapshot("Person(name='Alice', age=30)")


def test_dataclass_frozen():
    """Frozen dataclasses are converted like regular dataclasses."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})
    assert result.x == snapshot(10)
    assert result.y == snapshot(20)
    assert repr(result) == snapshot('Point(x=10, y=20)')


def test_dataclass_nested():
    """Nested dataclasses are recursively converted."""

    @dataclass
    class Address:
        city: str
        zip_code: str

    @dataclass
    class Person:
        name: str
        address: Address

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Bob', address=Address(city='NYC', zip_code='10001'))})
    assert result.name == snapshot('Bob')
    assert result.address.city == snapshot('NYC')
    assert result.address.zip_code == snapshot('10001')


def test_dataclass_with_list_field():
    """Dataclasses with list fields are properly converted."""

    @dataclass
    class Container:
        items: list[int]

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Container(items=[1, 2, 3])})
    assert result.items == snapshot([1, 2, 3])


def test_dataclass_with_dict_field():
    """Dataclasses with dict fields are properly converted."""

    @dataclass
    class Config:
        settings: dict[str, int]

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Config(settings={'a': 1, 'b': 2})})
    assert result.settings == snapshot({'a': 1, 'b': 2})


def test_dataclass_empty():
    """Empty dataclass (no fields) has empty repr."""

    @dataclass
    class Empty:
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Empty()})
    assert repr(result) == snapshot('Empty()')


def test_dataclass_type_raises():
    """Dataclass type (not instance) should raise TypeError."""

    @dataclass
    class MyClass:
        value: int

    m = monty.Monty('x', inputs=['x'])
    with pytest.raises(TypeError, match='Cannot convert type to Monty value'):
        m.run(inputs={'x': MyClass})


# === Field access ===


def test_dataclass_field_access():
    """Access individual fields of a dataclass."""

    @dataclass
    class Person:
        name: str
        age: int

    m = monty.Monty('x.name', inputs=['x'])
    assert m.run(inputs={'x': Person(name='Alice', age=30)}) == snapshot('Alice')

    m = monty.Monty('x.age', inputs=['x'])
    assert m.run(inputs={'x': Person(name='Alice', age=30)}) == snapshot(30)


def test_dataclass_field_access_nested():
    """Access fields of nested dataclasses."""

    @dataclass
    class Address:
        city: str
        zip_code: str

    @dataclass
    class Person:
        name: str
        address: Address

    m = monty.Monty('x.address.city', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Bob', address=Address(city='NYC', zip_code='10001'))})
    assert result == snapshot('NYC')


def test_dataclass_field_in_expression():
    """Use dataclass fields in expressions."""

    @dataclass
    class Point:
        x: int
        y: int

    m = monty.Monty('p.x + p.y', inputs=['p'])
    assert m.run(inputs={'p': Point(x=10, y=20)}) == snapshot(30)


def test_dataclass_field_access_missing():
    """Accessing a non-existent field raises AttributeError."""

    @dataclass
    class Person:
        name: str

    m = monty.Monty('x.age', inputs=['x'])
    with pytest.raises(AttributeError):
        m.run(inputs={'x': Person(name='Alice')})


# === Repr ===


def test_dataclass_repr():
    """Repr of dataclass shows ClassName(field=value, ...) format."""

    @dataclass
    class Person:
        name: str
        age: int

    m = monty.Monty('repr(x)', inputs=['x'])
    assert m.run(inputs={'x': Person(name='Alice', age=30)}) == snapshot("Person(name='Alice', age=30)")


def test_dataclass_repr_frozen():
    """Repr of frozen dataclass shows same format."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = monty.Monty('repr(p)', inputs=['p'])
    assert m.run(inputs={'p': Point(x=10, y=20)}) == snapshot('Point(x=10, y=20)')


def test_dataclass_repr_nested():
    """Repr of nested dataclass shows nested repr."""

    @dataclass
    class Inner:
        value: int

    @dataclass
    class Outer:
        inner: Inner

    m = monty.Monty('repr(x)', inputs=['x'])
    assert m.run(inputs={'x': Outer(inner=Inner(value=42))}) == snapshot('Outer(inner=Inner(value=42))')


def test_dataclass_repr_empty():
    """Repr of empty dataclass shows ClassName()."""

    @dataclass
    class Empty:
        pass

    m = monty.Monty('repr(x)', inputs=['x'])
    assert m.run(inputs={'x': Empty()}) == snapshot('Empty()')


# === Name attributes ===


def test_dataclass_name():
    """Access __name__ of returned dataclass."""

    @dataclass
    class Person:
        name: str
        age: int

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Alice', age=30)})
    assert result.__name__ == snapshot('Person')


def test_dataclass_qualname():
    """Access __qualname__ of returned dataclass (same as __name__)."""

    @dataclass
    class Person:
        name: str
        age: int

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Alice', age=30)})
    # MontyDataclass returns __name__ for __qualname__ since we don't track nesting
    assert result.__qualname__ == snapshot('Person')


# === Setattr ===


def test_dataclass_setattr_mutable():
    """Setting attributes on mutable dataclass works."""

    @dataclass
    class Point:
        x: int
        y: int

    m = monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    # Modify existing field
    result.x = 100
    assert result.x == snapshot(100)
    assert repr(result) == snapshot('Point(x=100, y=20)')

    # Add new attribute (not in repr since not a declared field)
    result.z = 30
    assert result.z == snapshot(30)
    assert repr(result) == snapshot('Point(x=100, y=20)')


def test_dataclass_setattr_frozen():
    """Setting attributes on frozen dataclass raises AttributeError."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    with pytest.raises(AttributeError, match="cannot assign to field 'x'"):
        result.x = 100

    with pytest.raises(AttributeError, match="cannot assign to field 'z'"):
        result.z = 30


# === Equality ===


def test_dataclass_equality_same():
    """Equal dataclasses compare equal."""

    @dataclass
    class Point:
        x: int
        y: int

    m = monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})
    assert a == b


def test_dataclass_equality_different_values():
    """Dataclasses with different values compare not equal."""

    @dataclass
    class Point:
        x: int
        y: int

    m = monty.Monty('(a, b)', inputs=['a', 'b'])
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

    m = monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Vector(x=10, y=20)})
    assert a != b


def test_dataclass_equality_with_other_type():
    """Dataclass compared to non-dataclass returns False."""

    @dataclass
    class Point:
        x: int
        y: int

    m = monty.Monty('p', inputs=['p'])
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

    m = monty.Monty('p', inputs=['p'])
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

    m = monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})

    assert hash(a) == hash(b)


def test_dataclass_hash_mutable_raises():
    """Mutable dataclasses are not hashable."""

    @dataclass
    class Point:
        x: int
        y: int

    m = monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})

    with pytest.raises(TypeError, match="unhashable type: 'Point'"):
        hash(result)


def test_dataclass_hash_in_set():
    """Frozen dataclasses can be used in sets."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = monty.Monty('(a, b, c)', inputs=['a', 'b', 'c'])
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

    m = monty.Monty('(a, b)', inputs=['a', 'b'])
    a, b = m.run(inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})

    d = {a: 'first'}
    assert d[b] == snapshot('first')
