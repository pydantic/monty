from __future__ import annotations

from typing import TYPE_CHECKING, Any, Callable, Literal, TypedDict, TypeVar, cast

if TYPE_CHECKING:
    from collections.abc import Awaitable
    from types import EllipsisType

from ._monty import (
    Frame,
    Monty,
    MontyComplete,
    MontyError,
    MontyFutureSnapshot,
    MontyRepl,
    MontyRuntimeError,
    MontySnapshot,
    MontySyntaxError,
    MontyTypingError,
    __version__,
)
from .os_access import AbstractFile, AbstractOS, CallbackFile, MemoryFile, OSAccess, OsFunction, StatResult

__all__ = (
    # this file
    'run_monty_async',
    'ExternalResult',
    'ResourceLimits',
    # _monty
    '__version__',
    'Monty',
    'MontyRepl',
    'MontyComplete',
    'MontySnapshot',
    'MontyFutureSnapshot',
    'MontyError',
    'MontySyntaxError',
    'MontyRuntimeError',
    'MontyTypingError',
    'Frame',
    # os_access
    'StatResult',
    'OsFunction',
    'AbstractOS',
    'AbstractFile',
    'MemoryFile',
    'CallbackFile',
    'OSAccess',
)
T = TypeVar('T')


async def run_monty_async(
    monty_runner: Monty,
    *,
    inputs: dict[str, Any] | None = None,
    external_functions: dict[str, Callable[..., Any]] | None = None,
    limits: ResourceLimits | None = None,
    print_callback: Callable[[Literal['stdout'], str], None] | None = None,
    os: AbstractOS | None = None,
) -> Any:
    """Run a Monty script with async external functions and optional OS access.

    This function provides a convenient way to run Monty code that uses both async
    external functions and filesystem operations via OSAccess.

    Args:
        monty_runner: The Monty runner to use.
        external_functions: A dictionary of external functions to use, can be sync or async.
        inputs: A dictionary of inputs to use.
        limits: The resource limits to use.
        print_callback: A callback to use for printing.
        os: Optional OS access handler for filesystem operations (e.g., OSAccess instance).

    Returns:
        The output of the Monty script.
    """
    import asyncio
    import inspect
    from concurrent.futures import ThreadPoolExecutor
    from functools import partial

    loop = asyncio.get_running_loop()
    external_functions = external_functions or {}
    tasks: dict[int, asyncio.Task[tuple[int, ExternalResult]]] = {}

    with ThreadPoolExecutor() as pool:

        async def run_in_pool(func: Callable[[], T]) -> T:
            return await loop.run_in_executor(pool, func)

        progress = await run_in_pool(
            partial(monty_runner.start, inputs=inputs, limits=limits, print_callback=print_callback)
        )

        try:
            while True:
                if isinstance(progress, MontyComplete):
                    return progress.output
                elif isinstance(progress, MontySnapshot):
                    # Handle OS function calls (e.g., Path.read_text, Path.exists)
                    if progress.is_os_function:
                        # When is_os_function is True, function_name is always an OsFunction
                        os_func_name = cast(OsFunction, progress.function_name)
                        if os is None:
                            e = NotImplementedError(
                                f'OS function {progress.function_name} called but no os handler provided'
                            )
                            progress = await run_in_pool(partial(progress.resume, exception=e))
                        else:
                            try:
                                result = os(os_func_name, progress.args, progress.kwargs)
                            except Exception as exc:
                                progress = await run_in_pool(partial(progress.resume, exception=exc))
                            else:
                                progress = await run_in_pool(partial(progress.resume, return_value=result))
                    # Handle external function calls
                    elif ext_function := external_functions.get(progress.function_name):
                        try:
                            result = ext_function(*progress.args, **progress.kwargs)
                        except Exception as exc:
                            progress = await run_in_pool(partial(progress.resume, exception=exc))
                        else:
                            if inspect.iscoroutine(result):
                                call_id = progress.call_id
                                tasks[call_id] = asyncio.create_task(_run_external_function(call_id, result))
                                progress = await run_in_pool(partial(progress.resume, future=...))
                            else:
                                progress = await run_in_pool(partial(progress.resume, return_value=result))
                    else:
                        e = KeyError(f'Function {progress.function_name} not found')
                        progress = await run_in_pool(partial(progress.resume, exception=e))
                else:
                    assert isinstance(progress, MontyFutureSnapshot), f'Unexpected progress type {progress!r}'

                    current_tasks: list[asyncio.Task[tuple[int, ExternalResult]]] = []
                    for call_id in progress.pending_call_ids:
                        if task := tasks.get(call_id):
                            current_tasks.append(task)

                    done, _ = await asyncio.wait(current_tasks, return_when=asyncio.FIRST_COMPLETED)

                    results: dict[int, ExternalResult] = {}
                    for task in done:
                        call_id, result = task.result()
                        results[call_id] = result
                        tasks.pop(call_id)

                    progress = await run_in_pool(partial(progress.resume, results))

        finally:
            for task in tasks.values():
                task.cancel()
            try:
                await asyncio.gather(*tasks.values())
            except asyncio.CancelledError:
                pass


async def _run_external_function(call_id: int, coro: Awaitable[Any]) -> tuple[int, ExternalResult]:
    try:
        result = await coro
    except Exception as e:
        return call_id, ExternalException(exception=e)
    else:
        return call_id, ExternalReturnValue(return_value=result)


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
