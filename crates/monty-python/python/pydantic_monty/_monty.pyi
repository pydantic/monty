from pathlib import Path
from typing import Any, Callable, Literal, NoReturn, final

from typing_extensions import Self

from . import (
    AsyncSnapshot,
    ExternalResult,
    ExternalSettledResult,
    OsHandler,
    PrintCallback,
    ResourceLimits,
    SyncSnapshot,
)
from .os_access import AbstractOS, OsFunction

__all__ = [
    '__version__',
    'NOT_HANDLED',
    'AsyncMonty',
    'AsyncMontySession',
    'AsyncMontyWebsocket',
    'CollectStreams',
    'CollectString',
    'Frame',
    'Monty',
    'MontyConversionError',
    'MontyCrashedError',
    'MontyError',
    'MontyFileHandle',
    'MontySession',
    'MontySyntaxError',
    'MontyRuntimeError',
    'MontyTypingError',
    'MountDir',
    'MontyComplete',
    'FunctionSnapshot',
    'NameLookupSnapshot',
    'FutureSnapshot',
    'AsyncFunctionSnapshot',
    'AsyncNameLookupSnapshot',
    'AsyncFutureSnapshot',
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
class MontyConversionError(MontyError):
    """Raised when a host value cannot be converted across the Monty/host boundary.

    A value Monty cannot represent — an `external_lookup` entry or an `inputs`
    value of an unsupported type — rejects the feed with this error rather than
    crossing into the sandbox. Inherits `exception()` (a native `TypeError`) and
    `__str__()` (the conversion message) from `MontyError`.
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
                environment's scripts directory (where the `pydantic-monty-runtime`
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
        external_lookup: dict[str, Any] | None = None,
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

        Arguments:
            code: The Python snippet to execute; its trailing expression value
                (if any) is converted to a Python object and returned.
            inputs: Values eagerly bound as globals before the snippet runs —
                every entry is converted and bound once, whether or not it is
                referenced.
            external_lookup: Host values resolving names the snippet leaves
                undefined, lazily and on demand: a callable entry becomes a host
                function the sandbox can call, any other value is converted and
                returned directly when the name is read, and an absent name
                raises `NameError`. The lazy counterpart to `inputs`; a name
                present in both is served by the eager `inputs` binding.
            print_callback: Receives the sandbox's `print()` output as
                `(stream, text)`, or a `CollectStreams` / `CollectString`
                collector. Defaults to the host process stdout/stderr.
            mount: Host directories mounted into the sandbox for this feed.
                Handled inside the worker — `'overlay'` writes live in the
                worker and are discarded when the feed ends.
            os: Fallback handler for OS calls (e.g. filesystem access) not
                covered by a mount, invoked as `(function_name, args, kwargs)`,
                or an `AbstractOS` instance.
            skip_type_check: Skip type checking for this feed even when the
                session was checked out with `type_check=True`.

        Raises:
            MontyRuntimeError: The code raised an exception (session survives).
            MontyTypingError: Type checking rejected the snippet (session survives).
            MontyCrashedError: The worker process died or hit `request_timeout`;
                the session is lost but the pool replaces the worker.
        """

    def feed_start(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_lookup: dict[str, Any] | None = None,
        print_callback: PrintCallback | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: OsHandler | None = None,
        skip_type_check: bool = False,
    ) -> SyncSnapshot:
        """
        Start a snippet and return a snapshot at each external call, OS call,
        name lookup, or future resolution instead of driving to completion.

        Answer the snapshot with `snapshot.resume(...)`, which returns the next
        snapshot or a `MontyComplete`. Alternatively, supply `external_lookup`
        (and/or `os`) and drive the whole snippet with `snapshot.resume_auto()`,
        which answers each suspension from them automatically:

        ```python
        snapshot = session.feed_start(code, external_lookup={'fetch': fetch})
        while not isinstance(snapshot, MontyComplete):
            snapshot = snapshot.resume_auto()
        ```

        Unlike `feed_run`, `external_lookup` is *not* consulted during this
        initial drive — external calls and name lookups are still surfaced as
        snapshots; it is only captured for later `resume_auto()` calls.

        Use `snapshot.dump()` to checkpoint the worker mid-execution and
        `load_snapshot` to restore it.

        Arguments:
            code: The Python snippet to execute; its trailing expression value
                (if any) is the `MontyComplete.output` when the feed completes.
            inputs: Values eagerly bound as globals before the snippet runs —
                every entry is converted and bound once, whether or not it is
                referenced.
            external_lookup: Host functions and values, by name, that
                `resume_auto()` resolves external calls and undefined names
                against (as in `feed_run`). Captured for `resume_auto()`; not
                used by a plain `resume(...)`.
            print_callback: Receives the sandbox's `print()` output as
                `(stream, text)`, or a `CollectStreams` / `CollectString`
                collector. Defaults to the host process stdout/stderr.
            mount: Host directories mounted into the sandbox for the whole feed
                (there is no `mount=` on `resume`). `'overlay'` writes live in
                the worker and are discarded when the feed ends.
            os: Fallback handler for OS calls not covered by a mount, invoked as
                `(function_name, args, kwargs)`, or an `AbstractOS` instance. It
                auto-dispatches uncovered OS calls until the next non-OS event;
                omit it to surface OS calls as snapshots instead.
            skip_type_check: Skip type checking for this feed even when the
                session was checked out with `type_check=True`.
        """

    def load(self, state: bytes) -> None:
        """
        Restore a dumped **idle** session — bytes from `session.dump()` taken
        between feeds — so you can keep feeding it. Use `load_snapshot` for a
        dump taken mid-execution.

        Valid only on a fresh session, before any feed or load; raises
        `RuntimeError` otherwise. The dump restores its own `script_name` /
        limits / type-check state (the `checkout()` config for those is not
        applied); the dataclass registry from `checkout()` is reused. Raises if
        the dump is actually a suspended snapshot.
        """

    def load_snapshot(
        self,
        state: bytes,
        *,
        mount: MountDir | list[MountDir] | None = None,
        print_callback: PrintCallback | None = None,
        external_lookup: dict[str, Any] | None = None,
        os: OsHandler | None = None,
    ) -> SyncSnapshot:
        """
        Restore a dumped **suspended** snapshot — bytes from `feed_start` +
        `snapshot.dump()` — and return the re-announced snapshot to resume. Use
        `load` for a dump taken between feeds.

        Valid only on a fresh session, before any feed or load; raises
        `RuntimeError` otherwise. The dump restores its own `script_name` /
        limits / type-check state (the `checkout()` config for those is not
        applied); the dataclass registry from `checkout()` is reused. `mount`
        re-establishes the suspended feed's mounts (whose host paths are not in
        the dump), validated against the dump's recorded requirements — a
        missing, extra, or altered mount raises. `'overlay'` writes made before
        the dump are not preserved (the restored overlay starts empty). Raises
        if the dump is actually an idle session.

        `external_lookup` / `os` are captured for `resume_auto()`, exactly as on
        `feed_start`. Two caveats apply to a *restored* snapshot: a restored
        `FutureSnapshot`'s pending coroutines are gone (they lived in the
        previous process), so `resume_auto()` on it raises — resolve it manually
        with `resume({call_id: ...})`; and a re-announced OS-call snapshot
        carries only its `not_handled_error`, not the original `args`/`kwargs`
        (those were consumed before the dump), so prefer a manual `resume` /
        `resume_not_handled` there.
        """

    def dump(self) -> bytes:
        """
        Serialize the worker's session state (idle or suspended) to opaque
        bytes using monty's existing dump format. The session stays usable.
        """

    def install_dependencies(self, requirements: list[str]) -> None:
        """
        Install third-party Python packages into the session, making them
        importable by subsequent `feed_run` calls. Session-scoped and
        repeatable; an empty list is a no-op.

        Only supported by an embedded-CPython worker (e.g. `monty-cpython`).
        Against the pure-Monty sandbox worker, or on a `uv` install failure
        (the error carries uv's stderr), raises `MontyRuntimeError`; the
        session stays usable. Bounded by the pool's `request_timeout`, so raise
        it for large dependency sets.

        Requirements are PEP 508 strings, e.g. `["httpx>=0.27", "numpy"]`.
        Dependencies a script declares inline via PEP 723 (`# /// script`) are
        installed automatically on `feed_run` and need no call here.
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
class AsyncMontyWebsocket:
    """
    Async context manager owning a pool of remote `monty` workers reached over a
    WebSocket. The dialed peer is the server side — a relay that pairs this
    connection with a child (such as `monty-cpython websocket`, which dials the
    relay from the other end), or any server that accepts the connection and
    bridges to a worker.

    Like `AsyncMonty`, but instead of spawning local subprocesses each checkout
    dials the configured URL. There is no sync counterpart — remote turns are
    network-bound. `checkout()` yields the same `AsyncMontySession`.

    ```python
    async with AsyncMontyWebsocket('ws://127.0.0.1:8799') as pool:
        async with pool.checkout() as session:
            result = await session.feed_run('1 + 1')
    ```
    """

    def __new__(
        cls,
        url: str,
        *,
        max_processes: int | None = None,
        checkout_timeout: float | None = None,
        request_timeout: float | None = 10.0,
    ) -> Self:
        """
        Configure a remote worker pool; connections are made by `async with` and
        each checkout (no workers are pre-warmed).

        Arguments:
            url: `ws://`/`wss://` URL to dial — a relay, or any server that
                bridges to a worker. Dialed verbatim; any session/rendezvous routing the URL
                needs (e.g. a `/<uuid>/parent` path for a relay) must already be
                in it.
            max_processes: Cap on concurrent connections (defaults to the CPU
                count); checkouts beyond it wait.
            checkout_timeout: Seconds `checkout()` waits for capacity before
                raising `TimeoutError`. `None` waits forever.
            request_timeout: Hard per-call deadline in seconds (default 10.0) — a
                worker that exceeds it has its connection killed and the call
                raises `MontyCrashedError` with `timed_out=True`. This also
                bounds the wait when a relay accepts the connection but never
                produces a worker. Pass `None` to wait indefinitely.

                Note that `install_dependencies` is a turn too, so the default
                10.0 is often too low for it — a real `uv pip install` can exceed
                it. Raise `request_timeout` (or pass `None`) when installing
                dependencies over the WebSocket transport.
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
        Prepare a REPL session served by a dedicated remote connection.

        Identical to `AsyncMonty.checkout`; the connection is opened by
        `async with` on the returned session.
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
        external_lookup: dict[str, Any] | None = None,
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

        Worker I/O runs off the event loop; external functions (the callable
        entries in `external_lookup`) may be coroutines, awaited concurrently.
        See `MontySession.feed_run` for the shared error types.

        Arguments:
            code: The Python snippet to execute; its trailing expression value
                (if any) is converted to a Python object and returned.
            inputs: Values eagerly bound as globals before the snippet runs —
                every entry is converted and bound once, whether or not it is
                referenced.
            external_lookup: Host values resolving names the snippet leaves
                undefined, lazily and on demand: a callable entry (sync or a
                coroutine function) becomes a host function the sandbox can call,
                any other value is converted and returned directly when the name
                is read, and an absent name raises `NameError`. The lazy
                counterpart to `inputs`; a name present in both is served by the
                eager `inputs` binding.
            print_callback: Receives the sandbox's `print()` output as
                `(stream, text)`, or a `CollectStreams` / `CollectString`
                collector. Defaults to the host process stdout/stderr.
            mount: Host directories mounted into the sandbox for this feed.
                Handled inside the worker — `'overlay'` writes live in the
                worker and are discarded when the feed ends.
            os: Fallback handler for OS calls (e.g. filesystem access) not
                covered by a mount, invoked as `(function_name, args, kwargs)`,
                or an `AbstractOS` instance.
            skip_type_check: Skip type checking for this feed even when the
                session was checked out with `type_check=True`.
        """

    async def feed_start(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_lookup: dict[str, Any] | None = None,
        print_callback: PrintCallback | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: OsHandler | None = None,
        skip_type_check: bool = False,
    ) -> AsyncSnapshot:
        """
        Async counterpart of `MontySession.feed_start`: resolves to a snapshot
        (whose `resume(...)` / `resume_auto()` is awaitable) or a
        `MontyComplete`.

        As in the sync version, `external_lookup` (and `os`) are captured for
        `await snapshot.resume_auto()` rather than consulted during this initial
        drive. A coroutine external answered by `resume_auto()` is awaited
        concurrently: it yields an `AsyncFutureSnapshot` whose `resume_auto()`
        settles the pending coroutines.

        Arguments:
            code: The Python snippet to execute; its trailing expression value
                (if any) is the `MontyComplete.output` when the feed completes.
            inputs: Values eagerly bound as globals before the snippet runs —
                every entry is converted and bound once, whether or not it is
                referenced.
            external_lookup: Host functions and values, by name, that
                `resume_auto()` resolves external calls and undefined names
                against (as in `feed_run`). Callables may be coroutine
                functions. Captured for `resume_auto()`; not used by a plain
                `resume(...)`.
            print_callback: Receives the sandbox's `print()` output as
                `(stream, text)`, or a `CollectStreams` / `CollectString`
                collector. Defaults to the host process stdout/stderr.
            mount: Host directories mounted into the sandbox for the whole feed
                (there is no `mount=` on `resume`). `'overlay'` writes live in
                the worker and are discarded when the feed ends.
            os: Fallback handler for OS calls not covered by a mount, invoked as
                `(function_name, args, kwargs)`, or an `AbstractOS` instance. It
                auto-dispatches uncovered OS calls until the next non-OS event;
                omit it to surface OS calls as snapshots instead. Also captured
                for `resume_auto()`.
            skip_type_check: Skip type checking for this feed even when the
                session was checked out with `type_check=True`.
        """

    async def load(self, state: bytes) -> None:
        """
        Async counterpart of `MontySession.load`: restores a dumped idle
        session. Valid only on a fresh session; raises if the dump is actually a
        suspended snapshot.
        """

    async def load_snapshot(
        self,
        state: bytes,
        *,
        mount: MountDir | list[MountDir] | None = None,
        print_callback: PrintCallback | None = None,
        external_lookup: dict[str, Any] | None = None,
        os: OsHandler | None = None,
    ) -> AsyncSnapshot:
        """
        Async counterpart of `MontySession.load_snapshot`: restores a dumped
        suspended snapshot and resolves to it (whose `resume(...)` /
        `resume_auto()` is awaitable). Valid only on a fresh session; raises if
        the dump is actually an idle session.

        `external_lookup` / `os` are captured for `resume_auto()`, with the same
        restored-snapshot caveats as the sync method (a restored `FutureSnapshot`
        cannot be driven with `resume_auto()` — its pending coroutines are gone).
        """

    async def dump(self) -> bytes:
        """
        Serialize the worker's session state (idle or suspended) to opaque
        bytes using monty's existing dump format. The session stays usable.
        """

    async def install_dependencies(self, requirements: list[str]) -> None:
        """
        Async counterpart of `MontySession.install_dependencies`: install
        third-party packages into the session (off the event loop) so later
        `feed_run` calls can import them. Session-scoped and repeatable; an
        empty list is a no-op.

        Only supported by an embedded-CPython worker. Against the pure-Monty
        sandbox worker, or on a `uv` install failure, raises
        `MontyRuntimeError`; the session stays usable. PEP 723 inline
        dependencies are installed automatically on `feed_run`.
        """

    @property
    def worker_pid(self) -> int | None:
        """OS process id of this session's worker (diagnostics/tests).

        `None` when no worker is attached or a turn is currently in flight
        on another thread (the getter never blocks on a running turn).
        """

@final
class MontyComplete:
    """The result of a completed `feed_start` execution."""

    @property
    def output(self) -> Any:
        """The final value, converted to a Python object on each access."""

    def __repr__(self) -> str: ...

@final
class FunctionSnapshot:
    """A paused execution waiting for an external function or OS call result.

    For OS calls `is_os_function` is `True` and `function_name` is the
    `OsFunction` name; resume with a value, an exception, or
    `resume_not_handled()`.
    """

    @property
    def script_name(self) -> str: ...
    @property
    def is_os_function(self) -> bool: ...
    @property
    def is_method_call(self) -> bool:
        """Whether this is a dataclass method call (the instance is `args[0]`)."""

    @property
    def function_name(self) -> str | OsFunction: ...
    @property
    def call_id(self) -> int: ...
    @property
    def args(self) -> tuple[Any, ...]: ...
    @property
    def kwargs(self) -> dict[str, Any]: ...
    def resume(
        self,
        result: ExternalResult,
        *,
        os: OsHandler | None = None,
    ) -> SyncSnapshot:
        """Resume with the call's result; resumes at most once.

        Mounts are fixed when the feed starts, so there is no `mount=` here. An
        `os=` handler auto-dispatches OS calls produced by the continuation
        until the next non-OS event.
        """

    def resume_not_handled(self, *, os: OsHandler | None = None) -> SyncSnapshot:
        """Resume an OS-call snapshot with monty's default unhandled behaviour."""

    def resume_auto(self) -> SyncSnapshot:
        """Answer this call automatically from the `external_lookup=` / `os=`
        captured at `feed_start` / `load_snapshot`, then return the next snapshot
        (or `MontyComplete`). Resumes at most once.

        A function name absent from `external_lookup` makes the sandbox raise
        `NameError` (as in `feed_run`). A coroutine external raises `RuntimeError`
        — use `AsyncMonty` for async externals."""

    def dump(self) -> bytes:
        """Serialize the suspended worker; restore via `MontySession.load_snapshot`."""

    def __repr__(self) -> str: ...

@final
class NameLookupSnapshot:
    """A paused execution waiting for the value of an undefined name."""

    @property
    def script_name(self) -> str: ...
    @property
    def variable_name(self) -> str: ...
    def resume(
        self,
        *,
        value: Any = ...,
        os: OsHandler | None = None,
    ) -> SyncSnapshot:
        """Resume by binding the name to `value` (any value, including `None`), or
        omit `value` to leave the name undefined and raise `NameError`."""

    def resume_auto(self) -> SyncSnapshot:
        """Answer this name lookup automatically from the captured
        `external_lookup=`, then return the next snapshot (or `MontyComplete`). A
        name absent from the lookup makes the sandbox raise `NameError`."""

    def dump(self) -> bytes:
        """Serialize the suspended worker; restore via `MontySession.load_snapshot`."""

    def __repr__(self) -> str: ...

@final
class FutureSnapshot:
    """A paused execution where every sandbox task is blocked on external futures."""

    @property
    def script_name(self) -> str: ...
    @property
    def pending_call_ids(self) -> list[int]: ...
    def resume(
        self,
        results: dict[int, ExternalSettledResult],
        *,
        os: OsHandler | None = None,
    ) -> SyncSnapshot:
        """Resume with settled results for one or more pending futures (by
        `call_id`); a future cannot resolve to another `future`."""

    def resume_auto(self) -> NoReturn:
        """Always raises `RuntimeError`: a sync session cannot drive coroutine
        externals. Resolve the pending futures manually with `resume({...})`, or
        use `AsyncMonty`. Does not consume the snapshot."""

    def dump(self) -> bytes:
        """Serialize the suspended worker; restore via `MontySession.load_snapshot`."""

    def __repr__(self) -> str: ...

@final
class AsyncFunctionSnapshot:
    """Async sibling of `FunctionSnapshot`; `resume`/`resume_not_handled` are awaitable."""

    @property
    def script_name(self) -> str: ...
    @property
    def is_os_function(self) -> bool: ...
    @property
    def is_method_call(self) -> bool: ...
    @property
    def function_name(self) -> str | OsFunction: ...
    @property
    def call_id(self) -> int: ...
    @property
    def args(self) -> tuple[Any, ...]: ...
    @property
    def kwargs(self) -> dict[str, Any]: ...
    async def resume(
        self,
        result: ExternalResult,
        *,
        os: OsHandler | None = None,
    ) -> AsyncSnapshot: ...
    async def resume_not_handled(self, *, os: OsHandler | None = None) -> AsyncSnapshot: ...
    async def resume_auto(self) -> AsyncSnapshot:
        """Async sibling of `FunctionSnapshot.resume_auto`. A coroutine external
        is spawned and answered with a pending future, so other sandbox tasks
        keep running; it is later settled by `AsyncFutureSnapshot.resume_auto`."""

    def dump(self) -> bytes: ...
    def __repr__(self) -> str: ...

@final
class AsyncNameLookupSnapshot:
    """Async sibling of `NameLookupSnapshot`."""

    @property
    def script_name(self) -> str: ...
    @property
    def variable_name(self) -> str: ...
    async def resume(
        self,
        *,
        value: Any = ...,
        os: OsHandler | None = None,
    ) -> AsyncSnapshot: ...
    async def resume_auto(self) -> AsyncSnapshot:
        """Async sibling of `NameLookupSnapshot.resume_auto`."""

    def dump(self) -> bytes: ...
    def __repr__(self) -> str: ...

@final
class AsyncFutureSnapshot:
    """Async sibling of `FutureSnapshot`."""

    @property
    def script_name(self) -> str: ...
    @property
    def pending_call_ids(self) -> list[int]: ...
    async def resume(
        self,
        results: dict[int, ExternalSettledResult],
        *,
        os: OsHandler | None = None,
    ) -> AsyncSnapshot: ...
    async def resume_auto(self) -> AsyncSnapshot:
        """Wait for one or more coroutine externals spawned by earlier
        `resume_auto` calls to settle, deliver them, and return the next
        snapshot. Raises if there are no pending coroutines to await (e.g. a
        snapshot restored via `load_snapshot`)."""

    def dump(self) -> bytes: ...
    def __repr__(self) -> str: ...
