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
from conftest import RunMonty
from inline_snapshot import snapshot

import pydantic_monty


@dataclass
class Person:
    name: str
    age: int


def test_dataclass_input(monty_run: RunMonty):
    """Dataclass instances are converted and returned as the registered dataclass."""

    result = monty_run('x', inputs={'x': Person(name='Alice', age=30)}, dataclass_registry=[Person])
    assert result.name == snapshot('Alice')
    assert result.age == snapshot(30)
    assert is_dataclass(result)
    assert isinstance(result, Person)
    assert asdict(result) == snapshot({'name': 'Alice', 'age': 30})
    assert repr(result) == snapshot("Person(name='Alice', age=30)")


def test_dataclass_auto_registered(monty_run: RunMonty):
    """Dataclass passed as input is auto-registered, so isinstance() works without explicit registry."""

    result = monty_run('x', inputs={'x': Person(name='Alice', age=30)})
    assert result.name == snapshot('Alice')
    assert result.age == snapshot(30)
    assert is_dataclass(result)
    assert isinstance(result, Person)
    assert asdict(result) == snapshot({'name': 'Alice', 'age': 30})
    assert repr(result) == snapshot("Person(name='Alice', age=30)")


@dataclass(frozen=True)
class Point:
    x: int
    y: int


def test_dataclass_frozen(monty_run: RunMonty):
    """Frozen dataclasses are converted like regular dataclasses."""

    result = monty_run('p', inputs={'p': Point(x=10, y=20)}, dataclass_registry=[Point])
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


def test_dataclass_nested(monty_run: RunMonty):
    """Nested dataclasses are recursively converted."""

    result = monty_run(
        'x',
        inputs={'x': PersonAddress(name='Bob', address=Address(city='NYC', zip_code='10001'))},
        dataclass_registry=[Address, PersonAddress],
    )
    assert isinstance(result, PersonAddress)
    assert result.name == snapshot('Bob')
    assert isinstance(result.address, Address)
    assert result.address.city == snapshot('NYC')
    assert result.address.zip_code == snapshot('10001')


def test_dataclass_nested_auto_registered(monty_run: RunMonty):
    """Nested dataclasses are auto-registered when passed as input."""
    result = monty_run('x', inputs={'x': PersonAddress(name='Bob', address=Address(city='NYC', zip_code='10001'))})
    assert isinstance(result, PersonAddress)
    assert result.name == snapshot('Bob')
    assert isinstance(result.address, Address)
    assert result.address.city == snapshot('NYC')
    assert result.address.zip_code == snapshot('10001')


def test_dataclass_auto_registered_in_list(monty_run: RunMonty):
    """Dataclass inside a list input is auto-registered."""

    result = monty_run('x[0]', inputs={'x': [Person(name='Alice', age=30)]})
    assert isinstance(result, Person)
    assert result.name == snapshot('Alice')


def test_dataclass_auto_registered_in_dict_value(monty_run: RunMonty):
    """Dataclass inside a dict value is auto-registered."""

    result = monty_run('x["key"]', inputs={'x': {'key': Person(name='Alice', age=30)}})
    assert isinstance(result, Person)
    assert result.name == snapshot('Alice')


def test_dataclass_explicit_registry_idempotent(monty_run: RunMonty):
    """Explicit registry still works alongside auto-registration (idempotent)."""

    result = monty_run('x', inputs={'x': Person(name='Alice', age=30)}, dataclass_registry=[Person])
    assert isinstance(result, Person)
    assert result.name == snapshot('Alice')
    assert result.age == snapshot(30)


def test_dataclass_with_list_field(monty_run: RunMonty):
    """Dataclasses with list fields are properly converted."""

    @dataclass
    class Container:
        items: list[int]

    result = monty_run('x', inputs={'x': Container(items=[1, 2, 3])})
    assert result.items == snapshot([1, 2, 3])


def test_dataclass_with_dict_field(monty_run: RunMonty):
    """Dataclasses with dict fields are properly converted."""

    @dataclass
    class Config:
        settings: dict[str, int]

    result = monty_run('x', inputs={'x': Config(settings={'a': 1, 'b': 2})}, dataclass_registry=[Config])
    assert result.settings == snapshot({'a': 1, 'b': 2})


def test_dataclass_empty(monty_run: RunMonty):
    """Empty dataclass (no fields) has empty repr."""

    @dataclass
    class Empty:
        pass

    result = monty_run('x', inputs={'x': Empty()}, dataclass_registry=[Empty])
    assert repr(result) == snapshot('test_dataclass_empty.<locals>.Empty()')


@pytest.mark.xfail(reason='We should extend the dataclass registry to cover all types, then test it is enforced')
def test_dataclass_type_raises(monty_run: RunMonty):
    """Dataclass type (not instance) should raise TypeError."""

    @dataclass
    class MyClass:
        value: int

    with pytest.raises(TypeError) as exc_info:
        monty_run('x', inputs={'x': MyClass}, dataclass_registry=[MyClass])

    assert str(exc_info.value) == snapshot('Cannot convert builtins.type to Monty value')


# === Field access ===


def test_dataclass_field_access(monty_run: RunMonty):
    """Access individual fields of a dataclass."""

    @dataclass
    class Person:
        name: str
        age: int

    assert monty_run('x.name', inputs={'x': Person(name='Alice', age=30)}) == snapshot('Alice')
    assert monty_run('x.age', inputs={'x': Person(name='Alice', age=30)}) == snapshot(30)


def test_dataclass_field_access_nested(monty_run: RunMonty):
    """Access fields of nested dataclasses."""

    result = monty_run(
        'x.address.city', inputs={'x': PersonAddress(name='Bob', address=Address(city='NYC', zip_code='10001'))}
    )
    assert result == snapshot('NYC')


def test_dataclass_field_in_expression(monty_run: RunMonty):
    """Use dataclass fields in expressions."""

    @dataclass
    class Point:
        x: int
        y: int

    assert monty_run('p.x + p.y', inputs={'p': Point(x=10, y=20)}) == snapshot(30)


def test_dataclass_field_access_missing(monty_run: RunMonty):
    """Accessing a non-existent field raises AttributeError."""

    @dataclass
    class Person:
        name: str

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('x.age', inputs={'x': Person(name='Alice')})
    assert isinstance(exc_info.value.exception(), AttributeError)


def test_dataclass_private_method_not_dispatched(monty_run: RunMonty):
    @dataclass
    class Secret:
        value: int

        def _leak(self) -> int:
            return self.value

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('x._leak()', inputs={'x': Secret(value=42)})
    assert isinstance(exc_info.value.exception(), AttributeError)
    assert str(exc_info.value) == snapshot("AttributeError: 'Secret' object has no attribute '_leak'")


# === Repr ===


def test_dataclass_repr(monty_run: RunMonty):
    """Repr of dataclass shows ClassName(field=value, ...) format."""

    @dataclass
    class Person:
        name: str
        age: int

    assert monty_run('repr(x)', inputs={'x': Person(name='Alice', age=30)}) == snapshot("Person(name='Alice', age=30)")


def test_dataclass_repr_frozen(monty_run: RunMonty):
    """Repr of frozen dataclass shows same format."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    assert monty_run('repr(p)', inputs={'p': Point(x=10, y=20)}) == snapshot('Point(x=10, y=20)')


def test_dataclass_repr_nested(monty_run: RunMonty):
    """Repr of nested dataclass shows nested repr."""

    @dataclass
    class Inner:
        value: int

    @dataclass
    class Outer:
        inner: Inner

    assert monty_run('repr(x)', inputs={'x': Outer(inner=Inner(value=42))}) == snapshot('Outer(inner=Inner(value=42))')


def test_dataclass_repr_empty(monty_run: RunMonty):
    """Repr of empty dataclass shows ClassName()."""

    @dataclass
    class Empty:
        pass

    assert monty_run('repr(x)', inputs={'x': Empty()}, dataclass_registry=[Empty]) == snapshot('Empty()')


# === Setattr ===


def test_dataclass_setattr_mutable(monty_run: RunMonty):
    """Setting attributes on mutable dataclass works (auto-registered, returns real dataclass)."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})
    assert isinstance(result, Point)

    # Modify existing field
    result.x = 100
    assert result.x == snapshot(100)
    assert repr(result) == snapshot('test_dataclass_setattr_mutable.<locals>.Point(x=100, y=20)')


def test_dataclass_setattr_frozen(monty_run: RunMonty):
    """Setting attributes on frozen dataclass raises FrozenInstanceError."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    # FrozenInstanceError is raised (which is a subclass of AttributeError)
    with pytest.raises(FrozenInstanceError, match="cannot assign to field 'x'"):
        result.x = 100

    with pytest.raises(FrozenInstanceError, match="cannot assign to field 'z'"):
        result.z = 30


def test_frozen_instance_error_is_attribute_error(monty_run: RunMonty):
    """FrozenInstanceError can be caught as AttributeError."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    # Can catch with AttributeError (parent class)
    with pytest.raises(AttributeError):
        result.x = 100

    # Verify it's actually FrozenInstanceError
    try:
        result.y = 200
    except AttributeError as e:
        assert isinstance(e, FrozenInstanceError)


def test_frozen_instance_error_message(monty_run: RunMonty):
    """FrozenInstanceError has correct message format."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    with pytest.raises(FrozenInstanceError) as exc_info:
        result.x = 100
    assert exc_info.value.args[0] == snapshot("cannot assign to field 'x'")


def test_frozen_instance_error_from_monty_code(monty_run: RunMonty):
    """FrozenInstanceError raised by Monty code is properly converted."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('p.x = 100', inputs={'p': Point(x=10, y=20)})
    inner = exc_info.value.exception()
    assert isinstance(inner, FrozenInstanceError)
    assert inner.args[0] == snapshot("cannot assign to field 'x'")


def test_frozen_instance_error_from_monty_caught_as_attribute_error(monty_run: RunMonty):
    """FrozenInstanceError from Monty can be caught as AttributeError."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    # Wrapped in MontyRuntimeError, but inner exception is FrozenInstanceError
    # which is a subclass of AttributeError
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('p.x = 100', inputs={'p': Point(x=10, y=20)})
    inner = exc_info.value.exception()
    assert isinstance(inner, AttributeError)
    assert isinstance(inner, FrozenInstanceError)


def test_frozen_instance_error_from_external_function(monty_run: RunMonty):
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

    def fail() -> NoReturn:
        raise FrozenInstanceError('cannot assign to field')

    # Monty should catch it as FrozenInstanceError specifically
    result = monty_run(code, external_lookup={'fail': fail})
    assert result == snapshot('frozen')


def test_frozen_instance_error_from_external_function_propagates(monty_run: RunMonty):
    """FrozenInstanceError from external function propagates to Python."""

    def fail() -> NoReturn:
        raise FrozenInstanceError('test frozen error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, FrozenInstanceError)
    assert inner.args[0] == snapshot('test frozen error')


# === Equality ===


def test_dataclass_equality_same(monty_run: RunMonty):
    """Equal dataclasses compare equal."""

    @dataclass
    class Point:
        x: int
        y: int

    a, b = monty_run('(a, b)', inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})
    assert a == b


def test_dataclass_equality_different_values(monty_run: RunMonty):
    """Dataclasses with different values compare not equal."""

    @dataclass
    class Point:
        x: int
        y: int

    a, b = monty_run('(a, b)', inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=30)})
    assert a != b


def test_dataclass_equality_different_types(monty_run: RunMonty):
    """Dataclasses of different types compare not equal."""

    @dataclass
    class Point:
        x: int
        y: int

    @dataclass
    class Vector:
        x: int
        y: int

    a, b = monty_run('(a, b)', inputs={'a': Point(x=10, y=20), 'b': Vector(x=10, y=20)})
    assert a != b


def test_dataclass_equality_with_other_type(monty_run: RunMonty):
    """Dataclass compared to non-dataclass returns False."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})
    assert result != {'x': 10, 'y': 20}
    assert result != (10, 20)
    assert result != 'Point(x=10, y=20)'


# === Hashing ===


def test_dataclass_hash_frozen(monty_run: RunMonty):
    """Frozen dataclasses are hashable."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    h = hash(result)
    assert isinstance(h, int)
    # Hash is consistent
    assert hash(result) == h


def test_dataclass_hash_frozen_equal_values(monty_run: RunMonty):
    """Equal frozen dataclasses have equal hashes."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    a, b = monty_run('(a, b)', inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})

    assert hash(a) == hash(b)


def test_dataclass_hash_mutable_raises(monty_run: RunMonty):
    """Mutable dataclasses are not hashable."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    with pytest.raises(TypeError, match="unhashable type: 'Point'"):
        hash(result)


def test_dataclass_hash_in_set(monty_run: RunMonty):
    """Frozen dataclasses can be used in sets."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    a, b, c = monty_run(
        '(a, b, c)',
        inputs={
            'a': Point(x=10, y=20),
            'b': Point(x=10, y=20),  # duplicate
            'c': Point(x=30, y=40),
        },
    )

    s = {a, b, c}
    assert len(s) == snapshot(2)


def test_dataclass_hash_as_dict_key(monty_run: RunMonty):
    """Frozen dataclasses can be used as dict keys."""

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    a, b = monty_run('(a, b)', inputs={'a': Point(x=10, y=20), 'b': Point(x=10, y=20)})

    d = {a: 'first'}
    assert d[b] == snapshot('first')


# === dataclasses module compatibility ===


def test_dataclass_is_dataclass(monty_run: RunMonty):
    """is_dataclass() returns True for returned dataclasses."""

    @dataclass
    class Person:
        name: str
        age: int

    result = monty_run('x', inputs={'x': Person(name='Alice', age=30)})
    assert is_dataclass(result) is True


def test_dataclass_fields(monty_run: RunMonty):
    """fields() returns Field objects for returned dataclasses."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    fs = fields(result)
    assert len(fs) == snapshot(2)
    assert fs[0].name == snapshot('x')
    assert fs[1].name == snapshot('y')
    # Type is inferred from value
    assert fs[0].type is int
    assert fs[1].type is int


def test_dataclass_fields_string(monty_run: RunMonty):
    """fields() returns correct type for string fields."""

    @dataclass
    class Person:
        name: str

    result = monty_run('p', inputs={'p': Person(name='Alice')})

    fs = fields(result)
    assert fs[0].name == snapshot('name')
    assert fs[0].type is str


def test_dataclass_asdict(monty_run: RunMonty):
    """asdict() converts returned dataclass to dict."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    d = asdict(result)
    assert d == snapshot({'x': 10, 'y': 20})


def test_dataclass_asdict_nested(monty_run: RunMonty):
    """asdict() recursively converts nested dataclasses."""

    @dataclass
    class Inner:
        value: int

    @dataclass
    class Outer:
        inner: Inner

    result = monty_run('x', inputs={'x': Outer(inner=Inner(value=42))})

    d = asdict(result)
    assert d == snapshot({'inner': {'value': 42}})


def test_dataclass_astuple(monty_run: RunMonty):
    """astuple() converts returned dataclass to tuple."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    t = astuple(result)
    assert t == snapshot((10, 20))


def test_dataclass_dataclass_fields_attr(monty_run: RunMonty):
    """__dataclass_fields__ attribute is accessible."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    df = result.__dataclass_fields__
    assert 'x' in df
    assert 'y' in df
    assert df['x'].name == snapshot('x')
    assert df['y'].name == snapshot('y')


def test_dataclass_params_frozen(monty_run: RunMonty):
    """__dataclass_params__.frozen reflects frozen status."""

    @dataclass(frozen=True)
    class FrozenPoint:
        x: int
        y: int

    @dataclass
    class MutablePoint:
        x: int
        y: int

    frozen, mutable = monty_run('(f, m)', inputs={'f': FrozenPoint(x=1, y=2), 'm': MutablePoint(x=3, y=4)})

    assert frozen.__dataclass_params__.frozen is True
    assert mutable.__dataclass_params__.frozen is False


def test_dataclass_params_attributes(monty_run: RunMonty):
    """__dataclass_params__ has expected attributes."""

    @dataclass
    class Point:
        x: int
        y: int

    result = monty_run('p', inputs={'p': Point(x=10, y=20)})

    params = result.__dataclass_params__
    assert params.init is True
    assert params.repr is True
    assert params.eq is True
    assert params.order is False
    assert params.frozen is False


def test_repeat_dataclass_name(monty_run: RunMonty):
    """Two classes with the same name are distinguished because we use id, not name."""

    def create_point():
        @dataclass
        class Point:
            x: int
            y: int

        return Point

    point_cls2 = create_point()
    a, b = monty_run(
        'a, b',
        inputs={'a': Point(x=10, y=20), 'b': point_cls2(x=30, y=40)},
        dataclass_registry=[Point, point_cls2],
    )
    assert isinstance(a, Point)
    assert isinstance(b, point_cls2)


# === Dataclass method call tests ===


@dataclass
class Greeter:
    greeting: str

    def greet(self) -> str:
        return self.greeting


@dataclass
class Calculator:
    value: int

    def add(self, n: int) -> int:
        return self.value + n

    def multiply(self, n: int) -> int:
        return self.value * n


@dataclass
class Point2D:
    x: float
    y: float

    def distance(self) -> float:
        return (self.x**2 + self.y**2) ** 0.5

    def translate(self, dx: float, dy: float) -> 'Point2D':
        return Point2D(x=self.x + dx, y=self.y + dy)


def test_method_no_args(monty_run: RunMonty):
    """Calling a dataclass method with no args (besides self)."""
    result = monty_run('g.greet()', inputs={'g': Greeter(greeting='hello')}, dataclass_registry=[Greeter])
    assert result == snapshot('hello')


def test_method_with_args(monty_run: RunMonty):
    """Calling a dataclass method with positional args."""
    result = monty_run('c.add(10)', inputs={'c': Calculator(value=5)}, dataclass_registry=[Calculator])
    assert result == snapshot(15)


def test_method_accessing_fields(monty_run: RunMonty):
    """Method that reads multiple fields from self."""
    result = monty_run('p.distance()', inputs={'p': Point2D(x=3.0, y=4.0)}, dataclass_registry=[Point2D])
    assert result == snapshot(5.0)


def test_method_returning_dataclass(monty_run: RunMonty):
    """Method that returns a new dataclass instance."""
    result = monty_run('p.translate(1.0, 2.0)', inputs={'p': Point2D(x=3.0, y=4.0)}, dataclass_registry=[Point2D])
    assert isinstance(result, Point2D)
    assert result.x == snapshot(4.0)
    assert result.y == snapshot(6.0)


def test_method_on_frozen_dataclass(monty_run: RunMonty):
    """Methods work on frozen dataclasses too."""

    @dataclass(frozen=True)
    class FrozenCalc:
        value: int

        def doubled(self) -> int:
            return self.value * 2

    result = monty_run('c.doubled()', inputs={'c': FrozenCalc(value=21)}, dataclass_registry=[FrozenCalc])
    assert result == snapshot(42)


def test_method_with_kwargs(monty_run: RunMonty):
    """Method called with keyword arguments."""

    @dataclass
    class Formatter:
        base: str

        def format(self, prefix: str = '', suffix: str = '') -> str:
            return prefix + self.base + suffix

    result = monty_run(
        "f.format(prefix='[', suffix=']')",
        inputs={'f': Formatter(base='hello')},
        dataclass_registry=[Formatter],
    )
    assert result == snapshot('[hello]')


def test_method_multiple_calls(monty_run: RunMonty):
    """Multiple method calls in the same expression."""
    result = monty_run(
        'c.add(10) + c.multiply(3)',
        inputs={'c': Calculator(value=5)},
        dataclass_registry=[Calculator],
    )
    assert result == snapshot(30)


def test_method_nonexistent_raises(monty_run: RunMonty):
    """Calling a non-existent method raises AttributeError."""
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('g.nonexistent()', inputs={'g': Greeter(greeting='hi')}, dataclass_registry=[Greeter])
    assert str(exc_info.value) == snapshot("AttributeError: 'Greeter' object has no attribute 'nonexistent'")


def test_method_on_nested_dataclass_in_list(monty_run: RunMonty):
    """Method call on a dataclass nested inside a list input."""
    result = monty_run('items[0].greet()', inputs={'items': [Greeter(greeting='nested')]}, dataclass_registry=[Greeter])
    assert result == snapshot('nested')


def test_method_on_nested_dataclass_in_dict(monty_run: RunMonty):
    """Method call on a dataclass nested inside a dict input."""
    result = monty_run(
        'd["g"].greet()', inputs={'d': {'g': Greeter(greeting='from dict')}}, dataclass_registry=[Greeter]
    )
    assert result == snapshot('from dict')


def test_method_on_nested_dataclass_in_tuple(monty_run: RunMonty):
    """Method call on a dataclass nested inside a tuple input."""
    result = monty_run('t[1].add(10)', inputs={'t': (0, Calculator(value=5))}, dataclass_registry=[Calculator])
    assert result == snapshot(15)


def test_dataclass_private_fields_skipped(monty_run: RunMonty):
    """Private fields (starting with _) are excluded from conversion."""

    @dataclass
    class WithPrivate:
        name: str
        _internal: int = 0

    result = monty_run('repr(x)', inputs={'x': WithPrivate(name='Alice', _internal=42)})
    assert result == snapshot("WithPrivate(name='Alice')")


def test_dataclass_private_fields_skipped_no_default(monty_run: RunMonty):
    """Private fields without defaults cause TypeError on reconstruction (field is missing)."""

    @dataclass
    class WithPrivateNoDefault:
        name: str
        _secret: str

    with pytest.raises(TypeError):
        monty_run('x', inputs={'x': WithPrivateNoDefault(name='Alice', _secret='hidden')})


def test_dataclass_private_field_not_accessible_in_monty(monty_run: RunMonty):
    """Private fields are not accessible inside Monty expressions."""

    @dataclass
    class WithPrivate:
        name: str
        _internal: int = 0

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('x._internal', inputs={'x': WithPrivate(name='Alice', _internal=42)})
    assert isinstance(exc_info.value.exception(), AttributeError)


def test_method_on_nested_dataclass_field(monty_run: RunMonty):
    """Method call on a dataclass that is a field of another dataclass (d.c.method())."""

    @dataclass
    class Inner:
        value: int

        def doubled(self) -> int:
            return self.value * 2

    @dataclass
    class Outer:
        inner: Inner

    result = monty_run(
        'o.inner.doubled()', inputs={'o': Outer(inner=Inner(value=21))}, dataclass_registry=[Outer, Inner]
    )
    assert result == snapshot(42)
