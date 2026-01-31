from __future__ import annotations

from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any, Callable, Literal, NamedTuple, TypedDict, TypeVar

if TYPE_CHECKING:
    from collections.abc import Awaitable
    from types import EllipsisType

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
    dir_stat,
    file_stat,
    symlink_stat,
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
    'run_monty_async',
    'ResourceLimits',
    'ExternalResult',
    'file_stat',
    'dir_stat',
    'symlink_stat',
    'OsFunction',
    'AbstractFileSystem',
    'StatResult',
)
T = TypeVar('T')

OsFunction = Literal[
    'Path.exists',
    'Path.is_file',
    'Path.is_dir',
    'Path.is_symlink',
    'Path.read_text',
    'Path.read_bytes',
    'Path.write_text',
    'Path.write_bytes',
    'Path.mkdir',
    'Path.unlink',
    'Path.rmdir',
    'Path.iterdir',
    'Path.stat',
    'Path.rename',
    'Path.resolve',
    'Path.absolute',
]


async def run_monty_async(
    monty_runner: Monty,
    *,
    inputs: dict[str, Any] | None = None,
    external_functions: dict[str, Callable[..., Any]] | None = None,
    limits: ResourceLimits | None = None,
    print_callback: Callable[[Literal['stdout'], str], None] | None = None,
) -> Any:
    """Run a Monty script with async external functions.

    Args:
        monty_runner: The Monty runner to use.
        external_functions: A dictionary of external functions to use, can be sync or async.
        inputs: A dictionary of inputs to use.
        limits: The resource limits to use.
        print_callback: A callback to use for printing.

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
                    if ext_function := external_functions.get(progress.function_name):
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


class AbstractFileSystem(ABC):
    """Abstract base class for implementing virtual filesystems.

    Subclass this and implement the abstract methods to provide a custom
    filesystem that Monty code can interact with via Path methods.

    Pass an instance as the `os_callback` parameter to `Monty.run()`.
    """

    def __call__(self, function_name: OsFunction, args: tuple[Any, ...]) -> Any:
        match function_name:
            case 'Path.exists':
                return self.path_exists(*args)
            case 'Path.is_file':
                return self.path_is_file(*args)
            case 'Path.is_dir':
                return self.path_is_dir(*args)
            case 'Path.is_symlink':
                return self.path_is_symlink(*args)
            case 'Path.read_text':
                return self.path_read_text(*args)
            case 'Path.read_bytes':
                return self.path_read_bytes(*args)
            case 'Path.write_text':
                return self.path_write_text(*args)
            case 'Path.write_bytes':
                return self.path_write_bytes(*args)
            case 'Path.mkdir':
                return self.path_mkdir(*args)
            case 'Path.unlink':
                return self.path_unlink(*args)
            case 'Path.rmdir':
                return self.path_rmdir(*args)
            case 'Path.iterdir':
                return self.path_iterdir(*args)
            case 'Path.stat':
                return self.path_stat(*args)
            case 'Path.rename':
                return self.path_rename(*args)
            case 'Path.resolve':
                return self.path_resolve(*args)
            case 'Path.absolute':
                return self.path_absolute(*args)

    @abstractmethod
    def path_exists(self, path: str) -> bool:
        raise NotImplementedError

    @abstractmethod
    def path_is_file(self, path: str) -> bool:
        raise NotImplementedError

    @abstractmethod
    def path_is_dir(self, path: str) -> bool:
        raise NotImplementedError

    @abstractmethod
    def path_is_symlink(self, path: str) -> bool:
        raise NotImplementedError

    @abstractmethod
    def path_read_text(self, path: str) -> str:
        raise NotImplementedError

    @abstractmethod
    def path_read_bytes(self, path: str) -> bytes:
        raise NotImplementedError

    @abstractmethod
    def path_write_text(self, path: str, data: str) -> None:
        raise NotImplementedError

    @abstractmethod
    def path_write_bytes(self, path: str, data: bytes) -> None:
        raise NotImplementedError

    @abstractmethod
    def path_mkdir(self, path: str, parents: bool, exist_ok: bool) -> None:
        raise NotImplementedError

    @abstractmethod
    def path_unlink(self, path: str) -> None:
        raise NotImplementedError

    @abstractmethod
    def path_rmdir(self, path: str) -> None:
        raise NotImplementedError

    @abstractmethod
    def path_iterdir(self, path: str) -> list[str]:
        raise NotImplementedError

    @abstractmethod
    def path_stat(self, path: str) -> StatResult:
        """Return stat result for the path.

        Use file_stat(), dir_stat(), or symlink_stat() helpers to create the return value.
        """
        raise NotImplementedError

    @abstractmethod
    def path_rename(self, path: str, target: str) -> None:
        raise NotImplementedError

    @abstractmethod
    def path_resolve(self, path: str) -> str:
        raise NotImplementedError

    @abstractmethod
    def path_absolute(self, path: str) -> str:
        raise NotImplementedError


class StatResult(NamedTuple):
    """Equivalent to os.stat_result"""

    st_mode: int
    """protection bits"""

    st_ino: int
    """inode"""

    st_dev: int
    """device"""

    st_nlink: int
    """number of hard links"""

    st_uid: int
    """user ID of owner"""

    st_gid: int
    """group ID of owner"""

    st_size: int
    """total size, in bytes"""

    st_atime: float
    """time of last access"""

    st_mtime: float
    """time of last modification"""

    st_ctime: float
    """time of last change"""
