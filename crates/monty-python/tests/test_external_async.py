"""Tests for the async external-function surface of the Python bindings."""

import asyncio
from typing import Any

import anyio
import pytest
from inline_snapshot import snapshot

import pydantic_monty


async def run_async(code: str, **kwargs: Any) -> Any:
    """Runs one snippet in a fresh async pool/session and returns its result."""
    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            return await session.feed_run(code, **kwargs)


@pytest.mark.parametrize('exit_mode', ['complete', 'error', 'cancel'])
@pytest.mark.parametrize('cleanup_raises', [False, True])
async def test_async_run_joins_unfinished_callbacks(exit_mode: str, cleanup_raises: bool):
    """Every run exit waits for the async cleanup of its unfinished callbacks."""
    started = asyncio.Event()
    cleaned_up = asyncio.Event()
    callback_tasks: list[asyncio.Task[Any]] = []

    async def background():
        task = asyncio.current_task()
        assert task is not None
        callback_tasks.append(task)
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned_up.set()
            if cleanup_raises:
                raise RuntimeError('callback cleanup failed')

    async def wait_until_started():
        await started.wait()

    code = 'background()\nawait wait_until_started()\n42'
    if exit_mode == 'error':
        code += '\nraise ValueError("sandbox failed")'
    elif exit_mode == 'cancel':
        code = 'await background()'
    unrelated = asyncio.create_task(asyncio.Event().wait())
    driver = asyncio.create_task(
        run_async(code, external_lookup={'background': background, 'wait_until_started': wait_until_started})
    )
    try:
        await asyncio.wait_for(started.wait(), timeout=5)
        if exit_mode == 'cancel':
            driver.cancel()
            with pytest.raises(asyncio.CancelledError):
                await driver
        elif exit_mode == 'error':
            with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
                await driver
            assert str(exc_info.value.exception()) == snapshot('sandbox failed')
        else:
            assert await driver == snapshot(42)
        assert [task.done() for task in callback_tasks] == snapshot([True])
        assert cleaned_up.is_set() == snapshot(True)
        assert unrelated.done() == snapshot(False)
    finally:
        driver.cancel()
        unrelated.cancel()
        for task in callback_tasks:
            task.cancel()
        await asyncio.gather(driver, unrelated, *callback_tasks, return_exceptions=True)


async def test_async_run_repeated_cancellation_during_callback_cleanup():
    cleanup_started = asyncio.Event()
    release_cleanup = asyncio.Event()
    cleaned_up = asyncio.Event()
    started = asyncio.Event()

    async def background():
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            cleanup_started.set()
            await release_cleanup.wait()
            cleaned_up.set()

    async def wait_until_started():
        await started.wait()

    driver = asyncio.create_task(
        run_async(
            'background()\nawait wait_until_started()',
            external_lookup={'background': background, 'wait_until_started': wait_until_started},
        )
    )
    try:
        await asyncio.wait_for(cleanup_started.wait(), timeout=5)
        for _ in range(3):
            driver.cancel()
            await asyncio.sleep(0)
            assert driver.done() == snapshot(False)
            assert cleaned_up.is_set() == snapshot(False)
        release_cleanup.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(driver, timeout=5)
        assert cleaned_up.is_set() == snapshot(True)
    finally:
        release_cleanup.set()
        driver.cancel()
        await asyncio.gather(driver, return_exceptions=True)


async def test_async_run_anyio_cancellation_joins_callback():
    started = asyncio.Event()
    cleaned_up = asyncio.Event()
    scopes: list[anyio.CancelScope] = []

    async def background():
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned_up.set()

    async def run():
        with anyio.CancelScope() as scope:
            scopes.append(scope)
            await run_async('await background()', external_lookup={'background': background})

    driver = asyncio.create_task(run())
    try:
        await asyncio.wait_for(started.wait(), timeout=5)
        scopes[0].cancel()
        await asyncio.wait_for(driver, timeout=5)
        assert cleaned_up.is_set() == snapshot(True)
    finally:
        driver.cancel()
        await asyncio.gather(driver, return_exceptions=True)


async def test_async_run_cancelled_before_start_leaves_session_healthy():
    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            feed = asyncio.ensure_future(session.feed_run('x = 1'))
            feed.cancel()
            with pytest.raises(asyncio.CancelledError):
                await feed
            with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
                await session.feed_run('x')
            assert str(exc_info.value.exception()) == snapshot("name 'x' is not defined")
            assert await session.feed_run('1 + 1') == snapshot(2)


async def test_async_run_callback_cleanup_preserves_session():
    started = asyncio.Event()
    cleaned_up = asyncio.Event()

    async def background():
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned_up.set()

    async def wait_until_started():
        await started.wait()

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_run(
                'background()\nawait wait_until_started()\nx = 42',
                external_lookup={'background': background, 'wait_until_started': wait_until_started},
            )
            assert cleaned_up.is_set() == snapshot(True)
            assert await session.feed_run('x') == snapshot(42)


async def test_async_run_does_not_own_tasks_created_by_callbacks():
    child_tasks: list[asyncio.Task[bool]] = []

    async def launch():
        child_tasks.append(asyncio.create_task(asyncio.Event().wait()))
        return 42

    try:
        assert await run_async('await launch()', external_lookup={'launch': launch}) == snapshot(42)
        assert [task.done() for task in child_tasks] == snapshot([False])
    finally:
        for task in child_tasks:
            task.cancel()
        await asyncio.gather(*child_tasks, return_exceptions=True)


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
