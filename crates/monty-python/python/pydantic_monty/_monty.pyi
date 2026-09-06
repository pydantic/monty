import uuid
from collections.abc import Mapping
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
    TypeCheckFormat,
)
from .os_access import AbstractOS, OsFunction

__all__ = [
    '__version__',
    '_install_telemetry',
    'NOT_HANDLED',
    'AsyncMonty',
    'AsyncMontySession',
    'AsyncMontyWebsocket',
    'CollectStreams',
    'CollectString',
    'Frame',
    'Monty',
    'MontyClassProxy',
    'MontyClassTypeProxy',
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
    'MontyComplete',
    'FunctionSnapshot',
    'NameLookupSnapshot',
    'FutureSnapshot',
    'AsyncFunctionSnapshot',
    'AsyncNameLookupSnapshot',
    'AsyncFutureSnapshot',
]
__version__: str

# Private native hook behind pydantic_monty.instrument_telemetry.
def _install_telemetry(tracer: Any | None, meter: Any | None, logger: Any | None) -> None: ...

NOT_HANDLED = object()

@final
class CollectStreams:
    """Collect printed output as `(stream, text)` tuples.

    Defaults to a 10 MiB cap. Pass `max_bytes=None` to disable (trusted hosts).
    Exceeding the cap fails the feed with `MontyRuntimeError` wrapping a
    `MemoryError`. Not covered by `ResourceLimits.max_memory`.
    The cap includes a fixed per-entry overhead (many tiny fragments).
    """

    def __new__(cls, max_bytes: int | None = 10 * 1024 * 1024) -> CollectStreams: ...
    @property
    def output(self) -> list[tuple[Literal['stdout', 'stderr'], str]]:
        """Collected output so far."""

@final
class CollectString:
    """Collect printed output as one concatenated string.

    Defaults to a 10 MiB cap. Pass `max_bytes=None` to disable (trusted hosts).
    Exceeding the cap fails the feed with `MontyRuntimeError` wrapping a
    `MemoryError`. Not covered by `ResourceLimits.max_memory`.
    """

    def __new__(cls, max_bytes: int | None = 10 * 1024 * 1024) -> CollectString: ...
    @property
    def output(self) -> str:
        """Collected output so far."""

@final
class MountDir:
    """A mount point mapping a virtual path to a host directory.

    The directory is opened here, and every feed this mount is passed to serves
    that same directory — so build one and reuse it. `'overlay'` writes live in
    each feed's own table and are discarded when the feed ends.

    **Warning: `mode='read-write'` writes files from untrusted code to your
    real filesystem.**

    Those files are untrusted input; do not execute them. Importing counts as
    executing, and the import can be indirect: with a directory on `sys.path`
    mounted, sandboxed code can write `json.py`, or any module not yet
    imported, and the next `import` runs it. That includes imports made by
    `pydantic_monty` itself. `sys.path[0]` is the script's directory, or the
    cwd for `python -m`, `python -c` and the REPL.

    Tools also read files without an explicit import: `conftest.py`,
    `sitecustomize.py`, `.git/hooks/*`, `Makefile`, `.env`, `__pycache__`.

    The `'overlay'` default keeps writes in memory, so nothing reaches the host
    filesystem. Use `'read-write'` only with a directory that contains no code
    or config and is not on `sys.path` or any other execution path.

    ```python
    from pathlib import Path

    from pydantic_monty import Monty, MountDir

    with Monty() as pool, MountDir(host_path=Path('host-dir'), virtual_path='/data') as mount:
        with pool.checkout() as session:
            contents = session.feed_run("open('/data/notes.txt').read()", mount=mount)
    ```

    The directory is held open from construction until the mount is closed, not
    for the duration of a feed — so reusing one across feeds is free, and works
    the same inside a `with` block or outside it. `with` (or `close()`) is what
    hands the directory back; feeds passed a closed mount raise `ValueError`.
    Without it the directory stays open until the object is collected: a file
    descriptor on Unix, but on Windows that blocks renaming or deleting it.
    """

    host_path: str
    virtual_path: str
    mode: Literal['read-only', 'read-write', 'overlay']
    write_bytes_limit: int | None
    memory_usage_limit: int

    def __new__(
        cls,
        *,
        host_path: str | Path,
        virtual_path: str,
        mode: Literal['read-only', 'read-write', 'overlay'] = 'overlay',
        write_bytes_limit: int | None = None,
        memory_usage_limit: int = 100_000_000,
    ) -> MountDir:
        """Configure a mount point; validation happens here, not at feed time.

        All arguments are keyword-only: mount tools disagree on host-first
        (docker `-v`) vs virtual-first (nginx `alias`) ordering, so requiring
        names removes the ambiguity.

        Arguments:
            host_path: Real host directory to expose. Opened at construction;
                raises if it doesn't exist, isn't a directory, or cannot be
                opened — on macOS/BSD a search-only (`0o111`) directory is not
                mountable, though Linux accepts one. Sandbox code can never see
                this path or reach outside it. The mount tracks the directory
                itself rather than its name, so renaming it on the host does
                not detach the mount; on Windows the open handle prevents the
                host renaming or deleting it at all while the mount lives.
                Symlinks inside it are followed only if their targets are
                relative — an absolute target raises `PermissionError` in the
                sandbox even when it points back into the same mount.
            virtual_path: Absolute POSIX-style path prefix inside the sandbox
                (e.g. `'/data'`), regardless of host OS. Raises `TypeError`
                if not absolute.
            mode: `'read-only'` — reads only, writes raise `PermissionError`;
                `'read-write'` — writes through to the host directory, where
                the files persist after the feed (see the warning above);
                `'overlay'` (default) — reads fall through to the host, writes
                are captured in memory per feed and discarded when it ends.
            write_bytes_limit: Cap on cumulative bytes written through the
                mount within one feed; exceeding it raises `OSError` in the
                sandbox. `None` (default) means unlimited.
            memory_usage_limit: Per-mount budget in bytes (default 100 MB,
                matches DEFAULT_MEMORY_USAGE_LIMIT in rust) shared by retained
                overlay data and transient filesystem results; an operation that
                would exceed it raises `MemoryError` in the sandbox.
        """

    def close(self) -> None:
        """Release the open host directory. Idempotent.

        Feeds passed this mount afterwards raise `ValueError`; the attributes
        above keep answering. Only Windows needs this — it refuses to rename or
        delete a directory while a handle to it is open — but `MountDir` also
        works as a context manager, which closes on exit.
        """

    def __enter__(self) -> MountDir: ...
    def __exit__(self, *args: object) -> bool: ...

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
    pre-rendered as text, in the `type_check_format` chosen at checkout.

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
    Exposed to callbacks (e.g. as the first argument of an `open` result or
    a `read`/`write` request) so they can route on `path` and branch on
    `mode`/`binary`/`readable`/`writable` without re-parsing the mode string.

    Construct one from a Python `open` OS handler to return a handle back to
    Monty: `MontyFileHandle('/data/foo.txt', 'r')`. The `mode` is canonicalized
    at construction (`'rt'` → `'r'`, `'r+b'` → `'rb+'`).
    """

    def __new__(cls, path: str, mode: str, *, position: int = 0) -> MontyFileHandle:
        """Construct a `MontyFileHandle` to return from an `open` OS callback.

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
class MontyClassProxy:
    """Read-only proxy for a class instance the host has no original object for:
    a sandbox-defined instance, or a host-sent one after a session restore (the
    instance store never survives `load_session` / `load_snapshot`). It keeps the
    instance's `id`, so passed back in it resolves to the live sandbox object
    (`attributes` not applied; a freed one raises) or, after a restore, re-enters
    as a host-backed copy built from `attributes`.
    """

    @property
    def name(self) -> str:
        """Class name of the instance (e.g. `'Point'`)."""

    @property
    def id(self) -> uuid.UUID:
        """Identity of the instance, the id the sandbox resolves it by when passed back."""

    @property
    def is_dataclass(self) -> bool:
        """Whether the instance was a dataclass on the side that produced it."""

    @property
    def attributes(self) -> dict[str, Any]:
        """The instance's attributes as a plain dict."""

    def __repr__(self) -> str: ...
    def __eq__(self, value: object, /) -> bool: ...

@final
class MontyClassTypeProxy:
    """Read-only proxy for a host class the session has no class object for: a
    `ClassType`, or `type(x)` of a `ClassInstance`, returned after a session
    restore. It keeps the class `id`, so passed back in it is the same sandbox
    type object.
    """

    @property
    def name(self) -> str:
        """Name of the class (e.g. `'Point'`)."""

    @property
    def id(self) -> uuid.UUID:
        """Identity of the class, the id the sandbox resolves it by when passed back."""

    @property
    def is_dataclass(self) -> bool:
        """Whether the class is a dataclass on the side that produced it."""

    @property
    def attributes(self) -> dict[str, Any]:
        """The eager class attrs that crossed with the class, as a plain dict."""

    def __repr__(self) -> str: ...
    def __eq__(self, value: object, /) -> bool: ...

@final
class MontyCrashedError(MontyError):
    """Raised when the sandbox is gone and the session with it.

    This is the failure mode subprocess pools exist to contain: the worker is
    gone — segfault, allocator abort, external kill, `request_timeout`
    watchdog, or a fatal error it announced before exiting — but the host
    process is unharmed and the pool replaces it. A remote server also reports
    its own failure to start a worker this way. Catch this error to retry or
    report; the message says which happened.

    `exit_status` is `None` whenever the process could not be reaped, which
    includes every remote worker.

    Cannot be constructed directly from Python.
    """

    @property
    def timed_out(self) -> bool:
        """`True` when the pool's `request_timeout` watchdog killed the worker."""

    @property
    def exit_status(self) -> int | None:
        """Exit code of the dead worker when the OS reported one (signal deaths report `None`)."""

@final
class MontyDisconnectError(MontyError):
    """Raised when a remote worker's connection closed mid-session (WebSocket transport only).

    The local analogue is `MontyCrashedError`. The sandbox may have died, or
    the server may have dropped the session by policy — an idle, session, or
    turn timeout, or being over capacity. A client that only sees the
    connection go away cannot tell those apart, so this error claims no more
    than that. Retry on a fresh session.

    Cannot be constructed directly from Python.
    """

@final
class MontyShutdown(MontyError):
    """Raised when the remote server is shutting down (WebSocket transport only).

    Not an error in your code, which is why it is the one exception here
    without an `Error` suffix; it still subclasses `MontyError`. The request
    that raised it **did not run**, so re-running it on a fresh session is
    safe.

    `dump` carries the session state captured just before shutdown — restore
    it on a new session to carry the session across a server restart, with
    `session.load_session` (idle, between feeds) or `session.load_snapshot`
    (suspended mid-feed).

    One caveat: if the interrupted request was answering a suspension (an
    external function or `os` callback), the host already ran that call and
    the restored session re-announces it, so it runs again. Make such
    callbacks idempotent if you intend to restore across a shutdown.

    Cannot be constructed directly from Python.
    """

    @property
    def dump(self) -> bytes | None:
        """Restorable session dump, or `None` when nothing had run yet or the server's dump failed."""

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
            request_timeout: Per-turn parent-side deadline in seconds — a worker
                that exceeds it is killed and the call raises `MontyCrashedError`
                with `timed_out=True`. Trusted synchronous span and log callbacks
                delay enforcement while they run. Backstops sandbox `limits`.
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
        type_check_format: TypeCheckFormat | None = None,
        type_check_color: bool = False,
        assert_message_annotations: bool | int = ...,
        print_flush_interval: float | None = None,
    ) -> MontySession:
        """
        Prepare a REPL session served by a dedicated worker.

        The worker is checked out of the pool by `with` on the returned
        session and returned to the pool when the `with` block exits.

        Arguments:
            script_name: Name used in tracebacks and error messages.
            limits: Resource limits enforced inside the worker, plus `max_suspensions`,
                which the pool enforces itself.
            type_check: Type-check each fed snippet before executing it; each
                successfully executed snippet is appended to the accumulated
                context used for type-checking subsequent snippets.
            type_check_stubs: Stub declarations made available to type checking.
            type_check_format: How `MontyTypingError` diagnostics are rendered;
                `None` (the default) means `'full'`. Chosen here rather than on
                the error because the checker's structured diagnostics never
                leave the worker.
            type_check_color: Render diagnostics with ANSI colour escapes; only
                `'full'` and `'concise'` carry colour.
            assert_message_annotations: Give failed `assert` statements
                pytest-style introspected messages, e.g.
                `AssertionError: assert 2 == 5` — a deliberate divergence from
                CPython's empty `AssertionError`. On by default; set to `False`
                to restore CPython's behavior, or to an int >= 1 to customize
                the per-operand repr truncation length (default 120 bytes).
            print_flush_interval: How long, in seconds, the worker may hold
                buffered `print()` output before sending it, so that a burst
                of prints costs one callback rather than one each. `None` (the
                default) means 0.005; `0` restores line buffering, delivering
                each completed line on its own. Output is always flushed
                before a host call and before a run ends, so this only sets
                how far live output may lag — never what arrives, or in what
                order.
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
                Serviced by the pool on the host side — `'overlay'` writes
                live in the pool's per-feed mount table and are discarded when
                the feed ends.
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
                the pool's per-feed mount table and are discarded when the feed
                ends.
            os: Fallback handler for OS calls not covered by a mount, invoked
                as `(function_name, args, kwargs)`, or an `AbstractOS` instance.
                Consulted only by `resume_auto()` — `feed_start` always surfaces
                OS calls as snapshots.
            skip_type_check: Skip type checking for this feed even when the
                session was checked out with `type_check=True`.
        """

    def load_session(self, state: bytes) -> None:
        """
        Restore a session between feeds.

        This method should take data from `session.dump()` taken when no block of
        code is running (i.e. between feeds).

        Use `load_snapshot` for a dump taken mid-execution.

        The dump restores its own `script_name` /
        limits / type-check state (the `checkout()` config for those is not
        applied). The class-instance store starts empty — it is host state and
        never part of a dump, so restored `ClassInstance` values fall back to
        `MontyClassProxy` stand-ins and method calls on them fail. Raises if
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
        Restore a snapshot generated while a block of code is running (e.g.
        after `feed_start`) and return the re-announced snapshot to resume.

        Use `load_session` for a dump taken between feeds.

        Valid only on a fresh session, before any feed or load; raises
        `RuntimeError` otherwise. The dump restores its own `script_name` /
        limits / type-check state (the `checkout()` config for those is not
        applied). The class-instance store starts empty — it is host state and
        never part of a dump, so restored `ClassInstance` values fall back to
        `MontyClassProxy` stand-ins and method calls on them fail. `mount`
        re-establishes the suspended feed's mounts, which are never part of the
        dump — pass the same mounts the original feed used, or its filesystem
        calls degrade into unhandled OS calls. `'overlay'` writes made before
        the dump are not preserved (the restored overlay starts empty). Raises
        if the dump is actually an idle session.

        `external_lookup` / `os` are captured for `resume_auto()`, exactly as on
        `feed_start`. One caveat applies to a *restored* snapshot: a restored
        `FutureSnapshot`'s pending coroutines are gone (they lived in the
        previous process), so `resume_auto()` on it raises — resolve it manually
        with `resume({call_id: ...})`.
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

        Only supported by an embedded-CPython worker.
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
        type_check_format: TypeCheckFormat | None = None,
        type_check_color: bool = False,
        assert_message_annotations: bool | int = ...,
        print_flush_interval: float | None = None,
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
    WebSocket. The intended peer is `monty-server` (the production server: one
    `monty subprocess` child per connection, plus capacity/timeout policy and
    graceful drain), but any server that accepts the connection and bridges to
    a worker fits — a relay pairing it with a child that dialed in from the
    other end, or the dev relay `scripts/websocket_relay.py`.

    Like `AsyncMonty`, but instead of spawning local subprocesses each checkout
    dials the configured URL. There is no sync counterpart — remote turns are
    network-bound. `checkout()` yields the same `AsyncMontySession`.

    A `monty-server` enforces its own policy on top of the pool's. On SIGTERM
    drain it answers the session's next request with `MontyShutdown`, whose
    `dump` restores the session onto another server; every other server-side
    drop (idle, session or turn timeout, capacity) closes the connection and
    raises `MontyDisconnectError`.

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
        connect_headers: Callable[[], Mapping[str, str]] | None = None,
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
            request_timeout: Hard per-turn deadline in seconds (default 10.0) — a
                worker that exceeds it has its connection killed and the call
                raises `MontyCrashedError` with `timed_out=True`. This also
                bounds the wait when a relay accepts the connection but never
                produces a worker. Pass `None` to wait indefinitely.

                Note that `install_dependencies` is a turn too, so the default
                10.0 is often too low for it — a real `uv pip install` can exceed
                it. Raise `request_timeout` (or pass `None`) when installing
                dependencies over the WebSocket transport.
            connect_headers: Called once per session, as it is entered and before
                any wait for pool capacity, to produce extra `str` to `str`
                headers for that connection's WebSocket upgrade request — e.g. a
                token for infrastructure in front of the worker. It runs
                synchronously on the checking-out task, so it sees that task's
                contextvars and must not block. Monty never interprets the
                values; duplicate names are last-wins, also over the default
                `user-agent` and the `traceparent` the Logfire integration
                adds, and a malformed name or value raises `RuntimeError` as
                the session is entered.
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
        type_check_format: TypeCheckFormat | None = None,
        type_check_color: bool = False,
        assert_message_annotations: bool | int = ...,
        print_flush_interval: float | None = None,
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
                Serviced by the pool on the host side — `'overlay'` writes
                live in the pool's per-feed mount table and are discarded when
                the feed ends.
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
                the pool's per-feed mount table and are discarded when the feed
                ends.
            os: Fallback handler for OS calls not covered by a mount, invoked
                as `(function_name, args, kwargs)`, or an `AbstractOS` instance.
                Consulted only by `resume_auto()` — `feed_start` always surfaces
                OS calls as snapshots.
            skip_type_check: Skip type checking for this feed even when the
                session was checked out with `type_check=True`.
        """

    async def load_session(self, state: bytes) -> None:
        """Async counterpart of `MontySession.load_session`: restore a session between feeds."""

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
        Async counterpart of `MontySession.load_snapshot`.

        Restore a snapshot generated while a block of code is running (e.g.
        after `feed_start`) and return the re-announced snapshot to resume.

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
    def object_id(self) -> uuid.UUID | None:
        """Session uuid of the routed receiver — a host class instance sent
        via `ClassInstance`, or a host class sent via `ClassType` (a
        classmethod call, or construction, which arrives as `__call__`);
        `None` for plain external functions and OS calls. The receiver is not
        included in `args`."""

    @property
    def function_name(self) -> str | OsFunction: ...
    @property
    def call_id(self) -> int: ...
    @property
    def args(self) -> tuple[Any, ...]: ...
    @property
    def kwargs(self) -> dict[str, Any]: ...
    def resume(self, result: ExternalResult) -> SyncSnapshot:
        """Resume with the call's result; resumes at most once.

        Answers only this call: the result is passed straight through, and
        neither the feed's mounts nor the captured `os=` are consulted. Use
        `resume_auto()` for those.
        """

    def resume_not_handled(self) -> SyncSnapshot:
        """Resume an OS-call snapshot with monty's default unhandled behaviour."""

    def resume_auto(self) -> SyncSnapshot:
        """Answer this call automatically, then return the next snapshot (or
        `MontyComplete`). Resumes at most once.

        An OS call is offered to the feed's mounts first, falling back to the
        `os=` captured at `feed_start` / `load_snapshot` and then to monty's
        unhandled default. An external call is resolved through
        `external_lookup=`; a name absent from it makes the sandbox raise
        `NameError` (as in `feed_run`). A coroutine external raises
        `RuntimeError` — use `AsyncMonty` for async externals."""

    def dump(self) -> bytes:
        """Serialize the suspended worker; restore via `MontySession.load_snapshot`."""

    def __repr__(self) -> str: ...

@final
class NameLookupSnapshot:
    """A paused execution waiting for the value of an undefined name, or —
    when `object_id` is set — a lazy attribute lookup on a host-backed object
    (a `ClassInstance` instance, or a `ClassType` class)."""

    @property
    def script_name(self) -> str: ...
    @property
    def variable_name(self) -> str: ...
    @property
    def object_id(self) -> uuid.UUID | None:
        """Session uuid of the receiver for a lazy attribute lookup; `None`
        for a plain undefined-name lookup. An omitted-`value` resume raises
        `AttributeError` (not `NameError`) for attribute lookups."""
    def resume(self, *, value: Any = ...) -> SyncSnapshot:
        """Resume by binding the name to `value` (any value, including `None`), or
        omit `value` to leave the name undefined — the sandbox then raises
        `NameError`, or `AttributeError` when `object_id` is set (a lazy
        attribute lookup on a host-backed object)."""

    def resume_auto(self) -> SyncSnapshot:
        """Answer this name lookup automatically, then return the next snapshot
        (or `MontyComplete`). A plain lookup resolves from the captured
        `external_lookup=` (an absent name raises `NameError` in the sandbox);
        an `object_id` lookup resolves through the sending wrapper's
        `lazy_attrs` policy (a denied or absent attribute raises
        `AttributeError`; any other exception raised while serving it, or a
        value that cannot be converted, is raised inside the sandbox)."""

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
    def resume(self, results: dict[int, ExternalSettledResult]) -> SyncSnapshot:
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
    def object_id(self) -> uuid.UUID | None:
        """As `FunctionSnapshot.object_id`: the routed receiver's session uuid."""

    @property
    def function_name(self) -> str | OsFunction: ...
    @property
    def call_id(self) -> int: ...
    @property
    def args(self) -> tuple[Any, ...]: ...
    @property
    def kwargs(self) -> dict[str, Any]: ...
    async def resume(self, result: ExternalResult) -> AsyncSnapshot: ...
    async def resume_not_handled(self) -> AsyncSnapshot: ...
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
    @property
    def object_id(self) -> uuid.UUID | None:
        """As `NameLookupSnapshot.object_id`: the host object a lazy attribute is read from."""

    async def resume(self, *, value: Any = ...) -> AsyncSnapshot: ...
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
    async def resume(self, results: dict[int, ExternalSettledResult]) -> AsyncSnapshot: ...
    async def resume_auto(self) -> AsyncSnapshot:
        """Wait for one or more coroutine externals spawned by earlier
        `resume_auto` calls to settle, deliver them, and return the next
        snapshot. Raises if there are no pending coroutines to await (e.g. a
        snapshot restored via `load_snapshot`)."""

    def dump(self) -> bytes: ...
    def __repr__(self) -> str: ...
