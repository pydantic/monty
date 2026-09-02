"""Tests for `pydantic_monty.ClassInstance` wrappers and `MontyClassProxy` proxies."""

from __future__ import annotations

import re
from dataclasses import FrozenInstanceError, dataclass
from functools import cached_property
from typing import Any, NoReturn
from uuid import uuid4

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import (
    AsyncMonty,
    ClassInstance,
    Monty,
    MontyClassProxy,
    MontyConversionError,
    MontySession,
)


@dataclass
class Person:
    name: str
    age: int

    def greeting(self) -> str:
        return f'hi {self.name}'


@dataclass(frozen=True)
class FrozenPoint:
    x: int
    y: int


@dataclass
class Calculator:
    value: int

    def add(self, n: int) -> int:
        return self.value + n

    def scale(self, *, factor: int = 2) -> int:
        return self.value * factor

    def boom(self) -> NoReturn:
        raise ValueError('nope')

    def _secret(self) -> int:
        return -1


@dataclass
class Wallet:
    balance: int

    def pay(self, amount: int) -> 'Wallet':
        return Wallet(balance=self.balance - amount)


class Greeter:
    """Plain (non-dataclass) class with public attrs, a method, and a private attr."""

    def __init__(self, greeting: str) -> None:
        self.greeting = greeting
        self._hidden = 'secret'

    def greet(self, name: str) -> str:
        return f'{self.greeting} {name}'


# === Identity round-trip ===


def test_identity_round_trip(monty_run: RunMonty):
    """Returning a host-sent instance gives the host the ORIGINAL object back."""
    p = Person(name='Alice', age=30)
    result = monty_run('x', inputs={'x': ClassInstance(p, eager_attrs='all')})
    assert result is p


def test_same_instance_two_feeds(session: MontySession):
    p = Person(name='Alice', age=30)
    r1 = session.feed_run('x', inputs={'x': ClassInstance(p, eager_attrs='all')})
    assert r1 is p
    r2 = session.feed_run('y', inputs={'y': ClassInstance(p, eager_attrs='all')})
    assert r2 is p


def test_sandbox_mutation_does_not_affect_host(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    result = monty_run('x.age = 31\nx', inputs={'x': ClassInstance(p, eager_attrs='all')})
    assert result is p
    assert p.age == snapshot(30)


# === Eager attrs ===


def test_eager_attrs_all(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    assert monty_run('x.name', inputs={'x': ClassInstance(p, eager_attrs='all')}) == snapshot('Alice')
    assert monty_run('x.age + 1', inputs={'x': ClassInstance(p, eager_attrs='all')}) == snapshot(31)


def test_eager_attrs_explicit_list(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    assert monty_run('x.name', inputs={'x': ClassInstance(p, eager_attrs=['name'])}) == snapshot('Alice')
    # `age` was not sent and there is no lazy policy, so the sandbox raises
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('x.age', inputs={'x': ClassInstance(p, eager_attrs=['name'])})
    assert str(exc_info.value) == snapshot("AttributeError: 'Person' object has no attribute 'age'")


def test_eager_attrs_none(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('x.name', inputs={'x': ClassInstance(p)})
    assert str(exc_info.value) == snapshot("AttributeError: 'Person' object has no attribute 'name'")


def test_repr_shows_eager_attrs(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    assert monty_run('repr(x)', inputs={'x': ClassInstance(p, eager_attrs='all')}) == snapshot(
        "Person(name='Alice', age=30)"
    )


def test_repr_includes_attr_set_in_sandbox(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    result = monty_run('x.extra = 5\nrepr(x)', inputs={'x': ClassInstance(p, eager_attrs='all')})
    assert result == snapshot("Person(name='Alice', age=30, extra=5)")


# === Lazy attrs ===


@pytest.mark.parametrize('lazy_attrs', [{'age'}, 'all'])
def test_lazy_attrs_allowed(monty_run: RunMonty, lazy_attrs: Any):
    p = Person(name='Alice', age=30)
    assert monty_run('x.age', inputs={'x': ClassInstance(p, lazy_attrs=lazy_attrs)}) == snapshot(30)


def test_lazy_attrs_denied(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('x.name', inputs={'x': ClassInstance(p, lazy_attrs={'age'})})
    assert str(exc_info.value) == snapshot("AttributeError: 'Person' object has no attribute 'name'")


def test_getattr_hasattr_consult_lazy_attrs(monty_run: RunMonty):
    """`getattr()` / `hasattr()` suspend to the host exactly like `x.attr`."""
    p = Person(name='Alice', age=30)
    inputs = {'x': ClassInstance(p, lazy_attrs={'age'})}
    served = monty_run("(hasattr(x, 'age'), getattr(x, 'age'), getattr(x, 'age', 0))", inputs=inputs)
    assert served == snapshot((True, 30, 30))
    denied = monty_run("(hasattr(x, 'name'), getattr(x, 'name', 'n/a'))", inputs=inputs)
    assert denied == snapshot((False, 'n/a'))
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run("getattr(x, 'name')", inputs=inputs)
    assert str(exc_info.value) == snapshot("AttributeError: 'Person' object has no attribute 'name'")


def test_private_attr_not_looked_up(monty_run: RunMonty):
    """Underscore-prefixed names never leave the sandbox, even with lazy_attrs='all'."""
    g = Greeter('hello')
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('g._hidden', inputs={'g': ClassInstance(g, lazy_attrs='all')})
    assert str(exc_info.value) == snapshot("AttributeError: 'Greeter' object has no attribute '_hidden'")


# === Method calls ===


@pytest.mark.parametrize('allowed_methods', [{'add'}, 'all'])
def test_method_call(monty_run: RunMonty, allowed_methods: Any):
    c = Calculator(value=5)
    assert monty_run('c.add(10)', inputs={'c': ClassInstance(c, allowed_methods=allowed_methods)}) == snapshot(15)


def test_method_call_kwargs(monty_run: RunMonty):
    c = Calculator(value=5)
    assert monty_run('c.scale(factor=3)', inputs={'c': ClassInstance(c, allowed_methods='all')}) == snapshot(15)


def test_method_denied(monty_run: RunMonty):
    c = Calculator(value=5)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('c.add(1)', inputs={'c': ClassInstance(c, allowed_methods={'scale'})})
    assert str(exc_info.value) == snapshot("AttributeError: 'Calculator' object has no attribute 'add'")


def test_method_none_allowed(monty_run: RunMonty):
    c = Calculator(value=5)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('c.add(1)', inputs={'c': ClassInstance(c)})
    assert str(exc_info.value) == snapshot("AttributeError: 'Calculator' object has no attribute 'add'")


def test_method_exception_propagates(monty_run: RunMonty):
    c = Calculator(value=5)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('c.boom()', inputs={'c': ClassInstance(c, allowed_methods='all')})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('nope')


def test_method_exception_catchable_in_sandbox(monty_run: RunMonty):
    c = Calculator(value=5)
    code = "try:\n    c.boom()\n    r = 'unexpected'\nexcept ValueError as e:\n    r = str(e)\nr"
    assert monty_run(code, inputs={'c': ClassInstance(c, allowed_methods='all')}) == snapshot('nope')


def test_private_method_not_dispatched(monty_run: RunMonty):
    c = Calculator(value=5)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('c._secret()', inputs={'c': ClassInstance(c, allowed_methods='all')})
    assert str(exc_info.value) == snapshot("AttributeError: 'Calculator' object has no attribute '_secret'")


def test_instance_call_method_dunder_call_rejected():
    """`__call__` routes only to `ClassType` construction; on an instance
    wrapper a (necessarily forged) `__call__` frame is denied even under
    `allowed_methods='all'`, so the wrapped instance can never be invoked."""

    class Invocable:
        def __call__(self) -> str:
            return 'invoked'

    wrapper = ClassInstance(Invocable(), allowed_methods='all')
    with pytest.raises(AttributeError) as exc_info:
        wrapper.call_method('__call__', (), {})
    assert str(exc_info.value) == snapshot("'Invocable' object has no attribute '__call__'")


# === convert_value / child wrapper ===


class UpperClassInstance(ClassInstance):
    """Wrapper that upper-cases every string crossing into the sandbox."""

    def convert_value(self, /, name: str, value: Any) -> Any:
        if isinstance(value, str):
            return value.upper()
        return super().convert_value(name, value)


def test_convert_value_override_method_return(monty_run: RunMonty):
    g = Greeter('hello')
    result = monty_run('g.greet("sam")', inputs={'g': UpperClassInstance(g, allowed_methods='all')})
    assert result == snapshot('HELLO SAM')


def test_convert_value_override_eager_attr(monty_run: RunMonty):
    g = Greeter('hello')
    assert monty_run('g.greeting', inputs={'g': UpperClassInstance(g, eager_attrs='all')}) == snapshot('HELLO')


def test_method_returning_dataclass_not_auto_wrapped(monty_run: RunMonty):
    """The default convert_value never wraps derived values: a returned bare
    dataclass fails conversion instead of inheriting this wrapper's policies."""
    w = Wallet(balance=100)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('w.pay(30)', inputs={'w': ClassInstance(w, eager_attrs='all', allowed_methods='all')})
    assert str(exc_info.value) == snapshot(
        'TypeError: Cannot convert test_class_instance.Wallet to Monty value — wrap class instances in pydantic_monty.ClassInstance(...)'
    )


class WalletClassInstance(ClassInstance):
    """Wrapper exposing derived wallets read-only via an explicit override."""

    def convert_value(self, /, name: str, value: Any) -> Any:
        if isinstance(value, Wallet):
            return WalletClassInstance(value, eager_attrs='all')
        return value


def test_method_returning_dataclass_wrapped_by_override(monty_run: RunMonty):
    """An explicit convert_value override chooses the derived value's policy —
    here read-only, so the child exposes attrs but no methods."""
    w = Wallet(balance=100)
    wrapper = WalletClassInstance(w, eager_attrs='all', allowed_methods='all')
    assert monty_run('w.pay(30).balance', inputs={'w': wrapper}) == snapshot(70)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('w.pay(30).pay(5)', inputs={'w': wrapper})
    assert str(exc_info.value) == snapshot("AttributeError: 'Wallet' object has no attribute 'pay'")


def test_returned_child_dataclass_is_host_object(monty_run: RunMonty):
    w = Wallet(balance=100)
    wrapper = WalletClassInstance(w, eager_attrs='all', allowed_methods='all')
    result = monty_run('w.pay(30)', inputs={'w': wrapper})
    assert isinstance(result, Wallet)
    assert result.balance == snapshot(70)


# === Plain (non-dataclass) class instances ===


def test_plain_class_eager_attrs(monty_run: RunMonty):
    """eager_attrs='all' on a plain class sends public `__dict__` entries."""
    g = Greeter('hello')
    assert monty_run('g.greeting', inputs={'g': ClassInstance(g, eager_attrs='all')}) == snapshot('hello')


def test_plain_class_method_call(monty_run: RunMonty):
    g = Greeter('hello')
    assert monty_run('g.greet("sam")', inputs={'g': ClassInstance(g, allowed_methods='all')}) == snapshot('hello sam')


def test_plain_class_identity_round_trip(monty_run: RunMonty):
    g = Greeter('hello')
    assert monty_run('g', inputs={'g': ClassInstance(g)}) is g


# === Bare instance rejection ===


def test_bare_dataclass_input_rejected(monty_run: RunMonty):
    with pytest.raises(MontyConversionError) as exc_info:
        monty_run('x', inputs={'x': Person(name='Alice', age=30)})
    assert str(exc_info.value) == snapshot(
        'Cannot convert test_class_instance.Person to Monty value — wrap class instances in pydantic_monty.ClassInstance(...)'
    )


def test_bare_class_instance_input_rejected(monty_run: RunMonty):
    with pytest.raises(MontyConversionError) as exc_info:
        monty_run('x', inputs={'x': Greeter('hi')})
    assert str(exc_info.value) == snapshot(
        'Cannot convert test_class_instance.Greeter to Monty value — wrap class instances in pydantic_monty.ClassInstance(...)'
    )


def test_bare_dataclass_from_external_function_rejected(monty_run: RunMonty):
    """A bare dataclass returned by an external function surfaces inside the
    sandbox as a catchable TypeError carrying the conversion message."""
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('f()', external_lookup={'f': lambda: Person(name='A', age=1)})
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)
    assert inner.args[0] == snapshot(
        'Cannot convert test_class_instance.Person to Monty value — wrap class instances in pydantic_monty.ClassInstance(...)'
    )


# === Frozen dataclasses cross as ordinary mutable copies ===


def test_frozen_dataclass_setattr_mutates_sandbox_copy(monty_run: RunMonty):
    """There is no frozen policy: in-sandbox setattr succeeds on the sandbox
    copy even for a frozen dataclass, and the host object is untouched."""
    p = FrozenPoint(x=1, y=2)
    result = monty_run('p.x = 5\np.x', inputs={'p': ClassInstance(p, eager_attrs='all')})
    assert result == snapshot(5)
    assert p.x == snapshot(1)


def test_frozen_instance_error_from_external_function(monty_run: RunMonty):
    """FrozenInstanceError raised by an external function is properly converted."""
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

    result = monty_run(code, external_lookup={'fail': fail})
    assert result == snapshot('frozen')


def test_frozen_instance_error_from_external_function_propagates(monty_run: RunMonty):
    def fail() -> NoReturn:
        raise FrozenInstanceError('test frozen error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, FrozenInstanceError)
    assert inner.args[0] == snapshot('test frozen error')


# === Equality and hashing in the sandbox ===


def test_equality_same_class_equal_attrs(monty_run: RunMonty):
    inputs = {
        'a': ClassInstance(FrozenPoint(x=1, y=2), eager_attrs='all'),
        'b': ClassInstance(FrozenPoint(x=1, y=2), eager_attrs='all'),
    }
    assert monty_run('a == b', inputs=inputs) is True


def test_equality_different_attrs(monty_run: RunMonty):
    inputs = {
        'a': ClassInstance(FrozenPoint(x=1, y=2), eager_attrs='all'),
        'b': ClassInstance(FrozenPoint(x=1, y=3), eager_attrs='all'),
    }
    assert monty_run('a == b', inputs=inputs) is False


def test_equality_different_classes(monty_run: RunMonty):
    inputs = {
        'a': ClassInstance(FrozenPoint(x=1, y=2), eager_attrs='all'),
        'b': ClassInstance(Person(name='x', age=1), eager_attrs='all'),
    }
    assert monty_run('a == b', inputs=inputs) is False


def test_frozen_dataclass_instances_unhashable(monty_run: RunMonty):
    """All host instances are unhashable (they define eq by attrs), frozen
    dataclasses included — matching CPython's eq-without-hash rule."""
    p = FrozenPoint(x=1, y=2)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('hash(x)', inputs={'x': ClassInstance(p, eager_attrs='all')})
    assert str(exc_info.value) == snapshot("TypeError: unhashable type: 'FrozenPoint'")


def test_mutable_instances_unhashable(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('{x}', inputs={'x': ClassInstance(p, eager_attrs='all')})
    # error messages name the real class, not the 'HostClass' placeholder
    assert str(exc_info.value) == snapshot(
        "TypeError: cannot use 'Person' as a set element (unhashable type: 'Person')"
    )


# === In-sandbox introspection of host-sent values ===


def test_is_dataclass_in_sandbox(monty_run: RunMonty):
    """is_dataclass reflects the flag sent by the host: True for wrapped dataclasses."""
    code = 'from dataclasses import is_dataclass\n(is_dataclass(p), is_dataclass(g))'
    inputs = {
        'p': ClassInstance(Person(name='Alice', age=30), eager_attrs='all'),
        'g': ClassInstance(Greeter('hi'), eager_attrs='all'),
    }
    assert monty_run(code, inputs=inputs) == snapshot((True, False))


def test_type_names_the_real_class(monty_run: RunMonty):
    p = Person(name='Alice', age=30)
    inputs = {'x': ClassInstance(p, eager_attrs='all')}
    assert monty_run('repr(type(x))', inputs=inputs) == snapshot("<class 'Person'>")
    assert monty_run('type(x).__name__', inputs=inputs) == snapshot('Person')
    # equal by class identity, though each type(x) call makes a fresh object
    assert monty_run('type(x) == type(x)', inputs=inputs) == snapshot(True)


# === Nesting in containers ===


def test_wrappers_nested_in_containers(monty_run: RunMonty):
    c = Calculator(value=5)
    w = ClassInstance(c, allowed_methods='all')
    assert monty_run('xs[0].add(1)', inputs={'xs': [w]}) == snapshot(6)
    assert monty_run('d["c"].add(2)', inputs={'d': {'c': w}}) == snapshot(7)
    assert monty_run('t[1].add(3)', inputs={'t': (0, w)}) == snapshot(8)


def test_nested_wrapper_identity_round_trip(monty_run: RunMonty):
    c = Calculator(value=5)
    result = monty_run('xs[0]', inputs={'xs': [ClassInstance(c)]})
    assert result is c


# === MontyClassProxy stand-ins for sandbox-defined classes ===


def test_sandbox_class_returns_proxy(monty_run: RunMonty):
    code = 'class Foo:\n    def __init__(self, a: int):\n        self.a = a\nFoo(1)'
    result = monty_run(code)
    assert isinstance(result, MontyClassProxy)
    assert result.name == snapshot('Foo')
    assert result.is_dataclass is False
    assert result.attributes == snapshot({'a': 1})
    assert repr(result) == snapshot("MontyClassProxy(name='Foo', attributes={'a': 1})")


def test_sandbox_dataclass_returns_proxy(monty_run: RunMonty):
    code = 'from dataclasses import dataclass\n@dataclass\nclass P:\n    x: int\n    y: int\nP(1, 2)'
    result = monty_run(code)
    assert isinstance(result, MontyClassProxy)
    assert result.name == snapshot('P')
    assert result.is_dataclass is True
    assert result.attributes == snapshot({'x': 1, 'y': 2})


def test_proxy_equality(monty_run: RunMonty):
    code = 'class Foo:\n    def __init__(self, a: int):\n        self.a = a\n[Foo(1), Foo(1), Foo(2)]'
    a, b, c = monty_run(code)
    assert a == b
    assert a != c


# === Async method calls ===


async def test_async_method_call_coroutine():
    """`call_method` returning a coroutine works with AsyncMonty like async externals."""

    class AsyncGreeter:
        async def greet(self, name: str) -> str:
            return f'hi {name}'

    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run(
                'await g.greet("sam")', inputs={'g': ClassInstance(AsyncGreeter(), allowed_methods='all')}
            )
    assert result == snapshot('hi sam')


async def test_async_method_result_passes_convert_value():
    """`convert_value` applies to the awaited result of an async method, not
    the coroutine object — a redaction hook must see the resolved value."""

    class AsyncGreeter:
        async def greet(self, name: str) -> str:
            return f'hi {name}'

    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run(
                'await g.greet("sam")', inputs={'g': UpperClassInstance(AsyncGreeter(), allowed_methods='all')}
            )
    assert result == snapshot('HI SAM')


# === Dump / load fallback ===


def test_dump_load_into_new_session_falls_back_to_proxy(pool: Monty):
    """After restoring into a new session the instance store is empty: returned
    instances become proxies, method calls fail, lazy lookups raise AttributeError."""
    p = Person(name='Alice', age=30)
    wrapper = ClassInstance(p, eager_attrs=['name'], lazy_attrs='all', allowed_methods='all')
    with pool.checkout() as session:
        session.feed_run('x = obj', inputs={'obj': wrapper})
        blob = session.dump()

    with pool.checkout() as session:
        assert session.load_session(blob) is None
        result = session.feed_run('x')
        assert isinstance(result, MontyClassProxy)
        assert result.name == snapshot('Person')
        assert result.attributes == snapshot({'name': 'Alice'})

    with pool.checkout() as session:
        assert session.load_session(blob) is None
        with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
            session.feed_run('x.greeting()')
        inner = exc_info.value.exception()
        assert isinstance(inner, RuntimeError)
        # the message embeds the (unstable) host id — normalize it before comparing
        assert re.sub(r'\(id [0-9a-f-]+\)', '(id ...)', str(inner)) == snapshot(
            "no host object registered for method call 'greeting' (id ...) — the instance store is empty after loading a dump into a fresh session"
        )

    with pool.checkout() as session:
        assert session.load_session(blob) is None
        with pytest.raises(pydantic_monty.MontyRuntimeError) as attr_exc_info:
            session.feed_run('x.age')
        assert str(attr_exc_info.value) == snapshot("AttributeError: 'Person' object has no attribute 'age'")


# === ClassType instantiation ===


def test_class_type_instantiation(monty_run: RunMonty):
    result = monty_run(
        'p = Person("Sam", 4)\np.greeting()',
        inputs={
            'Person': pydantic_monty.ClassType(
                Person, init=True, instance_eager_attrs='all', instance_allowed_methods='all'
            )
        },
    )
    assert result == snapshot('hi Sam')


def test_class_type_constructed_instance_round_trips(monty_run: RunMonty):
    result = monty_run(
        'Person("Ada", 36)',
        inputs={'Person': pydantic_monty.ClassType(Person, init=True, instance_eager_attrs='all')},
    )
    assert result == Person(name='Ada', age=36)


def test_class_type_init_false_raises_in_sandbox(monty_run: RunMonty):
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('Person("Sam", 4)', inputs={'Person': pydantic_monty.ClassType(Person)})
    assert str(exc_info.value) == snapshot("TypeError: cannot instantiate host class 'Person'")


def test_class_type_constructor_exception_propagates(monty_run: RunMonty):
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run(
            'Person("Sam")',
            inputs={'Person': pydantic_monty.ClassType(Person, init=True)},
        )
    assert str(exc_info.value) == snapshot("TypeError: Person.__init__() missing 1 required positional argument: 'age'")


def test_class_type_kwargs_and_instance_policy(monty_run: RunMonty):
    # kwargs reach the constructor; the constructed instance obeys the
    # wrapper's instance policy (allowed_methods here)
    result = monty_run(
        'c = Calculator(value=10)\nc.add(5)',
        inputs={'Calculator': pydantic_monty.ClassType(Calculator, init=True, instance_allowed_methods={'add'})},
    )
    assert result == snapshot(15)


def test_class_type_denied_method_on_constructed_instance(monty_run: RunMonty):
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run(
            'Calculator(value=10).boom()',
            inputs={'Calculator': pydantic_monty.ClassType(Calculator, init=True, instance_allowed_methods={'add'})},
        )
    assert str(exc_info.value) == snapshot("AttributeError: 'Calculator' object has no attribute 'boom'")


class Shape:
    """Plain class with a class constant, a classmethod, and a staticmethod."""

    SIDES = 4
    KIND = 'polygon'

    def __init__(self, size: int) -> None:
        self.size = size

    @classmethod
    def unit(cls) -> int:
        return cls.SIDES

    @staticmethod
    def double(n: int) -> int:
        return n * 2


def test_class_type_eager_class_attrs(monty_run: RunMonty):
    """eager_attrs on a ClassType sends class constants with the type."""
    wrapper = pydantic_monty.ClassType(Shape, eager_attrs='all')
    assert monty_run('Shape.SIDES + len(Shape.KIND)', inputs={'Shape': wrapper}) == snapshot(11)


def test_class_type_eager_all_skips_descriptors(monty_run: RunMonty):
    """eager_attrs='all' sends only plain class constants: non-callable
    descriptors like `functools.cached_property` are class machinery, not
    values, and must not be serialized."""

    class WithCached:
        KIND = 'cached'

        @cached_property
        def expensive(self) -> int:
            return 99

    wrapper = pydantic_monty.ClassType(WithCached, eager_attrs='all')
    assert monty_run('W.KIND', inputs={'W': wrapper}) == snapshot('cached')
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('W.expensive', inputs={'W': wrapper})
    assert str(exc_info.value) == snapshot("AttributeError: type object 'WithCached' has no attribute 'expensive'")


def test_class_type_lazy_class_attr(monty_run: RunMonty):
    """lazy_attrs on a ClassType serves class constants on demand."""
    wrapper = pydantic_monty.ClassType(Shape, lazy_attrs={'SIDES'})
    assert monty_run('Shape.SIDES', inputs={'Shape': wrapper}) == snapshot(4)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('Shape.KIND', inputs={'Shape': wrapper})
    assert str(exc_info.value) == snapshot("AttributeError: type object 'Shape' has no attribute 'KIND'")


def test_class_type_getattr_hasattr_lazy_class_attr(monty_run: RunMonty):
    wrapper = pydantic_monty.ClassType(Shape, lazy_attrs={'SIDES'})
    code = "(hasattr(Shape, 'SIDES'), getattr(Shape, 'SIDES'), hasattr(Shape, 'KIND'), getattr(Shape, 'KIND', None))"
    assert monty_run(code, inputs={'Shape': wrapper}) == snapshot((True, 4, False, None))
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run("getattr(Shape, 'KIND')", inputs={'Shape': wrapper})
    assert str(exc_info.value) == snapshot("AttributeError: type object 'Shape' has no attribute 'KIND'")


def test_class_type_classmethod_call(monty_run: RunMonty):
    wrapper = pydantic_monty.ClassType(Shape, allowed_methods={'unit', 'double'})
    assert monty_run('Shape.unit()', inputs={'Shape': wrapper}) == snapshot(4)
    assert monty_run('Shape.double(21)', inputs={'Shape': wrapper}) == snapshot(42)


def test_class_type_denied_classmethod(monty_run: RunMonty):
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('Shape.unit()', inputs={'Shape': pydantic_monty.ClassType(Shape)})
    assert str(exc_info.value) == snapshot("AttributeError: type object 'Shape' has no attribute 'unit'")


def test_type_of_instance_call_denied_without_init(monty_run: RunMonty):
    """Every ClassInstance materializes a default ClassType for its class, so
    calling type(x) is denied by that wrapper's init=False policy — not a
    store miss."""
    p = Person(name='Alice', age=30)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('type(x)("Bob", 2)', inputs={'x': ClassInstance(p)})
    assert str(exc_info.value) == snapshot("TypeError: cannot instantiate host class 'Person'")


def test_type_of_host_instance_round_trips_to_class(monty_run: RunMonty):
    # type(x) of a host instance crosses back and resolves to the real class
    p = Person(name='Alice', age=30)
    result = monty_run('type(x)', inputs={'x': ClassInstance(p)})
    assert result is Person


def test_instance_uuid_is_stable_across_feeds(session: MontySession):
    # the same host object keeps one identity for the whole session, so two
    # sends compare equal (each send still allocates its own sandbox proxy,
    # so `a is b` stays False — see limitations/classes.md)
    p = Person(name='Alice', age=30)
    wrapper = ClassInstance(p, eager_attrs='all')
    session.feed_run('a = x', inputs={'x': wrapper})
    result = session.feed_run('a == b', inputs={'b': wrapper})
    assert result is True


# === Wrapper identity ids ===


def test_duplicate_wrapper_id_rejected(monty_run: RunMonty):
    """Two wrappers sharing an id but wrapping different objects would alias
    routing (calls dispatch to whichever registered last) — rejected."""
    shared = uuid4()
    a = ClassInstance(Person(name='A', age=1), id=shared)
    b = ClassInstance(Person(name='B', age=2), id=shared)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('[x, y]', inputs={'x': a, 'y': b})
    assert (
        str(exc_info.value) == f'ValueError: wrapper id {shared} already identifies a different object in this session'
    )


def test_class_type_duplicate_id_rejected(monty_run: RunMonty):
    shared = uuid4()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run(
            '1',
            inputs={
                'A': pydantic_monty.ClassType(Person, id=shared),
                'B': pydantic_monty.ClassType(Greeter, id=shared),
            },
        )
    assert (
        str(exc_info.value) == f'ValueError: wrapper id {shared} already identifies a different object in this session'
    )


def test_unhashable_metaclass_class(monty_run: RunMonty):
    """Class identity in the store is `is`-based: a metaclass defining
    `__eq__` (which makes the class object unhashable) is never consulted."""

    class Meta(type):
        def __eq__(self, other: object) -> bool:
            return False

    class Odd(metaclass=Meta):
        def __init__(self) -> None:
            self.x = 5

    with pytest.raises(TypeError):
        hash(Odd)  # precondition: the metaclass made the class unhashable
    assert monty_run('o.x', inputs={'o': ClassInstance(Odd(), eager_attrs='all')}) == 5


def test_equal_comparing_metaclass_classes_stay_distinct(monty_run: RunMonty):
    """Two distinct classes whose metaclass `__eq__` says they are equal must
    not share an id: the store keys by wrapper uuid and compares by `is`."""

    class Meta(type):
        def __eq__(self, other: object) -> bool:
            return True

        def __hash__(self) -> int:
            return 0

    class A(metaclass=Meta):
        def __init__(self) -> None:
            self.x = 1

    class B(metaclass=Meta):
        def __init__(self) -> None:
            self.x = 1

    assert A == B  # precondition: the metaclass makes the classes compare equal
    inputs = {'a': ClassInstance(A(), eager_attrs='all'), 'b': ClassInstance(B(), eager_attrs='all')}
    assert monty_run('(type(a) == type(b), a == b, type(a).__name__, type(b).__name__)', inputs=inputs) == snapshot(
        (False, False, 'A', 'B')
    )


def test_same_qualname_distinct_classes(monty_run: RunMonty):
    """Class ids default from `module.qualname`, so two class objects sharing a
    name get one id; one session rejects the second rather than aliasing, and
    an explicit `id` on either wrapper separates them."""

    def make_class() -> type[Any]:
        class Shadow:
            def __init__(self) -> None:
                self.x = 1

        return Shadow

    first, second = make_class(), make_class()
    assert first is not second and first.__qualname__ == second.__qualname__
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run(
            '1',
            inputs={'a': ClassInstance(first(), eager_attrs='all'), 'b': ClassInstance(second(), eager_attrs='all')},
        )
    assert re.fullmatch(
        r'ValueError: wrapper id [0-9a-f-]{36} already identifies a different object in this session',
        str(exc_info.value),
    )
    inputs = {
        'a': ClassInstance(first(), eager_attrs='all'),
        'b': ClassInstance(second(), eager_attrs='all', class_type=pydantic_monty.ClassType(second, id=uuid4())),
    }
    assert monty_run('(type(a) == type(b), a == b)', inputs=inputs) == snapshot((False, False))
