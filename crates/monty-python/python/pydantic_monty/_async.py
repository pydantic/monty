"""Own Python callbacks for one automatically driven async feed."""

import asyncio
from collections.abc import Awaitable, Callable, Coroutine
from contextlib import suppress
from typing import Any

_CLEANUP_TIMEOUT = 1.0


class MontyCallbackCleanupError(TimeoutError):
    """Python callbacks did not finish cancellation cleanup within one second.

    The host must retain `tasks` until they finish and join them with
    `await asyncio.gather(*error.tasks, return_exceptions=True)`.
    Cancellation remains cooperative; joining these tasks has no time bound.
    """

    def __init__(self, tasks: tuple[asyncio.Task[Any], ...]) -> None:
        super().__init__('Async callback cleanup timed out')
        self.tasks = tasks
        """Unfinished callbacks whose ownership has transferred to the host."""


class CallbackTasks:
    """Cancel unfinished callbacks and give their cleanup a bounded wait."""

    def __init__(self) -> None:
        self._tasks: set[asyncio.Task[Any]] = set()
        self._pending: dict[int, Coroutine[Any, Any, Any]] = {}
        self._closed = False

    def run(self, start: Callable[[], Awaitable[Any]]) -> asyncio.Task[Any]:
        """Start the native drive only after its cleanup owner is running."""
        return asyncio.create_task(self._run(start))

    def wrap(self, coro: Coroutine[Any, Any, Any]) -> Coroutine[Any, Any, Any]:
        """Register callbacks before the Rust bridge schedules them on asyncio."""
        if self._closed:
            coro.close()
        else:
            self._pending[id(coro)] = coro
        return self._call(coro)

    async def _call(self, coro: Coroutine[Any, Any, Any]) -> Any:
        self._pending.pop(id(coro), None)
        if self._closed:
            raise asyncio.CancelledError
        task = asyncio.current_task()
        assert task is not None
        self._tasks.add(task)
        try:
            return await coro
        finally:
            self._tasks.discard(task)

    async def _run(self, start: Callable[[], Awaitable[Any]]) -> Any:
        cancelled = None
        try:
            return await start()
        except asyncio.CancelledError as exc:
            cancelled = exc
            raise
        finally:
            error, cleanup_cancelled = await self.close()
            if cancelled is not None or cleanup_cancelled:
                raise cancelled or asyncio.CancelledError() from error
            if error is not None:
                raise error

    async def close(self) -> tuple[MontyCallbackCleanupError | None, bool]:
        """Return overdue callbacks and caller cancellation after a bounded cleanup wait."""
        self._closed = True
        for coro in self._pending.values():
            # Ordinary cleanup errors must not replace the feed outcome or skip other callbacks.
            with suppress(Exception, asyncio.CancelledError):
                coro.close()
        self._pending.clear()
        tasks = self._tasks.copy()
        self._tasks.clear()
        for task in tasks:
            task.add_done_callback(_consume_exception)
            task.cancel()
        cancelled = False
        if tasks:
            loop = asyncio.get_running_loop()
            deadline = loop.time() + _CLEANUP_TIMEOUT
            while tasks:
                remaining = deadline - loop.time()
                if remaining <= 0:
                    break
                try:
                    # Caller cancellation must not interrupt the callbacks' cleanup.
                    _, tasks = await asyncio.wait(tasks, timeout=remaining)
                except asyncio.CancelledError:
                    cancelled = True
            tasks = {task for task in tasks if not task.done()}
        # Return, rather than raise, so timeout and cancellation cannot form a context cycle.
        return MontyCallbackCleanupError(tuple(tasks)) if tasks else None, cancelled


def _consume_exception(task: asyncio.Task[Any]) -> None:
    """Retrieve each callback's exception without retaining its siblings."""
    if not task.cancelled():
        task.exception()
