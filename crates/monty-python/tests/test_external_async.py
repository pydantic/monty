"""Tests for the async external-function surface of the Python bindings."""

import asyncio
import inspect
import sys
import time
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


async def run_async(code: str, *, stream_async_calls: bool = False, **kwargs: Any) -> Any:
    """Runs one snippet in a fresh async pool/session and returns its result."""
    async with pydantic_monty.AsyncMonty(stream_async_calls=stream_async_calls) as pool:
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


# How long `fetch` blocks the host. A suspended call cannot return sooner, so only a
# streamed call (which never waits for the host) can pass the sandbox's check below.
HOST_BLOCK_SECONDS = 1.0

# times the call from inside the sandbox, then prints: calls and prints reach the host in sandbox order
TIMED_CALL = f"""
import time
t0 = time.monotonic()
a = fetch(1)
elapsed = time.monotonic() - t0
print('after the call')
[await a, elapsed < {HOST_BLOCK_SECONDS}]
"""


@pytest.mark.skipif(sys.version_info < (3, 12), reason='inspect.markcoroutinefunction needs Python 3.12')
@pytest.mark.parametrize(
    'stream_async_calls,returned_at_once',
    [
        # the call streams, so the sandbox runs on while the host blocks in it
        (True, True),
        # the default: the call suspends until the host has answered
        (False, False),
    ],
)
async def test_coroutine_function_calls_stream_only_when_enabled(stream_async_calls: bool, returned_at_once: bool):
    """A call not awaited at once streams to the host, which starts it while the sandbox runs."""
    log: list[str] = []

    def fetch(x: int) -> Any:
        log.append(f'fetch({x})')
        time.sleep(HOST_BLOCK_SECONDS)
        return asyncio.sleep(0, x)

    def on_print(_stream: str, text: str) -> None:
        log.append(text)

    # a sync function returning a coroutine, marked so `iscoroutinefunction` sees it
    fetch = inspect.markcoroutinefunction(fetch)
    result = await run_async(
        TIMED_CALL, stream_async_calls=stream_async_calls, external_lookup={'fetch': fetch}, print_callback=on_print
    )
    assert result == [1, returned_at_once]
    # either way the host started the call before it saw the print that followed it
    assert log == ['fetch(1)', 'after the call\n']


@pytest.mark.skipif(sys.version_info < (3, 12), reason='inspect.markcoroutinefunction needs Python 3.12')
async def test_failed_print_stops_later_streamed_calls():
    """A call made after a print that failed never starts on the host."""
    log: list[str] = []

    def effect() -> Any:
        # logs when called, not when its coroutine is awaited
        log.append('effect')
        return asyncio.sleep(0)

    effect = inspect.markcoroutinefunction(effect)

    def on_print(_stream: str, _text: str) -> None:
        raise ValueError('print refused')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await run_async(
            "print('x')\na = effect()\nawait a",
            stream_async_calls=True,
            external_lookup={'effect': effect},
            print_callback=on_print,
        )
    assert str(exc_info.value) == snapshot('ValueError: print refused')
    assert log == []


async def test_feed_refused_while_another_is_suspended_leaves_that_feed_its_calls():
    """A second `feed_run` refused mid-suspension cannot take the running feed's streamed calls."""
    gate = asyncio.Event()
    started = asyncio.Event()

    async def fetch(x: int) -> int:
        started.set()
        await gate.wait()
        return x

    code = 'a = fetch(1)\nx = await a\nb = fetch(2)\nx + await b'
    async with pydantic_monty.AsyncMonty(stream_async_calls=True) as pool:
        async with pool.checkout() as session:
            first = asyncio.ensure_future(session.feed_run(code, external_lookup={'fetch': fetch}))
            await started.wait()
            with pytest.raises(RuntimeError) as exc_info:
                await session.feed_run('1', external_lookup={'fetch': fetch})
            assert str(exc_info.value) == snapshot(
                'monty worker protocol error: feed called while a suspension is awaiting an answer'
            )
            gate.set()
            assert await first == snapshot(3)


async def test_coroutine_function_error_raises_at_the_await():
    """The call returns at once; the host's exception is raised where it is awaited."""

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
    result = await run_async(code, stream_async_calls=True, external_lookup={'fail': fail})
    assert result == snapshot('called: intentional error')


async def test_coroutine_function_in_an_external_module():
    """Coroutine functions on an `ExternalModule` start without suspending too."""

    async def fetch(x: int) -> int:
        return x + 1

    async with pydantic_monty.AsyncMonty(stream_async_calls=True) as pool:
        async with pool.checkout(
            external_modules={'tools': pydantic_monty.ExternalModule({'fetch': fetch})}
        ) as session:
            code = 'import asyncio\nimport tools\nawait asyncio.gather(tools.fetch(1), tools.fetch(2))'
            assert await session.feed_run(code) == snapshot([2, 3])


async def test_streamed_calls_count_against_max_suspensions():
    """Each started call spends one suspension, as the call it replaces did."""

    async def fetch(x: int) -> int:
        return x

    async with pydantic_monty.AsyncMonty(stream_async_calls=True) as pool:
        async with pool.checkout(limits={'max_suspensions': 2}) as session:
            code = 'import asyncio\nawait asyncio.gather(*[fetch(i) for i in range(3)])'
            with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
                await session.feed_run(code, external_lookup={'fetch': fetch})
            assert str(exc_info.value) == snapshot('RuntimeError: suspension limit 2 exceeded')


def test_feed_start_suspends_at_coroutine_function_calls():
    """`feed_start` hands every call to the caller, so a coroutine function's call suspends as before."""

    async def fetch(x: int) -> int:
        return x

    with pydantic_monty.Monty() as pool:
        with pool.checkout() as session:
            progress = session.feed_start('a = fetch(1)\nawait a', external_lookup={'fetch': fetch})
            assert isinstance(progress, pydantic_monty.FunctionSnapshot)
            assert progress.function_name == snapshot('fetch')
