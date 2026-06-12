from __future__ import annotations

from typing_extensions import TypedDict

from ._monty import (
    NOT_HANDLED,
    AsyncMonty,
    AsyncMontySession,
    CollectStreams,
    CollectString,
    Frame,
    Monty,
    MontyCrashedError,
    MontyError,
    MontyFileHandle,
    MontyRuntimeError,
    MontySession,
    MontySyntaxError,
    MontyTypingError,
    MountDir,
    __version__,
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
    'ResourceLimits',
    # _monty
    '__version__',
    'AsyncMonty',
    'AsyncMontySession',
    'CollectStreams',
    'CollectString',
    'Frame',
    'Monty',
    'MontyCrashedError',
    'MontyError',
    'MontyFileHandle',
    'MontySession',
    'MontySyntaxError',
    'MontyRuntimeError',
    'MontyTypingError',
    'MountDir',
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


class ResourceLimits(TypedDict, total=False):
    """
    Configuration for resource limits during code execution.

    All limits are optional. Omit a key — or set it to `None` explicitly —
    to disable that limit.
    """

    max_allocations: int | None
    """Maximum number of heap allocations allowed."""

    max_duration_secs: float | None
    """Maximum execution time in seconds."""

    max_memory: int | None
    """Maximum heap memory in bytes."""

    gc_interval: int | None
    """Run garbage collection every N allocations."""

    max_recursion_depth: int | None
    """Maximum function call stack depth (default: 1000)."""
