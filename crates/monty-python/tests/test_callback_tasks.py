"""Callback registration can race with completion of the native feed."""

import asyncio
import inspect

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
