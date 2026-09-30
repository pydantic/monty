"""Tests for the async external-function surface of the Python bindings."""

import asyncio
from typing import Any

import anyio
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


@pytest.mark.parametrize('exit_mode', ['complete', 'error', 'cancel', 'cancel_deferred'])
@pytest.mark.parametrize('cleanup_raises', [False, True])
async def test_async_run_joins_unfinished_callbacks(exit_mode: str, cleanup_raises: bool):
    """Leaving the session joins its unfinished callbacks on every run exit."""
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
    elif exit_mode == 'cancel_deferred':
        code = 'import asyncio\nawait asyncio.gather(background())'
    unrelated = asyncio.create_task(asyncio.Event().wait())
    driver = asyncio.create_task(
        run_async(code, external_lookup={'background': background, 'wait_until_started': wait_until_started})
    )
    try:
        await asyncio.wait_for(started.wait(), timeout=5)
        if exit_mode in {'cancel', 'cancel_deferred'}:
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


@pytest.mark.parametrize('cancel_before_cleanup', [False, True])
async def test_async_run_repeated_cancellation_during_callback_cleanup(cancel_before_cleanup: bool):
    cleanup_started = asyncio.Event()
    cleanup_cancelled = asyncio.Event()
    release_cleanup = asyncio.Event()
    cleaned_up = asyncio.Event()
    started = asyncio.Event()
    callback_tasks: list[asyncio.Task[Any]] = []

    async def background():
        task = asyncio.current_task()
        assert task is not None
        callback_tasks.append(task)
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            cleanup_started.set()
            try:
                await release_cleanup.wait()
            except asyncio.CancelledError:
                cleanup_cancelled.set()
                await release_cleanup.wait()
            cleaned_up.set()

    async def wait_until_started():
        await started.wait()

    code = 'await background()' if cancel_before_cleanup else 'background()\nawait wait_until_started()'
    driver = asyncio.create_task(
        run_async(code, external_lookup={'background': background, 'wait_until_started': wait_until_started})
    )
    try:
        await asyncio.wait_for(started.wait(), timeout=5)
        if cancel_before_cleanup:
            driver.cancel()
        await asyncio.wait_for(cleanup_started.wait(), timeout=5)
        driver.cancel()
        await asyncio.wait_for(cleanup_cancelled.wait(), timeout=5)
        assert driver.done() == snapshot(False)
        assert [task.done() for task in callback_tasks] == snapshot([False])
        release_cleanup.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(driver, timeout=5)
        assert cleaned_up.is_set() == snapshot(True)
        assert [task.done() for task in callback_tasks] == snapshot([True])
    finally:
        release_cleanup.set()
        driver.cancel()
        await asyncio.gather(driver, *callback_tasks, return_exceptions=True)


async def test_async_run_anyio_cancellation_joins_callback():
    started = asyncio.Event()
    cleanup_exited = asyncio.Event()
    callback_tasks: list[asyncio.Task[Any]] = []
    scopes: list[anyio.CancelScope] = []

    async def background():
        task = asyncio.current_task()
        assert task is not None
        callback_tasks.append(task)
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            try:
                await asyncio.sleep(0)
            finally:
                cleanup_exited.set()

    async def run():
        with anyio.CancelScope() as scope:
            scopes.append(scope)
            await run_async('await background()', external_lookup={'background': background})

    driver = asyncio.create_task(run())
    try:
        await asyncio.wait_for(started.wait(), timeout=5)
        scopes[0].cancel()
        await asyncio.wait_for(driver, timeout=5)
        assert cleanup_exited.is_set() == snapshot(True)
        assert [task.done() for task in callback_tasks] == snapshot([True])
    finally:
        driver.cancel()
        await asyncio.gather(driver, *callback_tasks, return_exceptions=True)


@pytest.mark.parametrize('exit_mode', ['complete', 'error'])
async def test_async_run_waits_for_slow_callback_cleanup(exit_mode: str):
    started = asyncio.Event()
    cleaned_up = asyncio.Event()

    async def background():
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(1.05)
            cleaned_up.set()

    async def wait_until_started():
        await started.wait()

    code = 'background()\nawait wait_until_started()\n42'
    if exit_mode == 'error':
        code += '\nraise ValueError("sandbox failed")'
    driver = asyncio.create_task(
        run_async(code, external_lookup={'background': background, 'wait_until_started': wait_until_started})
    )
    try:
        if exit_mode == 'error':
            with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
                await asyncio.wait_for(driver, timeout=5)
            assert str(exc_info.value.exception()) == snapshot('sandbox failed')
        else:
            assert await asyncio.wait_for(driver, timeout=5) == snapshot(42)
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


@pytest.mark.parametrize('manual', [False, True])
async def test_unfinished_callbacks_live_until_session_exit(manual: bool):
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
            code = 'background()\nawait wait_until_started()\nx = 42'
            external_lookup = {'background': background, 'wait_until_started': wait_until_started}
            if manual:
                progress = await session.feed_start(code, external_lookup=external_lookup)
                while not isinstance(progress, pydantic_monty.MontyComplete):
                    progress = await progress.resume_auto()
            else:
                await session.feed_run(code, external_lookup=external_lookup)
            assert cleaned_up.is_set() == snapshot(False)
            assert await session.feed_run('x') == snapshot(42)
        assert cleaned_up.is_set() == snapshot(True)


@pytest.mark.parametrize('manual_start,manual_finish', [(False, False), (False, True), (True, False), (True, True)])
async def test_external_future_survives_feeds(manual_start: bool, manual_finish: bool):
    ready = asyncio.Event()
    started = asyncio.Event()
    cleaned_up = asyncio.Event()

    async def fetch():
        try:
            started.set()
            await ready.wait()
            return 42
        finally:
            cleaned_up.set()

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            if manual_start:
                progress = await session.feed_start('f = fetch()', external_lookup={'fetch': fetch})
                while not isinstance(progress, pydantic_monty.MontyComplete):
                    progress = await progress.resume_auto()
            else:
                await session.feed_run('f = fetch()', external_lookup={'fetch': fetch})
            await asyncio.wait_for(started.wait(), timeout=5)
            assert cleaned_up.is_set() == snapshot(False)
            ready.set()
            if manual_finish:
                progress = await session.feed_start('await f')
                while not isinstance(progress, pydantic_monty.MontyComplete):
                    progress = await asyncio.wait_for(progress.resume_auto(), timeout=5)
                result = progress.output
            else:
                result = await asyncio.wait_for(session.feed_run('await f'), timeout=5)
            assert result == snapshot(42)
            assert await session.feed_run('await f') == snapshot(42)
            assert cleaned_up.is_set() == snapshot(True)


@pytest.mark.parametrize('manual', [False, True])
@pytest.mark.parametrize('callback_raises', [False, True])
async def test_manually_resolved_callback_does_not_settle_twice(manual: bool, callback_raises: bool):
    ready = asyncio.Event()
    started = asyncio.Event()
    finished = asyncio.Event()

    async def original():
        started.set()
        await ready.wait()
        finished.set()
        if callback_raises:
            raise ValueError('superseded result')
        return 11

    async def following():
        await asyncio.sleep(0)
        return 22

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_run('f = original()', external_lookup={'original': original})
            await asyncio.wait_for(started.wait(), timeout=5)
            progress = await session.feed_start('await f')
            assert isinstance(progress, pydantic_monty.AsyncFutureSnapshot)
            result = await progress.resume({progress.pending_call_ids[0]: {'return_value': 42}})
            assert isinstance(result, pydantic_monty.MontyComplete)
            assert result.output == snapshot(42)
            ready.set()
            await asyncio.wait_for(finished.wait(), timeout=5)
            code = 'g = following()\nawait g'
            if manual:
                progress = await session.feed_start(code, external_lookup={'following': following})
                while not isinstance(progress, pydantic_monty.MontyComplete):
                    progress = await asyncio.wait_for(progress.resume_auto(), timeout=5)
                output = progress.output
            else:
                output = await asyncio.wait_for(
                    session.feed_run(code, external_lookup={'following': following}), timeout=5
                )
            assert output == snapshot(22)
            assert await session.feed_run('await f') == snapshot(42)


@pytest.mark.parametrize('failed_feed', ['raise ValueError("feed failed")', 'await asyncio.gather(f, fail())'])
async def test_external_future_survives_failed_feed(failed_feed: str):
    ready = asyncio.Event()

    async def fetch():
        await ready.wait()
        return 42

    async def fail():
        raise ValueError('feed failed')

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_run('import asyncio\nf = fetch()', external_lookup={'fetch': fetch})
            with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
                await session.feed_run(failed_feed, external_lookup={'fail': fail})
            assert str(exc_info.value.exception()) == snapshot('feed failed')
            ready.set()
            assert await asyncio.wait_for(session.feed_run('await f'), timeout=5) == snapshot(42)


async def test_external_futures_from_multiple_feeds_do_not_collide():
    ready = asyncio.Event()

    async def first():
        await ready.wait()
        return 11

    async def second():
        ready.set()
        return 22

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_run('f = first()', external_lookup={'first': first})
            await session.feed_run('g = second()', external_lookup={'second': second})
            assert await asyncio.wait_for(session.feed_run('(await f, await g)'), timeout=5) == snapshot((11, 22))


async def test_system_sleep_future_survives_feeds():
    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_run('import asyncio\nf = asyncio.sleep(0.01)')
            assert await asyncio.wait_for(session.feed_run('await f'), timeout=5) == snapshot(None)


@pytest.mark.parametrize('manual', [False, True])
@pytest.mark.parametrize('resolve', [False, True])
async def test_os_callback_has_session_lifetime(manual: bool, resolve: bool):
    started = asyncio.Event()
    ready = asyncio.Event()
    cleaned_up = asyncio.Event()

    async def os_handler(**_: Any):
        try:
            started.set()
            await ready.wait()
        finally:
            await asyncio.sleep(0)
            cleaned_up.set()

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout(os_policy={'sleep': 'call_host'}) as session:
            code = 'import asyncio\nf = asyncio.sleep(1)'
            if manual:
                progress = await session.feed_start(code, os=os_handler)
                while not isinstance(progress, pydantic_monty.MontyComplete):
                    progress = await progress.resume_auto()
            else:
                await session.feed_run(code, os=os_handler)
            await asyncio.wait_for(started.wait(), timeout=5)
            assert cleaned_up.is_set() == snapshot(False)
            if resolve:
                ready.set()
                assert await asyncio.wait_for(session.feed_run('await f'), timeout=5) == snapshot(None)
        assert cleaned_up.is_set() == snapshot(True)


async def test_idle_dump_does_not_transfer_host_callbacks():
    ready = asyncio.Event()
    started = asyncio.Event()

    async def fetch():
        started.set()
        await ready.wait()
        return 42

    async with pydantic_monty.AsyncMonty() as pool:
        async with pool.checkout() as original:
            await original.feed_run('f = fetch()', external_lookup={'fetch': fetch})
            await asyncio.wait_for(started.wait(), timeout=5)
            blob = await original.dump()
            async with pool.checkout() as restored:
                await restored.load_session(blob)
                progress = await restored.feed_start('await f')
                assert isinstance(progress, pydantic_monty.AsyncFutureSnapshot)
                assert progress.pending_call_ids == snapshot([0])
                with pytest.raises(RuntimeError) as exc_info:
                    await progress.resume_auto()
                assert str(exc_info.value) == snapshot('No pending async tasks but ResolveFutures requested')
            async with pool.checkout() as restored:
                await restored.load_session(blob)
                progress = await restored.feed_start('await f')
                assert isinstance(progress, pydantic_monty.AsyncFutureSnapshot)
                result = await progress.resume({0: {'return_value': 99}})
                assert isinstance(result, pydantic_monty.MontyComplete)
                assert result.output == snapshot(99)
            ready.set()
            assert await asyncio.wait_for(original.feed_run('await f'), timeout=5) == snapshot(42)


async def test_session_exit_aborts_system_sleep_before_worker_reuse():
    async with pydantic_monty.AsyncMonty(min_processes=1, max_processes=1) as pool:

        async def run():
            async with pool.checkout() as session:
                worker_pid = session.worker_pid
                await session.feed_run('import asyncio\nf = asyncio.sleep(60)')
            async with pool.checkout() as session:
                assert session.worker_pid == worker_pid
                assert await session.feed_run('1 + 1') == snapshot(2)
                with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
                    await session.feed_run('f')
                assert str(exc_info.value.exception()) == snapshot("name 'f' is not defined")

        await asyncio.wait_for(run(), timeout=5)


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
