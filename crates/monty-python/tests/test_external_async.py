"""Tests for the async external-function surface of the Python bindings."""

import asyncio
from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty


@pytest.mark.parametrize('wrapped', [False, True])
async def test_coroutine_calls_keep_gather_concurrent(wrapped: bool):
    """Both host coroutines must start before either can finish, including sandbox wrappers."""
    ready = asyncio.Event()

    async def first() -> int:
        await ready.wait()
        return 1

    async def second() -> int:
        ready.set()
        return 2

    code = 'import asyncio\n'
    if wrapped:
        code += 'async def a():\n    return await first()\nasync def b():\n    return await second()\n'
        code += 'await asyncio.gather(a(), b())'
    else:
        code += 'await asyncio.gather(first(), second())'
    result = await asyncio.wait_for(run_async(code, external_lookup={'first': first, 'second': second}), 5)
    assert result == [1, 2]


async def run_async(code: str, **kwargs: Any) -> Any:
    """Runs one snippet in a fresh async pool/session and returns its result."""
    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            return await session.feed_run(code, **kwargs)


async def test_sequential_coroutines_use_one_suspension_per_call():
    """Two eager calls fit a two-suspension budget, including container results."""

    async def fetch() -> list[int]:
        return [21]

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout(limits={'max_suspensions': 2}) as session:
            result = await session.feed_run(
                'a = await fetch()\nb = await fetch()\na[0] + b[0]', external_lookup={'fetch': fetch}
            )
            assert result == 42


async def test_async_external_function_raises_surfaces_as_monty_runtime_error():
    """An uncaught exception from an awaited async callback surfaces as
    `MontyRuntimeError` with the original exception preserved in
    `exc.exception()`."""

    async def fail():
        raise ValueError('intentional error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await run_async('await fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('intentional error')


async def test_async_external_function_return_lone_surrogate_catchable_inside_monty():
    """An async callback returning a string with a lone surrogate surfaces inside Monty
    as a `ValueError` that can be caught, not as a raw `PyErr` escaping to the caller."""
    code = """
try:
    await get_str()
    result = 'no error'
except ValueError:
    result = 'caught'
result
"""

    async def get_str():
        return '\ud83d'

    assert await run_async(code, external_lookup={'get_str': get_str}) == snapshot('caught')


async def test_async_external_function_return_unconvertible_catchable_inside_monty():
    """An async callback returning an unconvertible object surfaces inside Monty as a
    `TypeError` that can be caught."""
    code = """
try:
    await get_thing()
    result = 'no error'
except TypeError:
    result = 'caught'
result
"""

    async def get_thing():
        return object()

    assert await run_async(code, external_lookup={'get_thing': get_thing}) == snapshot('caught')


async def test_async_external_lookup_name_conversion_error_discards_session():
    """As in the sync drive loop, a conversion failure while resolving a bare
    name discards the suspended worker rather than wedging it: the feed raises,
    and a follow-up feed on the same session fails fast instead of hanging."""
    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            with pytest.raises(pydantic_monty.MontyConversionError) as exc_info:
                await session.feed_run('x', external_lookup={'x': object()})
            assert str(exc_info.value) == snapshot(
                'Cannot convert builtins.object to Monty value — wrap class instances in pydantic_monty.ClassInstance(...)'
            )
            # the worker was discarded, so the session can no longer be fed
            with pytest.raises(RuntimeError) as exc_info2:
                await session.feed_run('1 + 1')
            assert str(exc_info2.value) == snapshot('this checkout has already been finished')


@pytest.mark.parametrize('wrapped', [False, True])
async def test_async_function_keeps_gather_concurrent(wrapped: bool):
    """Marked host coroutines start without suspending and still run concurrently."""
    ready = asyncio.Event()

    @pydantic_monty.async_function
    async def first() -> int:
        await ready.wait()
        return 1

    @pydantic_monty.async_function
    async def second() -> int:
        ready.set()
        return 2

    code = 'import asyncio\n'
    if wrapped:
        code += 'async def a():\n    return await first()\nasync def b():\n    return await second()\n'
        code += 'await asyncio.gather(a(), b())'
    else:
        code += 'await asyncio.gather(first(), second())'
    result = await asyncio.wait_for(run_async(code, external_lookup={'first': first, 'second': second}), 5)
    assert result == snapshot([1, 2])


async def test_async_function_raises_at_the_await():
    """The call returns at once; the host's exception is raised where it is awaited."""

    @pydantic_monty.async_function
    async def fail() -> None:
        raise ValueError('intentional error')

    code = """
a = fail()
r = 'called'
try:
    await a
except ValueError as e:
    r += ': ' + str(e)
r
"""
    assert await run_async(code, external_lookup={'fail': fail}) == snapshot('called: intentional error')


async def test_async_function_returning_a_value():
    """A marked function that returns a plain value settles its future at once."""

    @pydantic_monty.async_function
    def double(x: int) -> int:
        return x * 2

    code = 'import asyncio\nawait asyncio.gather(double(1), double(2))'
    assert await run_async(code, external_lookup={'double': double}) == snapshot([2, 4])


def test_async_function_returning_a_value_on_sync_monty():
    """The synchronous pool runs marked functions that return values."""

    @pydantic_monty.async_function
    def double(x: int) -> int:
        return x * 2

    with pydantic_monty.Monty() as pool:
        with pool.checkout() as session:
            code = 'a = double(1)\nb = double(2)\n[await a, await b]'
            assert session.feed_run(code, external_lookup={'double': double}) == snapshot([2, 4])


async def test_async_function_in_an_external_module():
    """Marked callables on an `ExternalModule` start without suspending too."""

    @pydantic_monty.async_function
    async def fetch(x: int) -> int:
        return x + 1

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout(
            external_modules={'tools': pydantic_monty.ExternalModule({'fetch': fetch})}
        ) as session:
            code = 'import asyncio\nimport tools\nawait asyncio.gather(tools.fetch(1), tools.fetch(2))'
            assert await session.feed_run(code) == snapshot([2, 3])


async def test_async_function_counts_against_max_suspensions():
    """Each started call spends one suspension, as the call it replaces did."""

    @pydantic_monty.async_function
    async def fetch(x: int) -> int:
        return x

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout(limits={'max_suspensions': 2}) as session:
            code = 'import asyncio\nawait asyncio.gather(*[fetch(i) for i in range(3)])'
            with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
                await session.feed_run(code, external_lookup={'fetch': fetch})
            assert str(exc_info.value) == snapshot('RuntimeError: suspension limit 2 exceeded')


def test_async_function_rejected_by_feed_start():
    """Snapshots cannot hand a started call to the caller yet, so they refuse one."""

    @pydantic_monty.async_function
    def double(x: int) -> int:
        return x * 2

    with pydantic_monty.Monty() as pool:
        with pool.checkout() as session:
            with pytest.raises(RuntimeError) as exc_info:
                session.feed_start('await double(1)', external_lookup={'double': double})
            assert exc_info.value.args[0] == snapshot(
                'functions marked with pydantic_monty.async_function are not supported by feed_start(); use feed_run()'
            )


def test_async_function_keeps_the_function_metadata():
    def fetch(x: int) -> int:
        """Fetches."""
        return x

    marked = pydantic_monty.async_function(fetch)
    assert (marked.__name__, marked.__doc__, marked(3)) == snapshot(('fetch', 'Fetches.', 3))


def test_async_function_requires_a_callable():
    with pytest.raises(TypeError) as exc_info:
        pydantic_monty.async_function(1)  # type: ignore[arg-type]
    assert exc_info.value.args[0] == snapshot('async_function() argument must be callable, not int')
