from __future__ import annotations

from typing import TYPE_CHECKING, Any, Callable, Literal, TypedDict

from typing_extensions import deprecated

if TYPE_CHECKING:
    from types import EllipsisType

from ._monty import (
    NOT_HANDLED,
    CollectStreams,
    CollectString,
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
    MountDir,
    NameLookupSnapshot,
    __version__,
    load_repl_snapshot,
    load_snapshot,
)
from .os_access import (
    AbstractFile,
    AbstractOS,
    CallbackFile,
    MemoryFile,
    OSAccess,
    OsFunction,
    StatResult,
)

__all__ = (
    # this file
    'run_monty_async',
    'run_repl_async',
    'ExternalResult',
    'ResourceLimits',
    # _monty
    '__version__',
    'CollectStreams',
    'CollectString',
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
    'MountDir',
    'load_snapshot',
    'load_repl_snapshot',
    # os_access
    'StatResult',
    'OsFunction',
    'NOT_HANDLED',
    'AbstractOS',
    'AbstractFile',
    'MemoryFile',
    'CallbackFile',
    'OSAccess',
)


@deprecated('Use Monty.run_async() instead')
async def run_monty_async(
    monty_runner: Monty,
    *,
    inputs: dict[str, Any] | None = None,
    external_functions: dict[str, Callable[..., Any]] | None = None,
    limits: ResourceLimits | None = None,
    print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
    os: AbstractOS | None = None,
) -> Any:
    return await monty_runner.run_async(
        inputs=inputs,
        external_functions=external_functions,
        limits=limits,
        print_callback=print_callback,
        os=os,
    )


@deprecated('Use MontyRepl.feed_run_async() instead')
async def run_repl_async(
    repl: MontyRepl,
    code: str,
    *,
    inputs: dict[str, Any] | None = None,
    external_functions: dict[str, Callable[..., Any]] | None = None,
    print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
    os: AbstractOS | None = None,
) -> Any:
    return await repl.feed_run_async(
        code,
        inputs=inputs,
        external_functions=external_functions,
        print_callback=print_callback,
        os=os,
    )


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
    """Represents the return value of an external function call."""

    return_value: Any


class ExternalException(TypedDict):
    """Represents an exception raised during an external function call."""

    exception: Exception


class ExternalFuture(TypedDict):
    """Represents a pending future returned from an external function call."""

    future: EllipsisType


ExternalResult = ExternalReturnValue | ExternalException | ExternalFuture
