"""Callback registration can race with completion of the native feed."""

import asyncio
import inspect
from collections.abc import Coroutine
from contextlib import suppress
from typing import Any

import pytest
from inline_snapshot import snapshot

from pydantic_monty._async import CallbackTasks


@pytest.mark.parametrize('register_before_exit', [True, False])
async def test_callback_queued_after_feed_exit_never_starts(register_before_exit: bool):
    callbacks = CallbackTasks()
    started = False

    async def background():
        nonlocal started
        started = True

    async def complete():
        return 42

    coro = background()
    wrapped = callbacks.wrap(coro) if register_before_exit else None
    try:
        assert await callbacks.run(complete) == snapshot(42)
        if wrapped is None:
            wrapped = callbacks.wrap(coro)
        with pytest.raises(asyncio.CancelledError):
            await wrapped
        assert started == snapshot(False)
        assert inspect.getcoroutinestate(coro) == snapshot('CORO_CLOSED')
    finally:
        coro.close()
        if wrapped is not None:
            wrapped.close()


@pytest.mark.parametrize('feed_raises', [False, True])
@pytest.mark.parametrize('cleanup_error', [RuntimeError, asyncio.CancelledError])
async def test_pending_callback_cleanup_error_preserves_outcome_and_other_cleanup(
    feed_raises: bool, cleanup_error: type[BaseException]
):
    """A returned coroutine may have started before the binding takes ownership."""
    callbacks = CallbackTasks()
    started = asyncio.Event()
    cleaned_up: list[str] = []

    async def active():
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned_up.append('active')

    async def pending(name: str, raises: bool):
        try:
            await asyncio.sleep(0)
        finally:
            cleaned_up.append(name)
            if raises:
                raise cleanup_error('pending cleanup failed')

    async def complete():
        if feed_raises:
            raise ValueError('feed failed')
        return 42

    active_task = asyncio.create_task(callbacks.wrap(active()))
    pending_coros = [pending('first', True), pending('second', False)]
    wrappers: list[Coroutine[Any, Any, Any]] = []
    try:
        await asyncio.wait_for(started.wait(), timeout=5)
        for coro in pending_coros:
            coro.send(None)
            wrappers.append(callbacks.wrap(coro))
        if feed_raises:
            with pytest.raises(ValueError) as exc_info:
                await callbacks.run(complete)
            assert str(exc_info.value) == snapshot('feed failed')
        else:
            assert await callbacks.run(complete) == snapshot(42)
        assert cleaned_up == snapshot(['first', 'second', 'active'])
        assert active_task.done() == snapshot(True)
        assert [inspect.getcoroutinestate(coro) for coro in pending_coros] == snapshot(['CORO_CLOSED', 'CORO_CLOSED'])
    finally:
        active_task.cancel()
        await asyncio.gather(active_task, return_exceptions=True)
        for coro in [*wrappers, *pending_coros]:
            with suppress(BaseException):
                coro.close()


@pytest.mark.parametrize('cleanup_error', [KeyboardInterrupt, SystemExit])
async def test_pending_callback_cleanup_preserves_host_control_flow(cleanup_error: type[BaseException]):
    callbacks = CallbackTasks()

    async def pending():
        try:
            await asyncio.sleep(0)
        finally:
            raise cleanup_error('host interrupted')

    coro = pending()
    coro.send(None)
    wrapped = callbacks.wrap(coro)
    try:
        # Await directly so asyncio's task-level interrupt handling cannot stop the test runner.
        with pytest.raises(cleanup_error):
            await callbacks.close()
    finally:
        wrapped.close()
        coro.close()
