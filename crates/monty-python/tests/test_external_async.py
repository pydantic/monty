"""Tests for the async external-function surface of the Python bindings."""

import pytest
from inline_snapshot import snapshot

import pydantic_monty


async def test_async_external_function_raises_surfaces_as_monty_runtime_error():
    """An uncaught exception from an awaited async callback surfaces as
    ``MontyRuntimeError`` with the original exception preserved in
    ``exc.exception()``."""
    m = pydantic_monty.Monty('await fail()')

    async def fail():
        raise ValueError('intentional error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await m.run_async(external_functions={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('intentional error')


async def test_async_external_function_return_lone_surrogate_catchable_inside_monty():
    """An async callback returning a string with a lone surrogate surfaces inside Monty
    as a ``ValueError`` that can be caught, not as a raw ``PyErr`` escaping to the caller."""
    code = """
try:
    await get_str()
    result = 'no error'
except ValueError:
    result = 'caught'
result
"""
    m = pydantic_monty.Monty(code)

    async def get_str():
        return '\ud83d'

    assert await m.run_async(external_functions={'get_str': get_str}) == snapshot('caught')


async def test_async_external_function_return_unconvertible_catchable_inside_monty():
    """An async callback returning an unconvertible object surfaces inside Monty as a
    ``TypeError`` that can be caught."""
    code = """
try:
    await get_thing()
    result = 'no error'
except TypeError:
    result = 'caught'
result
"""
    m = pydantic_monty.Monty(code)

    async def get_thing():
        return object()

    assert await m.run_async(external_functions={'get_thing': get_thing}) == snapshot('caught')
