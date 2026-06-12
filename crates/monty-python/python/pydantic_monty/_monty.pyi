from pathlib import Path
from typing import Any, Callable, Literal, final

from typing_extensions import Self

from . import ResourceLimits
from .os_access import AbstractOS, OsFunction

__all__ = [
    '__version__',
    'NOT_HANDLED',
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
]
__version__: str

NOT_HANDLED = object()

@final
class CollectStreams:
    """Collect printed output as `(stream, text)` tuples."""

    def __new__(cls) -> CollectStreams: ...
    @property
    def output(self) -> list[tuple[Literal['stdout', 'stderr'], str]]:
        """Collected output so far."""

@final
class CollectString:
    """Collect printed output as one concatenated string."""

    def __new__(cls) -> CollectString: ...
    @property
    def output(self) -> str:
        """Collected output so far."""

@final
class MountDir:
    """A single mount point configuration mapping a virtual path to a host directory."""

    virtual_path: str
    host_path: str
    mode: Literal['read-only', 'read-write', 'overlay']
    write_bytes_limit: int | None

    def __new__(
        cls,
        virtual_path: str,
        host_path: str | Path,
        *,
        mode: Literal['read-only', 'read-write', 'overlay'] = 'overlay',
        write_bytes_limit: int | None = None,
    ) -> MountDir: ...

class MontyError(Exception):
    """Base exception for all Monty interpreter errors.

    Catching `MontyError` will catch syntax, runtime, and typing errors from Monty.
    This exception is raised internally by Monty and cannot be constructed directly.
    """

    def exception(self) -> BaseException:
        """Returns the inner exception as a Python exception object."""

    def __str__(self) -> str:
        """Returns the exception message."""

@final
class MontySyntaxError(MontyError):
    """Raised when Python code has syntax errors or cannot be parsed by Monty.

    Inherits exception(), __str__() from MontyError.
    """

    def traceback(self) -> list[Frame]:
        """Returns the Monty traceback as a list of Frame objects."""

    def display(self, format: Literal['traceback', 'type-msg', 'msg'] = 'traceback') -> str:
        """Returns formatted exception string.

        Args:
            format: 'traceback' - full traceback with exception
                  'type-msg' - 'ExceptionType: message' format
                  'msg' - just the message
        """

@final
class MontyTypingError(MontyError):
    """Raised when type checking rejects a fed snippet.

    Type checking runs inside the worker subprocess; the diagnostics arrive
    pre-rendered as text.

    Inherits exception(), __str__() from MontyError.
    Cannot be constructed directly from Python.
    """

    def display(self) -> str:
        """Returns the rendered type-check diagnostics."""

@final
class MontyRuntimeError(MontyError):
    """Raised when Monty code fails during execution.

    Inherits exception(), __str__() from MontyError.
    Additionally provides traceback() and display() methods.
    """

    def traceback(self) -> list[Frame]:
        """Returns the Monty traceback as a list of Frame objects."""

    def display(self, format: Literal['traceback', 'type-msg', 'msg'] = 'traceback') -> str:
        """Returns formatted exception string.

        Args:
            format: 'traceback' - full traceback with exception
                  'type-msg' - 'ExceptionType: message' format
                  'msg' - just the message
        """

@final
class Frame:
    """A single frame in a Monty traceback."""

    @property
    def filename(self) -> str:
        """The filename where the code is located."""

    @property
    def line(self) -> int:
        """Line number (1-based)."""

    @property
    def column(self) -> int:
        """Column number (1-based)."""

    @property
    def end_line(self) -> int:
        """End line number (1-based)."""

    @property
    def end_column(self) -> int:
        """End column number (1-based)."""

    @property
    def function_name(self) -> str | None:
        """The name of the function, or None for module-level code."""

    @property
    def source_line(self) -> str | None:
        """The source code line for preview in the traceback."""

    def dict(self) -> dict[str, int | str | None]:
        """dict of attributes."""

@final
class MontyFileHandle:
    """Host-side handle to a file opened inside a Monty sandbox.

    Plain data holder — Monty never gives the host a live OS file descriptor.
    Exposed to callbacks (e.g. as the first argument of an `Open` result or
    a `read`/`write` request) so they can route on `path` and branch on
    `mode`/`binary`/`readable`/`writable` without re-parsing the mode string.

    Construct one from a Python `Open` OS handler to return a handle back to
    Monty: `MontyFileHandle('/data/foo.txt', 'r')`. The `mode` is canonicalized
    at construction (`'rt'` → `'r'`, `'r+b'` → `'rb+'`).
    """

    def __new__(cls, path: str, mode: str, *, position: int = 0) -> MontyFileHandle:
        """Construct a `MontyFileHandle` to return from an `Open` OS callback.

        Arguments:
            path: Virtual sandbox path of the opened file (POSIX-style).
            mode: Python `open()` mode string. Parsed and canonicalized at
                construction, so `'rt'` becomes `'r'` and `'r+b'` becomes
                `'rb+'`. Raises `ValueError` for malformed or unsupported
                modes (e.g. `'x'`).
            position: Initial position for sized/line/seek operations (char
                index in text mode, byte index in binary mode). Almost always
                `0` for a freshly opened file.
        """

    @property
    def path(self) -> str:
        """Virtual sandbox path of the open file (always POSIX-style, never a host path)."""

    @property
    def mode(self) -> str:
        """Canonical Python `open()` mode string for this file (e.g. `'r'`, `'rb+'`, `'w'`)."""

    @property
    def position(self) -> int:
        """Current position for sized/line/seek operations.

        Char index in text mode, byte index in binary mode. `0` for a freshly
        opened file.
        """

    @property
    def binary(self) -> bool:
        """`True` if the mode opens the file in binary form (`'rb'`, `'wb'`, …)."""

    @property
    def readable(self) -> bool:
        """`True` if the mode permits `read()` (`'r'`, `'r+'`, `'w+'`, `'a+'`, and binary variants)."""

    @property
    def writable(self) -> bool:
        """`True` if the mode permits `write()` (`'w'`, `'a'`, `'r+'`, `'w+'`, `'a+'`, and binary variants)."""

@final
class MontyCrashedError(MontyError):
    """Raised when a worker process died or hit `request_timeout`.

    This is the failure mode subprocess pools exist to contain: the sandbox
    process is gone (segfault, allocator abort, external kill, or watchdog
    timeout) but the host process is unharmed and the pool replaces the
    worker. Catch this error to retry or report.

    Cannot be constructed directly from Python.
    """

    @property
    def timed_out(self) -> bool:
        """`True` when the pool's `request_timeout` watchdog killed the worker."""

    @property
    def exit_status(self) -> int | None:
        """Exit code of the dead worker when the OS reported one (signal deaths report `None`)."""

@final
class Monty:
    """
    Sync context manager owning a pool of `monty` subprocess workers.

    Monty processes can never be made fully crash-proof against memory errors
    (stack overflow, allocator aborts), so execution always happens in worker
    subprocesses: a crashed worker raises `MontyCrashedError` and is replaced
    transparently — the host Python process is never at risk.

    ```python
    with Monty() as pool:
        with pool.checkout() as session:
            result = session.feed_run('1 + 1')
    ```
    """

    def __new__(
        cls,
        *,
        binary_path: str | Path | None = None,
        min_processes: int = 1,
        max_processes: int | None = None,
        checkout_timeout: float | None = None,
        request_timeout: float | None = None,
        max_checkouts_per_worker: int | None = None,
    ) -> Self:
        """
        Configure a worker pool; the workers are spawned by `with`.

        Arguments:
            binary_path: Path to the `monty` CLI binary. When omitted it is
                resolved from the `MONTY_BIN` environment variable, the
                environment's scripts directory (where the `pydantic-monty-cli`
                dependency installs it), or `PATH`.
            min_processes: Workers spawned eagerly and kept warm.
            max_processes: Cap on live workers (defaults to the CPU count);
                checkouts beyond it wait for a worker to be returned.
            checkout_timeout: Seconds `checkout()` waits for a free worker
                before raising `TimeoutError`. `None` waits forever.
            request_timeout: Hard per-call deadline in seconds — a worker that
                exceeds it is killed and the call raises `MontyCrashedError`
                with `timed_out=True`. Backstops the sandbox `limits`.
            max_checkouts_per_worker: Recycle a worker after this many sessions.
        """

    def __enter__(self) -> Self: ...
    def __exit__(self, *args: Any) -> None: ...
    def checkout(
        self,
        *,
        script_name: str = 'main.py',
        limits: ResourceLimits | None = None,
        type_check: bool = False,
        type_check_stubs: str | None = None,
        dataclass_registry: list[type] | None = None,
    ) -> MontySession:
        """
        Prepare a REPL session served by a dedicated worker.

        The worker is checked out of the pool by `with` on the returned
        session and returned to the pool when the `with` block exits.

        Arguments:
            script_name: Name used in tracebacks and error messages.
            limits: Resource limits enforced inside the worker.
            type_check: Type-check each fed snippet before executing it; each
                successfully executed snippet is appended to the accumulated
                context used for type-checking subsequent snippets.
            type_check_stubs: Stub declarations made available to type checking.
            dataclass_registry: Dataclass types to register for proper
                isinstance() support on output.
        """

@final
class MontySession:
    """
    A REPL session running in a dedicated `monty` subprocess worker.

    Obtained from `Monty.checkout()` and used as a context manager. Session
    state (globals, functions) persists across `feed_run` calls within the
    session.
    """

    def __enter__(self) -> Self: ...
    def __exit__(self, *args: Any) -> None: ...
    def feed_run(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        print_callback: Callable[[Literal['stdout', 'stderr'], str], None]
        | CollectStreams
        | CollectString
        | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | AbstractOS | None = None,
        skip_type_check: bool = False,
    ) -> Any:
        """
        Execute one snippet in the worker and return its result.

        Blocks the calling thread (with the GIL released) while the worker
        runs; external functions, the `os` fallback, and print callbacks are
        invoked in this process. Async external functions are not supported
        here — use `AsyncMonty`.

        Mounts are handled inside the worker process: `'overlay'` writes live
        in the worker and are discarded when the feed ends.

        Raises:
            MontyRuntimeError: The code raised an exception (session survives).
            MontyTypingError: Type checking rejected the snippet (session survives).
            MontyCrashedError: The worker process died or hit `request_timeout`;
                the session is lost but the pool replaces the worker.
        """

    def dump(self) -> bytes:
        """
        Serialize the worker's session state (idle or suspended) to opaque
        bytes using monty's existing dump format. The session stays usable.
        """

    @property
    def worker_pid(self) -> int | None:
        """OS process id of this session's worker (diagnostics/tests).

        `None` when no worker is attached or a turn is currently in flight
        on another thread (the getter never blocks on a running turn).
        """

@final
class AsyncMonty:
    """
    Async context manager owning a pool of `monty` subprocess workers.

    The async counterpart of `Monty`: worker I/O runs off the event loop, and
    external functions may be coroutines.

    ```python
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run('1 + 1')
    ```
    """

    def __new__(
        cls,
        *,
        binary_path: str | Path | None = None,
        min_processes: int = 1,
        max_processes: int | None = None,
        checkout_timeout: float | None = None,
        request_timeout: float | None = None,
        max_checkouts_per_worker: int | None = None,
    ) -> Self:
        """
        Configure a worker pool; the workers are spawned by `async with`.

        Arguments are identical to `Monty`.
        """

    async def __aenter__(self) -> Self: ...
    async def __aexit__(self, *args: Any) -> None: ...
    def checkout(
        self,
        *,
        script_name: str = 'main.py',
        limits: ResourceLimits | None = None,
        type_check: bool = False,
        type_check_stubs: str | None = None,
        dataclass_registry: list[type] | None = None,
    ) -> AsyncMontySession:
        """
        Prepare a REPL session served by a dedicated worker.

        The worker is checked out of the pool by `async with` on the returned
        session and returned to the pool when the `async with` block exits.
        Arguments are identical to `Monty.checkout`.
        """

@final
class AsyncMontySession:
    """
    A REPL session running in a dedicated `monty` subprocess worker.

    Obtained from `AsyncMonty.checkout()` and used as an async context
    manager. Session state (globals, functions) persists across
    `feed_run` calls within the session.
    """

    async def __aenter__(self) -> Self: ...
    async def __aexit__(self, *args: Any) -> None: ...
    async def feed_run(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        print_callback: Callable[[Literal['stdout', 'stderr'], str], None]
        | CollectStreams
        | CollectString
        | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | AbstractOS | None = None,
        skip_type_check: bool = False,
    ) -> Any:
        """
        Execute one snippet in the worker and return its result.

        Worker I/O runs off the event loop; external functions may be
        coroutines, awaited concurrently. See `MontySession.feed_run` for the
        shared semantics (mounts, error types).
        """

    async def dump(self) -> bytes:
        """
        Serialize the worker's session state (idle or suspended) to opaque
        bytes using monty's existing dump format. The session stays usable.
        """

    @property
    def worker_pid(self) -> int | None:
        """OS process id of this session's worker (diagnostics/tests).

        `None` when no worker is attached or a turn is currently in flight
        on another thread (the getter never blocks on a running turn).
        """
