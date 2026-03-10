from __future__ import annotations

from typing import TYPE_CHECKING, Any, Callable, Literal, TypedDict, TypeVar, cast

if TYPE_CHECKING:
    from collections.abc import Awaitable
    from types import EllipsisType

from ._monty import (
    Frame,
    FunctionSnapshot,
    FutureSnapshot,
    Monty,
    MontyComplete,
    MontyError,
    MontyRepl,
    MontyRuntimeError,
    MontySyntaxError,
    MontyTypingError,
    NameLookupSnapshot,
    __version__,
    load_repl_snapshot,
    load_snapshot,
)
from .os_access import AbstractFile, AbstractOS, CallbackFile, MemoryFile, OSAccess, OsFunction, StatResult

__all__ = (
    # this file
    'run_monty_async',
    'run_repl_async',
    'ExternalResult',
    'ResourceLimits',
    # _monty
    '__version__',
    'Monty',
    'MontyRepl',
    'MontyComplete',
    'FunctionSnapshot',
    'NameLookupSnapshot',
    'FutureSnapshot',
    'MontyError',
    'MontySyntaxError',
    'MontyRuntimeError',
    'MontyTypingError',
    'Frame',
    'load_snapshot',
    'load_repl_snapshot',
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
    from functools import partial

    progress = await _run_in_pool(
        partial(monty_runner.start, inputs=inputs, limits=limits, print_callback=print_callback)
    )
    return await _dispatch_loop(progress, external_functions or {}, os)


async def run_repl_async(
    repl: MontyRepl,
    code: str,
    *,
    inputs: dict[str, Any] | None = None,
    external_functions: dict[str, Callable[..., Any]] | None = None,
    print_callback: Callable[[Literal['stdout'], str], None] | None = None,
    os: AbstractOS | None = None,
) -> Any:
    """Feed a snippet to a REPL session with async external function support.

    This is the REPL equivalent of `run_monty_async`. It calls `feed_start()` on
    the REPL and drives the snapshot/resume loop, dispatching external function
    calls (sync or async), OS calls, dataclass method calls, and future resolution.

    Args:
        repl: The REPL session to feed the snippet to.
        code: The Python code snippet to execute.
        external_functions: A dictionary of external functions to use, can be sync or async.
        inputs: A dictionary of inputs to use.
        print_callback: A callback to use for printing.
        os: Optional OS access handler for filesystem operations (e.g., OSAccess instance).

    Returns:
        The output of the snippet.
    """
    from functools import partial

    progress = await _run_in_pool(partial(repl.feed_start, code, inputs=inputs, print_callback=print_callback))
    return await _dispatch_loop(progress, external_functions or {}, os)


async def _run_in_pool(func: Callable[[], T]) -> T:
    """Run a function in a thread pool executor, releasing the GIL."""
    import asyncio
    from concurrent.futures import ThreadPoolExecutor

    loop = asyncio.get_running_loop()
    with ThreadPoolExecutor() as pool:
        return await loop.run_in_executor(pool, func)


async def _dispatch_loop(
    progress: FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete,
    external_functions: dict[str, Callable[..., Any]],
    os: AbstractOS | None,
) -> Any:
    """Drive the snapshot/resume loop for both Monty and MontyRepl.

    Handles external function calls (sync and async), OS calls, dataclass method
    calls, name lookups, and future resolution.
    """
    import asyncio
    import inspect
    from functools import partial

    tasks: dict[int, asyncio.Task[tuple[int, ExternalResult]]] = {}

    try:
        while True:
            if isinstance(progress, MontyComplete):
                return progress.output
            elif isinstance(progress, FunctionSnapshot):
                # Handle OS function calls (e.g., Path.read_text, Path.exists)
                if progress.is_os_function:
                    # When is_os_function is True, function_name is always an OsFunction
                    os_func_name = cast(OsFunction, progress.function_name)
                    if os is None:
                        e = NotImplementedError(
                            f'OS function {progress.function_name} called but no os handler provided'
                        )
                        progress = await _run_in_pool(partial(progress.resume, exception=e))
                    else:
                        try:
                            result = os(os_func_name, progress.args, progress.kwargs)
                        except Exception as exc:
                            progress = await _run_in_pool(partial(progress.resume, exception=exc))
                        else:
                            progress = await _run_in_pool(partial(progress.resume, return_value=result))
                # Handle dataclass method calls (first arg is the instance)
                elif progress.is_method_call:
                    self_obj = progress.args[0]
                    method = getattr(self_obj, progress.function_name)
                    remaining_args = progress.args[1:]
                    try:
                        result = method(*remaining_args, **progress.kwargs)
                    except Exception as exc:
                        progress = await _run_in_pool(partial(progress.resume, exception=exc))
                    else:
                        if inspect.iscoroutine(result):
                            call_id = progress.call_id
                            tasks[call_id] = asyncio.create_task(_run_external_function(call_id, result))
                            progress = await _run_in_pool(partial(progress.resume, future=...))
                        else:
                            progress = await _run_in_pool(partial(progress.resume, return_value=result))
                # Handle external function calls
                elif ext_function := external_functions.get(progress.function_name):
                    try:
                        result = ext_function(*progress.args, **progress.kwargs)
                    except Exception as exc:
                        progress = await _run_in_pool(partial(progress.resume, exception=exc))
                    else:
                        if inspect.iscoroutine(result):
                            call_id = progress.call_id
                            tasks[call_id] = asyncio.create_task(_run_external_function(call_id, result))
                            progress = await _run_in_pool(partial(progress.resume, future=...))
                        else:
                            progress = await _run_in_pool(partial(progress.resume, return_value=result))
                else:
                    e = LookupError(f"Unable to find '{progress.function_name}' in external functions dict")
                    progress = await _run_in_pool(partial(progress.resume, exception=e))
            elif isinstance(progress, NameLookupSnapshot):
                ext_function = external_functions.get(progress.variable_name)
                if ext_function is not None:
                    progress = await _run_in_pool(partial(progress.resume, value=ext_function))
                else:
                    progress = await _run_in_pool(progress.resume)
            else:
                assert isinstance(progress, FutureSnapshot), f'Unexpected progress type {progress!r}'

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

                progress = await _run_in_pool(partial(progress.resume, results))

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
