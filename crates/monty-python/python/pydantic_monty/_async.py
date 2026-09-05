"""Own Python callbacks for one automatically driven async feed."""

import asyncio
from collections.abc import Awaitable, Callable, Coroutine
from contextlib import suppress
from typing import Any


class CallbackTasks:
    """Cancel and join unfinished callbacks owned by one feed."""

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
        try:
            return await start()
        finally:
            await self.close()

    async def close(self) -> None:
        """Cancel callbacks and join them, forwarding any further caller cancellation."""
        self._closed = True
        for coro in self._pending.values():
            # Ordinary cleanup errors must not replace the feed outcome or skip other callbacks.
            with suppress(Exception, asyncio.CancelledError):
                coro.close()
        self._pending.clear()
        tasks = self._tasks.copy()
        self._tasks.clear()
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
