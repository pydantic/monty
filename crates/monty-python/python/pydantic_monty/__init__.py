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
    MontyConversionError,
    MontyCrashedError,
    MontyDisconnectError,
    MontyError,
    MontyFileHandle,
    MontyRuntimeError,
    MontySession,
    MontyShutdown,
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
    'TypeCheckFormat',
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
    'MontyConversionError',
    'MontyCrashedError',
    'MontyDisconnectError',
    'MontyError',
    'MontyFileHandle',
    'MontySession',
    'MontyShutdown',
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
    to disable that limit, with two exceptions: `max_recursion_depth` and
    `max_suspensions_per_run` cannot be disabled, and omitting them leaves
    their defaults in place.
    """

    max_duration_secs: float | None
    """Maximum execution time in seconds."""

    max_memory: int | None
    """Maximum heap memory in bytes."""

    gc_interval: int | None
    """Run garbage collection every N allocations."""

    max_recursion_depth: int | None
    """Maximum function call stack depth (default: 1000)."""

    max_suspensions_per_run: int | None
    """
    Maximum host round trips in a single run (default: 10000).

    Every host function call, name lookup and `os` callback suspends the
    sandbox and costs the host retained state, which no sandbox-side limit
    can see. A run is one `feed_run` and every resume that continues it.
    """

    max_total_suspensions: int | None
    """
    Maximum host round trips across the whole session.

    Unset by default, so only each run is bounded. Set it to stop code
    sidestepping `max_suspensions_per_run` by feeding repeatedly.
    """


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
    'binascii.Error',
]
"""String names of Python exception types that Monty understands.

Used by `ExternalExceptionData` to identify an exception by name rather than
passing a concrete Python exception instance. Names match Python's built-in
exception classes, except for `json.JSONDecodeError`, `re.PatternError` and
`binascii.Error`, which are dotted to disambiguate from their `ValueError` /
`Exception` parents.
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

TypeCheckFormat: TypeAlias = Literal[
    'full', 'concise', 'azure', 'json', 'jsonlines', 'rdjson', 'pylint', 'gitlab', 'github'
]
"""How `MontyTypingError` diagnostics are rendered — ty's diagnostic formats.

Picked by `checkout(type_check_format=...)`, not on the raised error: the type
checker runs inside the worker and its structured diagnostics never leave it,
so only the already-rendered text crosses the wire."""

OsHandler: TypeAlias = Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | AbstractOS
"""OS-call handler shared by `feed_run` / `feed_start`."""

SyncSnapshot: TypeAlias = FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete
"""What `MontySession.feed_start` (and each sync `resume` / `resume_auto`) yields."""

AsyncSnapshot: TypeAlias = AsyncFunctionSnapshot | AsyncNameLookupSnapshot | AsyncFutureSnapshot | MontyComplete
"""What `AsyncMontySession.feed_start` (and each async `resume` / `resume_auto`) yields."""
