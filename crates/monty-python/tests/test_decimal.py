from __future__ import annotations

import decimal
from decimal import Decimal, DecimalTuple
from typing import Any

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

import pydantic_monty


@pytest.mark.parametrize(
    'value',
    [
        Decimal('1.20'),
        Decimal('-0'),
        Decimal('1E+5'),
        Decimal('-1.2345678901234567890123456789E-30'),
        Decimal('Infinity'),
        Decimal('-Infinity'),
    ],
)
def test_decimal_round_trip(monty_run: RunMonty, value: Decimal):
    result = monty_run('x', inputs={'x': value})
    assert type(result) is Decimal
    # `==` ignores trailing zeros, so the exact digits are compared too.
    assert result.as_tuple() == value.as_tuple()


def test_decimal_nan_round_trip(monty_run: RunMonty):
    result = monty_run('[x, y]', inputs={'x': Decimal('-NaN12'), 'y': Decimal('sNaN')})
    assert [type(d) for d in result] == [Decimal, Decimal]
    assert [d.as_tuple() for d in result] == snapshot(
        [DecimalTuple(sign=1, digits=(1, 2), exponent='n'), DecimalTuple(sign=0, digits=(), exponent='N')]
    )


def test_decimal_output(monty_run: RunMonty):
    code = """
from decimal import Decimal
[Decimal('19.99'), Decimal('0.10'), Decimal(10) ** 30]
"""
    result = monty_run(code)
    assert [type(d) for d in result] == [Decimal, Decimal, Decimal]
    # `==` ignores trailing zeros, so the exact digits are compared too.
    assert [str(d) for d in result] == snapshot(['19.99', '0.10', '1.000000000000000000000000000E+30'])


def test_decimal_input_used_in_sandbox(monty_run: RunMonty):
    result = monty_run('(str(price), price > 1, type(price).__name__)', inputs={'price': Decimal('19.99')})
    assert result == snapshot(('19.99', True, 'Decimal'))


def test_decimal_nested_in_containers(monty_run: RunMonty):
    value = {'amounts': [Decimal('1.10'), (Decimal('2'), Decimal('-0.5'))]}
    assert monty_run('x', inputs={'x': value}) == snapshot(
        {'amounts': [Decimal('1.10'), (Decimal('2'), Decimal('-0.5'))]}
    )


def test_decimal_subclass_input(monty_run: RunMonty):
    class Money(Decimal):
        pass

    result = monty_run('x', inputs={'x': Money('3.50')})
    assert type(result) is Decimal
    assert str(result) == snapshot('3.50')


def test_decimal_type_object(monty_run: RunMonty):
    assert monty_run('import decimal\ndecimal.Decimal') is Decimal
    assert monty_run('x', inputs={'x': Decimal}) is Decimal


def test_decimal_external_function(monty_run: RunMonty):
    def price(*args: Any, **kwargs: Any) -> Decimal:
        return Decimal('4.25')

    assert monty_run('str(price())', external_lookup={'price': price}) == snapshot('4.25')


def test_decimal_signal_reaches_host(monty_run: RunMonty):
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run("import decimal\ndecimal.Decimal('abc')")
    inner = exc_info.value.exception()
    assert type(inner) is decimal.InvalidOperation
    assert inner.args[0] == snapshot("[<class 'decimal.ConversionSyntax'>]")


@pytest.mark.parametrize(
    'exc_class',
    [decimal.InvalidOperation, decimal.DivisionByZero, decimal.FloatOperation, decimal.Overflow],
)
def test_decimal_exception_from_host(monty_run: RunMonty, exc_class: type[Exception]):
    def fail(*args: Any, **kwargs: Any) -> None:
        raise exc_class('host error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert type(inner) is exc_class
    assert inner.args[0] == snapshot('host error')


def test_decimal_exception_caught_in_sandbox(monty_run: RunMonty):
    """A host `FloatOperation` is still a `TypeError`, and `DivisionByZero` a `ZeroDivisionError`."""

    def fail_float(*args: Any, **kwargs: Any) -> None:
        raise decimal.FloatOperation('mixed')

    def fail_zero(*args: Any, **kwargs: Any) -> None:
        raise decimal.DivisionByZero('zero')

    code = """
caught = []
try:
    fail_float()
except TypeError:
    caught.append('TypeError')
try:
    fail_zero()
except ZeroDivisionError:
    caught.append('ZeroDivisionError')
caught
"""
    result = monty_run(code, external_lookup={'fail_float': fail_float, 'fail_zero': fail_zero})
    assert result == snapshot(['TypeError', 'ZeroDivisionError'])
