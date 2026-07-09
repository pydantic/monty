from __future__ import annotations

from decimal import Decimal

from conftest import RunMonty
from inline_snapshot import snapshot


def test_decimal_input_roundtrip(monty_run: RunMonty):
    # A host Decimal crosses into the sandbox and back losslessly.
    result = monty_run('x', inputs={'x': Decimal('1.20')})
    assert isinstance(result, Decimal)
    assert (type(result).__name__, repr(result)) == snapshot(('Decimal', "Decimal('1.20')"))


def test_decimal_exponent_roundtrip(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': Decimal('1E+5')})
    assert (type(result).__name__, repr(result)) == snapshot(('Decimal', "Decimal('1E+5')"))


def test_decimal_special_roundtrip(monty_run: RunMonty):
    result = monty_run('x', inputs={'x': Decimal('-Infinity')})
    assert repr(result) == snapshot("Decimal('-Infinity')")


def test_decimal_output(monty_run: RunMonty):
    # A Decimal created in the sandbox is returned as a host Decimal.
    result = monty_run("from decimal import Decimal\nDecimal('1.23') + Decimal('4.56')")
    assert isinstance(result, Decimal)
    assert (type(result).__name__, str(result)) == snapshot(('Decimal', '5.79'))


def test_decimal_division_prec(monty_run: RunMonty):
    result = monty_run('from decimal import Decimal\nDecimal(1) / Decimal(3)')
    assert str(result) == snapshot('0.3333333333333333333333333333')


def test_decimal_input_used_in_arithmetic(monty_run: RunMonty):
    # A host Decimal input is used in sandbox arithmetic and returned.
    result = monty_run('x * 2 + 1', inputs={'x': Decimal('1.5')})
    assert str(result) == snapshot('4.0')


def test_decimal_in_container(monty_run: RunMonty):
    result = monty_run('[x, x + 1]', inputs={'x': Decimal('2.5')})
    assert [str(d) for d in result] == snapshot(['2.5', '3.5'])


def test_decimal_snan_payload_roundtrip(monty_run: RunMonty):
    # Signaling NaNs and payload NaNs cross the boundary losslessly.
    result = monty_run('x', inputs={'x': Decimal('-sNaN123')})
    assert repr(result) == snapshot("Decimal('-sNaN123')")


def test_decimal_class_roundtrip(monty_run: RunMonty):
    # Returning the Decimal *class* maps to the host decimal.Decimal type.
    result = monty_run('from decimal import Decimal\nDecimal')
    assert result is Decimal
