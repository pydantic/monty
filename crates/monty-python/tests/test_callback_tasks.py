"""Callback registration can race with completion of the native feed."""

import asyncio
import gc
import inspect
import weakref
from collections.abc import Coroutine
from contextlib import suppress
from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty._async as async_callbacks
from pydantic_monty import MontyCallbackCleanupError
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


@pytest.mark.parametrize('cleanup_raises', [False, True])
async def test_callback_outliving_cleanup_deadline_transfers_ownership(
    monkeypatch: pytest.MonkeyPatch, cleanup_raises: bool
):
    monkeypatch.setattr(async_callbacks, '_CLEANUP_TIMEOUT', 0.01)
    callbacks = CallbackTasks()
    started = asyncio.Event()
    releases: list[weakref.ReferenceType[asyncio.Event]] = []
    sibling_started = asyncio.Event()
    errors: list[dict[str, Any]] = []
    loop = asyncio.get_running_loop()
    previous_handler = loop.get_exception_handler()
    loop.set_exception_handler(lambda _loop, context: errors.append(context))

    async def background():
        try:
            started.set()
            await asyncio.Event().wait()
        finally:
            release = asyncio.Event()
            releases.append(weakref.ref(release))
            await release.wait()
            if cleanup_raises:
                raise RuntimeError('late cleanup failed')

    async def complete():
        await started.wait()
        await sibling_started.wait()
        return 42

    async def sibling():
        try:
            sibling_started.set()
            await asyncio.Event().wait()
        finally:
            raise ValueError('sibling cleanup failed')

    sibling_task = asyncio.create_task(callbacks.wrap(sibling()))
    sibling_ref = weakref.ref(sibling_task)
    del sibling_task
    task = asyncio.create_task(callbacks.wrap(background()))
    task_ref = weakref.ref(task)
    del task
    driver = callbacks.run(complete)
    observations: dict[str, Any] = {}
    try:
        with pytest.raises(MontyCallbackCleanupError) as exc_info:
            await asyncio.wait_for(asyncio.shield(driver), timeout=1)
        observations['error'] = str(exc_info.value)
        pending = exc_info.value.tasks
        observations['pending_count'] = len(pending)
        observations['pending_identity'] = pending[0] is task_ref()
        gc.collect()
        observations['sibling_released'] = sibling_ref() is None
        del callbacks, driver, exc_info
        gc.collect()
        observations['host_owns_pending'] = task_ref() is not None
        release = releases[0]()
        assert release is not None
        release.set()
        del release
        await asyncio.gather(*pending, return_exceptions=True)
        del pending
        await asyncio.sleep(0)
        gc.collect()
        observations['pending_released'] = task_ref() is None
    finally:
        task = task_ref()
        if releases and (release := releases[0]()) is not None:
            release.set()
        if task is not None:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        loop.set_exception_handler(previous_handler)
    # Snapshot capture retains frame locals, so measure ownership before calling it.
    assert observations == snapshot(
        {
            'error': 'Async callback cleanup timed out',
            'pending_count': 1,
            'pending_identity': True,
            'sibling_released': True,
            'host_owns_pending': True,
            'pending_released': True,
        }
    )
    assert errors == snapshot([])


async def test_discarded_cleanup_errors_do_not_retain_callbacks_globally(monkeypatch: pytest.MonkeyPatch):
    """Ignoring ownership transfer abandons cleanup, but must not create global roots."""
    monkeypatch.setattr(async_callbacks, '_CLEANUP_TIMEOUT', 0.01)
    refs: list[weakref.ReferenceType[asyncio.Task[Any]]] = []
    loop = asyncio.get_running_loop()
    previous_handler = loop.get_exception_handler()
    loop.set_exception_handler(lambda _loop, _context: None)

    async def abandon_feed():
        callbacks = CallbackTasks()
        started = asyncio.Event()

        async def background():
            started.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                await asyncio.Event().wait()

        async def complete():
            await started.wait()

        task = asyncio.create_task(callbacks.wrap(background()))
        refs.append(weakref.ref(task))
        with pytest.raises(MontyCallbackCleanupError):
            await callbacks.run(complete)

    try:
        for _ in range(3):
            await abandon_feed()
        await asyncio.sleep(0)
        gc.collect()
        released = [ref() is None for ref in refs]
    finally:
        tasks = [task for ref in refs if (task := ref()) is not None]
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        loop.set_exception_handler(previous_handler)
    assert released == snapshot([True, True, True])
