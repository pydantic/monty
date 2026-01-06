from dataclasses import dataclass

import pytest
from inline_snapshot import snapshot

import monty


def test_dataclass_input():
    """Dataclass instances are converted and returned as dicts."""

    @dataclass
    class Person:
        name: str
        age: int

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Person(name='Alice', age=30)})
    assert result == snapshot({'name': 'Alice', 'age': 30})


def test_dataclass_frozen():
    """Frozen dataclasses are converted like regular dataclasses."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    m = monty.Monty('p', inputs=['p'])
    result = m.run(inputs={'p': Point(x=10, y=20)})
    assert result == snapshot({'x': 10, 'y': 20})


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
    assert result == snapshot({'name': 'Bob', 'address': {'city': 'NYC', 'zip_code': '10001'}})


def test_dataclass_with_list_field():
    """Dataclasses with list fields are properly converted."""

    @dataclass
    class Container:
        items: list[int]

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Container(items=[1, 2, 3])})
    assert result == snapshot({'items': [1, 2, 3]})


def test_dataclass_with_dict_field():
    """Dataclasses with dict fields are properly converted."""

    @dataclass
    class Config:
        settings: dict[str, int]

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Config(settings={'a': 1, 'b': 2})})
    assert result == snapshot({'settings': {'a': 1, 'b': 2}})


def test_dataclass_empty():
    """Empty dataclass (no fields) is converted to empty dict."""

    @dataclass
    class Empty:
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': Empty()})
    assert result == snapshot({})


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
