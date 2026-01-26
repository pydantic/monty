from __future__ import annotations

import asyncio
from collections.abc import Awaitable
from functools import partial
from types import EllipsisType
from typing import Any, Callable, Literal, TypedDict, TypeVar

from ._monty import (
    Frame,
    Monty,
    MontyComplete,
    MontyError,
    MontyFutureSnapshot,
    MontyRuntimeError,
    MontySnapshot,
    MontySyntaxError,
    MontyTypingError,
    __version__,
)

__all__ = (
    'Monty',
    'MontyComplete',
    'MontySnapshot',
    'MontyFutureSnapshot',
    'MontyError',
    'MontySyntaxError',
    'MontyRuntimeError',
    'MontyTypingError',
    'Frame',
    '__version__',
    'AsyncMonty',
    'ResourceLimits',
    'ExternalResult',
)
T = TypeVar('T')


class AsyncMonty:
    _runner: Monty
    _tasks: dict[int, asyncio.Task[tuple[int, ExternalResult]]]

    def __init__(self, monty_runner: Monty):
        self._runner = monty_runner
        self._tasks = {}

    async def run(
        self,
        *,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        inputs: dict[str, Any] | None = None,
        limits: ResourceLimits | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | None = None,
    ) -> Any:
        import inspect
        from concurrent.futures import ThreadPoolExecutor

        external_functions = external_functions or {}

        loop = asyncio.get_running_loop()

        with ThreadPoolExecutor() as pool:

            async def run_in_pool(func: Callable[[], T]) -> T:
                return await loop.run_in_executor(pool, func)

            progress = await run_in_pool(
                partial(self._runner.start, inputs=inputs, limits=limits, print_callback=print_callback)
            )
            try:
                while True:
                    if isinstance(progress, MontyComplete):
                        return progress.output
                    elif isinstance(progress, MontySnapshot):
                        if ext_function := external_functions.get(progress.function_name):
                            try:
                                result = ext_function(*progress.args, **progress.kwargs)
                            except Exception as exc:
                                progress = await run_in_pool(partial(progress.resume, exception=exc))
                            else:
                                if inspect.iscoroutine(result):
                                    self._add_task(progress.call_id, result)
                                    progress = await run_in_pool(partial(progress.resume, future=...))
                                else:
                                    progress = await run_in_pool(partial(progress.resume, return_value=result))
                        else:
                            e = KeyError(f'Function {progress.function_name} not found')
                            progress = await run_in_pool(partial(progress.resume, exception=e))
                    else:
                        assert isinstance(progress, MontyFutureSnapshot)
                        results = await self._get_results(progress.pending_call_ids)
                        progress = await run_in_pool(partial(progress.resume, results))

            finally:
                await self._finish()

    def _add_task(self, call_id: int, coro: Awaitable[Any]):
        self._tasks[call_id] = asyncio.create_task(self._run_external_function(call_id, coro))

    async def _get_results(self, call_ids: list[int]) -> dict[int, ExternalResult]:
        tasks: list[asyncio.Task[tuple[int, ExternalResult]]] = []
        for call_id in call_ids:
            if task := self._tasks.get(call_id):
                tasks.append(task)

        done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)

        results: dict[int, ExternalResult] = {}
        for task in done:
            call_id, result = await task
            results[call_id] = result
            self._tasks.pop(call_id)

        return results

    async def _run_external_function(self, call_id: int, coro: Awaitable[Any]) -> tuple[int, ExternalResult]:
        try:
            result = await coro
        except Exception as e:
            return call_id, ExternalException(exception=e)
        else:
            return call_id, ExternalReturnValue(return_value=result)

    async def _finish(self) -> None:
        for task in self._tasks.values():
            task.cancel()
        try:
            await asyncio.gather(*self._tasks.values())
        except asyncio.CancelledError:
            pass


class ResourceLimits(TypedDict, total=False):
    """
    Configuration for resource limits during code execution.

    All limits are optional. Omit a key to disable that limit.
    """

    max_allocations: int
    """Maximum number of heap allocations allowed."""

    max_duration_secs: float
    """Maximum execution time in seconds."""

    max_memory: int
    """Maximum heap memory in bytes."""

    gc_interval: int
    """Run garbage collection every N allocations."""

    max_recursion_depth: int
    """Maximum function call stack depth (default: 1000)."""


class ExternalReturnValue(TypedDict):
    return_value: Any


class ExternalException(TypedDict):
    exception: Exception


class ExternalFuture(TypedDict):
    future: EllipsisType


ExternalResult = ExternalReturnValue | ExternalException | ExternalFuture
