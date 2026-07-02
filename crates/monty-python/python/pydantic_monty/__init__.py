from __future__ import annotations

from types import EllipsisType
from typing import Any, Callable, Literal

from typing_extensions import NotRequired, TypeAlias, TypedDict

from ._monty import (
    NOT_HANDLED,
    AsyncFunctionSnapshot,
    AsyncFutureSnapshot,
    AsyncMonty,
    AsyncMontySession,
    AsyncMontyWebsocket,
    AsyncNameLookupSnapshot,
    CollectStreams,
    CollectString,
    Frame,
    FunctionSnapshot,
    FutureSnapshot,
    Monty,
    MontyComplete,
    MontyCrashedError,
    MontyError,
    MontyFileHandle,
    MontyRuntimeError,
    MontySession,
    MontySyntaxError,
    MontyTypingError,
    MountDir,
    NameLookupSnapshot,
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
    'ExternalResult',
    'ExternalSettledResult',
    'ExternalReturnValue',
    'ExternalException',
    'ExternalExceptionData',
    'ExternalFuture',
    'ExcType',
    'PrintCallback',
    'OsHandler',
    'SyncSnapshot',
    'AsyncSnapshot',
    # _monty
    '__version__',
    'AsyncMonty',
    'AsyncMontySession',
    'AsyncMontyWebsocket',
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
    # feed_start snapshots
    'MontyComplete',
    'FunctionSnapshot',
    'NameLookupSnapshot',
    'FutureSnapshot',
    'AsyncFunctionSnapshot',
    'AsyncNameLookupSnapshot',
    'AsyncFutureSnapshot',
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


class ExternalReturnValue(TypedDict):
    """Represents the return value of an external function call."""

    return_value: Any


class ExternalException(TypedDict):
    """Represents an exception raised during an external function call."""

    exception: BaseException


ExcType = Literal[
    'Exception',
    'BaseException',
    'SystemExit',
    'KeyboardInterrupt',
    'ArithmeticError',
    'OverflowError',
    'ZeroDivisionError',
    'LookupError',
    'IndexError',
    'KeyError',
    'RuntimeError',
    'NotImplementedError',
    'RecursionError',
    'AttributeError',
    'FrozenInstanceError',
    'NameError',
    'UnboundLocalError',
    'ValueError',
    'UnicodeDecodeError',
    'UnicodeEncodeError',
    'json.JSONDecodeError',
    'ImportError',
    'ModuleNotFoundError',
    'OSError',
    'FileNotFoundError',
    'FileExistsError',
    'IsADirectoryError',
    'NotADirectoryError',
    'PermissionError',
    'io.UnsupportedOperation',
    'AssertionError',
    'MemoryError',
    'StopIteration',
    'SyntaxError',
    'TimeoutError',
    'TypeError',
    're.PatternError',
]
"""String names of Python exception types that Monty understands.

Used by `ExternalExceptionData` to identify an exception by name rather than
passing a concrete Python exception instance. Names match Python's built-in
exception classes, except for `json.JSONDecodeError` and `re.PatternError`
which are dotted to disambiguate from their `ValueError` / `Exception`
parents.
"""


class ExternalExceptionData(TypedDict):
    """Represents an exception raised during an external function call by its type and optional message.

    Prefer this variant over `ExternalException` when the caller does not have
    (or does not want to construct) a concrete Python exception instance —
    e.g. when resuming a snapshot whose original exception type is not
    available, or when resuming from another language.
    """

    exc_type: ExcType
    message: NotRequired[str]


class ExternalFuture(TypedDict):
    """Represents a pending future returned from an external function call."""

    future: EllipsisType


ExternalSettledResult = ExternalReturnValue | ExternalException | ExternalExceptionData
"""A *settled* answer — a return value or an exception, but never a pending
`future`. Resolving a `FutureSnapshot` requires settled results: a future
cannot resolve to another future."""

ExternalResult = ExternalSettledResult | ExternalFuture
"""A caller's answer to a `FunctionSnapshot`: a return value, an exception (by
instance or by type name), or a pending `future`."""

PrintCallback: TypeAlias = Callable[[Literal['stdout', 'stderr'], str], None] | CollectStreams | CollectString
"""Print sink accepted by `feed_run` / `feed_start` / `load_snapshot`."""

OsHandler: TypeAlias = Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | AbstractOS
"""OS-call handler shared by `feed_run` / `feed_start`."""

SyncSnapshot: TypeAlias = FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete
"""What `MontySession.feed_start` (and each sync `resume`) yields."""

AsyncSnapshot: TypeAlias = AsyncFunctionSnapshot | AsyncNameLookupSnapshot | AsyncFutureSnapshot | MontyComplete
"""What `AsyncMontySession.feed_start` (and each async `resume`) yields."""
