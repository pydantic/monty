from collections.abc import Coroutine
from pathlib import Path
from typing import Any, Callable, Literal, final

from typing_extensions import Self

from . import ExternalResult, ResourceLimits
from .os_access import AbstractOS, OsFunction

__all__ = [
    '__version__',
    'NOT_HANDLED',
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
    'MountDir',
    'Frame',
    'load_snapshot',
    'load_repl_snapshot',
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

@final
class Monty:
    """
    A sandboxed Python interpreter instance.

    Parses and compiles Python code on initialization, then can be run
    multiple times with different input values. This separates the parsing
    cost from execution, making repeated runs more efficient.
    """

    def __new__(
        cls,
        code: str,
        *,
        script_name: str = 'main.py',
        inputs: list[str] | None = None,
        type_check: bool = False,
        type_check_stubs: str | None = None,
        dataclass_registry: list[type] | None = None,
    ) -> Self:
        """
        Create a new Monty interpreter by parsing the given code.

        Arguments:
            code: Python code to execute
            script_name: Name used in tracebacks and error messages
            inputs: List of input variable names available in the code
            type_check: Whether to perform type checking on the code (default: True)
            type_check_stubs: Optional code to prepend before type checking,
                e.g. with input variable declarations or external function signatures
            dataclass_registry: Optional list of dataclass types to register for proper
                isinstance() support on output, see `register_dataclass()` above.

        Raises:
            MontySyntaxError: If the code cannot be parsed
            MontyTypingError: If type_check is True and type errors are found
        """

    @staticmethod
    def acreate(
        code: str,
        *,
        script_name: str = 'main.py',
        inputs: list[str] | None = None,
        type_check: bool = False,
        type_check_stubs: str | None = None,
        dataclass_registry: list[type] | None = None,
    ) -> Coroutine[Any, Any, Monty]:
        """
        Async alternative constructor that parses and (optionally) type-checks
        the code on a worker thread, returning a coroutine that resolves to a
        new `Monty` instance.

        Use this from `async def` callers when the source might be large or
        when type-checking is enabled, so the build does not block the event
        loop. Arguments and exceptions match `__new__`.

        Raises:
            MontySyntaxError: If the code cannot be parsed
            MontyTypingError: If type_check is True and type errors are found
        """

    def type_check(self, type_check_stubs: str | None = None) -> None:
        """
        Perform static type checking on the code.

        Analyzes the code for type errors without executing it. This uses
        a subset of Python's type system supported by Monty.

        Arguments:
            type_check_stubs: Optional code to prepend before type checking,
                e.g. with input variable declarations or external function signatures.

        Raises:
            MontyTypingError: If type errors are found. Use `.display(format, color)`
                on the exception to render the diagnostics in different formats.
            RuntimeError: If the type checking infrastructure fails internally.
        """

    def run(
        self,
        *,
        inputs: dict[str, Any] | None = None,
        limits: ResourceLimits | None = None,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
    ) -> Any:
        """
        Execute the code and return the result.

        The GIL is released allowing parallel execution.

        Arguments:
            inputs: Dict of input variable values (must match names from __init__)
            limits: Optional resource limits configuration
            external_functions: Dict of external function callbacks
            print_callback: `None` (write to stdout/stderr), a callable `(stream, text) -> None`,
                `CollectStreams()`, or `CollectString()`.
            mount: Optional filesystem mount(s) to expose inside the sandbox.
            os: Optional callback for OS calls.
                Called with (function_name, args) where function_name is like 'Path.exists'
                and args is a tuple of arguments. Must return the appropriate value for the
                OS function (e.g., bool for exists(), stat_result for stat()).

        Returns:
            The result of the last expression in the code.

        Raises:
            MontyRuntimeError: If the code raises an exception during execution.
        """

    def start(
        self,
        *,
        inputs: dict[str, Any] | None = None,
        limits: ResourceLimits | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
    ) -> FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete:
        """
        Start the code execution and return a progress object, or completion.

        This allows you to iteratively run code and parse/resume whenever an external function is called.

        The GIL is released allowing parallel execution.

        When `mount` or `os` is provided, OS calls are resolved automatically using
        the same logic as `run()` (mount table first, then the `os` callback),
        this method only returns a snapshot when a non-OS event is reached
        (external function, name lookup, future, or completion).

        Auto-dispatch does NOT persist across subsequent `snapshot.resume()` calls —
        OS calls produced after the first resume surface as a `FunctionSnapshot`
        with `is_os_function=True`, as before.

        Arguments:
            inputs: Dict of input variable values (must match names from __init__)
            limits: Optional resource limits configuration
            print_callback: Optional callback for print output
            mount: Optional filesystem mount(s) to expose inside the sandbox.
            os: Optional callback for OS calls. Called with (function_name, args, kwargs)
                and must return the appropriate value for the OS function. Return
                `NOT_HANDLED` to fall back to Monty's default unhandled behavior.

        Returns:
            FunctionSnapshot if an external function call is pending,
            NameLookupSnapshot if more futures need to be resolved,
            FutureSnapshot if futures need to be resolved,
            MontyComplete if execution finished without external calls.

        Raises:
            MontyRuntimeError: If the code raises an exception during execution
        """

    def run_async(
        self,
        *,
        inputs: dict[str, Any] | None = None,
        limits: ResourceLimits | None = None,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
        os: AbstractOS | None = None,
    ) -> Coroutine[Any, Any, Any]:
        """
        Execute the code with support for async external functions.

        VM resume calls are offloaded to a new thread to avoid blocking the event loop.
        External functions that return coroutines are awaited on the Python event loop.

        Arguments:
            inputs: Dict of input variable values (must match names from __init__)
            limits: Optional resource limits configuration
            external_functions: Dict of external function callbacks (sync or async)
            print_callback: `None` (stdout), a callable `(stream, text) -> None`,
                `CollectStreams()`, or `CollectString()`.
            os: Optional OS access handler for filesystem operations

        Returns:
            A coroutine that resolves to the result of the last expression.

        Raises:
            MontyRuntimeError: If the code raises an exception during execution.
        """

    def dump(self) -> bytes:
        """
        Serialize the Monty instance to a binary format.

        The serialized data can be stored and later restored with `Monty.load()`.
        This allows caching parsed code to avoid re-parsing on subsequent runs.

        Returns:
            Bytes containing the serialized Monty instance.

        Raises:
            ValueError: If serialization fails.
        """

    @staticmethod
    def load(
        data: bytes,
        *,
        dataclass_registry: list[type] | None = None,
    ) -> Monty:
        """
        Deserialize a Monty instance from binary format.

        Arguments:
            data: The serialized Monty data from `dump()`
            dataclass_registry: Optional list of dataclass types to register for proper
                isinstance() support on output, see `register_dataclass()` above.

        Returns:
            A new Monty instance.

        Raises:
            ValueError: If deserialization fails.
        """

    def register_dataclass(self, cls: type) -> None:
        """
        Register a dataclass type for proper isinstance() support on output.

        When a dataclass passes through Monty and is returned, it normally becomes
        an `UnknownDataclass`. By registering the original type, we can use it to
        instantiate a real instance of that dataclass.

        Arguments:
            cls: The dataclass type to register.

        Raises:
            TypeError: If the argument is not a dataclass type.
        """

    def __repr__(self) -> str: ...

@final
class MontyRepl:
    """
    Incremental no-replay REPL session.

    Create with `MontyRepl()` then call `feed_run()` to execute snippets
    incrementally against persistent heap and namespace state.
    """

    def __new__(
        cls,
        *,
        script_name: str = 'main.py',
        limits: ResourceLimits | None = None,
        type_check: bool = False,
        type_check_stubs: str | None = None,
        dataclass_registry: list[type] | None = None,
    ) -> Self:
        """
        Create an empty REPL session ready to receive snippets via `feed_run()`.

        No code is parsed or executed at construction time.

        Arguments:
            script_name: Name used in tracebacks and error messages
            limits: Optional resource limits configuration
            type_check: Whether to type-check each snippet before execution.
                When enabled, each `feed_run`/`feed_run_async`/`feed_start` call
                runs static type checking before executing the code, and each
                successfully executed snippet is appended to the accumulated
                context used for type-checking subsequent snippets.
            type_check_stubs: Optional stub code providing type declarations for
                variables and functions available in the REPL, e.g. input variable
                types or external function signatures.
            dataclass_registry: Optional list of dataclass types to register for proper
                isinstance() support on output.
        """

    @property
    def script_name(self) -> str:
        """The name of the script being executed."""

    def register_dataclass(self, cls: type) -> None:
        """
        Register a dataclass type for proper isinstance() support on output.
        """

    def type_check(self, code: str, type_check_stubs: str | None = None) -> None:
        """
        Perform static type checking on the given code snippet.

        Checks the snippet in isolation using `type_check_stubs` as stub context.
        This does not use the accumulated code from previous `feed_run` calls —
        use `type_check_stubs` to provide any needed declarations.

        Arguments:
            code: The code to type check
            type_check_stubs: Optional code to prepend before type checking,
                e.g. with input variable declarations or external function signatures

        Raises:
            RuntimeError: If type checking infrastructure fails
            MontyTypingError: If type errors are found
        """

    def feed_run(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
        skip_type_check: bool = False,
    ) -> Any:
        """
        Execute one incremental snippet and return its output.

        Arguments:
            code: The Python code snippet to execute
            inputs: Dict of input values injected into the REPL namespace
                before executing the snippet
            external_functions: Dict of external function callbacks. When
                provided, external function calls and name lookups are
                dispatched to the provided callables — matching the behavior
                of `Monty.run(external_functions=...)`.
            print_callback: Optional callback for print output
            mount: Optional filesystem mount(s) to expose inside the sandbox
            os: Optional OS access handler for filesystem operations
            skip_type_check: When `True`, static type checking is bypassed for
                this snippet AND the snippet is NOT appended to the accumulated
                type-check context, so later type-checked snippets will not see
                any names it defined. Has no effect unless `type_check=True`
                was set on the REPL.
        """

    def feed_run_async(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
        os: AbstractOS | None = None,
        skip_type_check: bool = False,
    ) -> Coroutine[Any, Any, Any]:
        """
        Execute one incremental snippet and return its output with support for async external functions.

        VM resume calls are offloaded to a new thread to avoid blocking the event loop.
        External functions that return coroutines are awaited on the Python event loop.

        Arguments:
            code: The Python code snippet to execute
            inputs: Dict of input values to inject into the REPL namespace
            external_functions: Dict of external function callbacks (sync or async)
            print_callback: Optional callback for print output
            os: Optional OS access handler for filesystem operations
            skip_type_check: When `True`, static type checking is bypassed for
                this snippet AND the snippet is NOT appended to the accumulated
                type-check context, so later type-checked snippets will not see
                any names it defined. Has no effect unless `type_check=True`
                was set on the REPL.

        Returns:
            A coroutine that resolves to the output of the snippet

        Raises:
            MontyRuntimeError: If the code raises an exception during execution
        """

    def feed_start(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
        skip_type_check: bool = False,
    ) -> FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete:
        """
        Start executing an incremental snippet, yielding snapshots for external calls.

        Unlike `feed_run()`, which handles external function dispatch internally,
        `feed_start()` returns a snapshot object whenever the code needs an external
        function call, OS call, name lookup, or future resolution. The caller provides
        the result via `snapshot.resume(...)`, which returns the next snapshot or
        `MontyComplete`.

        This enables the same iterative start/resume pattern used by `Monty.start()`,
        including support for async external functions via `FutureSnapshot`.

        When `mount` or `os` is provided, OS calls are resolved automatically using
        the same logic as `feed_run()`, and this method only returns a snapshot when
        a non-OS event is reached. Auto-dispatch does NOT persist across subsequent
        `snapshot.resume()` calls — OS calls produced after the first resume surface
        as a `FunctionSnapshot` with `is_os_function=True`, as before.

        On completion or error, the REPL state is automatically restored.

        Arguments:
            code: The Python code snippet to execute
            inputs: Dict of input values injected into the REPL namespace
                before executing the snippet
            print_callback: Optional callback for print output
            mount: Optional filesystem mount(s) to expose inside the sandbox.
            os: Optional callback for OS calls. Called with (function_name, args, kwargs)
                and must return the appropriate value for the OS function. Return
                `NOT_HANDLED` to fall back to Monty's default unhandled behavior.
            skip_type_check: When `True`, static type checking is bypassed for
                this snippet AND the snippet is NOT appended to the accumulated
                type-check context, so later type-checked snippets will not see
                any names it defined. Has no effect unless `type_check=True`
                was set on the REPL.
        """

    def dump(self) -> bytes:
        """Serialize the REPL session to bytes."""

    @staticmethod
    def load(
        data: bytes,
        *,
        dataclass_registry: list[type] | None = None,
    ) -> MontyRepl:
        """Restore a REPL session from bytes."""

@final
class FunctionSnapshot:
    """
    Represents a paused execution waiting for an external function call return value.

    Contains information about the pending external function call and allows
    resuming execution with the return value.
    """

    @property
    def script_name(self) -> str:
        """The name of the script being executed."""

    @property
    def is_os_function(self) -> bool:
        """Whether this snapshot is for an OS function call (e.g., Path.stat)."""

    @property
    def is_method_call(self) -> bool:
        """Whether this snapshot is for a dataclass method call (first arg is `self`)."""

    @property
    def function_name(self) -> str | OsFunction:
        """The name of the function being called (external function or OS function like 'Path.stat').

        Will be a `OsFunction` if `is_os_function` is `True`.
        """

    @property
    def args(self) -> tuple[Any, ...]:
        """The positional arguments passed to the external function."""

    @property
    def kwargs(self) -> dict[str, Any]:
        """The keyword arguments passed to the external function."""

    def args_json(self) -> str:
        """Serialize the positional args as a JSON array.

        Uses the same natural-form mapping as 'MontyComplete.output_json':
        JSON-native Python values ('None', 'bool', 'int', 'float',
        'str', list, and dict with string keys) are emitted bare, while
        non-JSON-native values (tuples, bytes, sets, dataclasses, ...) are
        wrapped in a single-key object with a '$'-prefixed tag such as
        '{"$tuple": [...]}'.

        Raises:
            RuntimeError: If serialization fails.
        """

    def kwargs_json(self) -> str:
        """Serialize the keyword args as a JSON object.

        Python kwargs always have string keys, so the result is a plain
        '{"<name>": <value>, ...}' object using the same natural-form
        mapping as 'args_json' for the values.

        Raises:
            RuntimeError: If serialization fails.
        """

    @property
    def call_id(self) -> int:
        """The unique identifier for this external function call."""

    def resume(
        self,
        result: ExternalResult,
        *,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
    ) -> FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete:
        """Resume execution with a return value from the external function.

        `resume` may only be called once on each FunctionSnapshot instance.

        The GIL is released allowing parallel execution.

        When `mount` or `os` is provided, OS calls produced by the resumed
        execution are auto-dispatched internally until the next non-OS event,
        matching the semantics of `Monty.start(mount=..., os=...)`. Auto-dispatch
        does not persist beyond this single `resume()` call — each `resume()`
        must be passed `mount`/`os` again to continue the behavior.

        Arguments:
            result: A typeddict representing the return value, exception, or pending future.
            mount: Optional filesystem mount(s) to expose inside the sandbox.
            os: Optional callback for OS calls. Return `NOT_HANDLED` to fall back
                to Monty's default unhandled behavior.

        Returns:
            FunctionSnapshot if another external function call is pending,
            FutureSnapshot if another name lookup is pending,
            FutureSnapshot if futures need to be resolved,
            MontyComplete if execution finished.

        Raises:
            TypeError: If both arguments are incorrect.
            RuntimeError: If execution has already completed.
            MontyRuntimeError: If the code raises an exception during execution
        """

    def resume_not_handled(
        self,
        *,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
    ) -> FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete:
        """Resume an OS snapshot using Monty's default unhandled-OS behavior.

        This is only valid when `is_os_function` is `True`. It behaves the same
        as leaving the OS call unhandled in Monty's runtime.

        When `mount` or `os` is provided, OS calls produced by the resumed
        execution are auto-dispatched until a non-OS event is reached.
        """

    def dump(self) -> bytes:
        """
        Serialize the FunctionSnapshot instance to a binary format.

        The serialized data can be restored with `load_snapshot()` or `load_repl_snapshot()`.
        This allows suspending execution and resuming later, potentially in a different process.

        Note: The `print_callback` is not serialized and must be re-provided to
        `load_snapshot()` or `load_repl_snapshot()` when the snapshot is restored.

        Returns:
            Bytes containing the serialized FunctionSnapshot instance.

        Raises:
            ValueError: If serialization fails.
            RuntimeError: If the progress has already been resumed.
        """

    def __repr__(self) -> str: ...

@final
class NameLookupSnapshot:
    """
    Represents a paused execution waiting for multiple futures to be resolved.

    Contains information about the pending futures and allows resuming execution
    with the results.
    """

    @property
    def script_name(self) -> str:
        """The name of the script being executed."""

    @property
    def variable_name(self) -> str:
        """The name of the variable being looked up."""

    def resume(
        self,
        *,
        value: Any | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
    ) -> FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete:
        """Resume execution with result the value from a name lookup, if any.

        If no `value` is passed, a `NameError` is raised.

        `resume` may only be called once on each NameLookupSnapshot instance.

        The GIL is released allowing parallel execution.

        When `mount` or `os` is provided, OS calls produced after the name is
        resolved are auto-dispatched until a non-OS event is reached, matching
        the semantics of `Monty.start(mount=..., os=...)`.

        Arguments:
            value: The value from the name lookup, if any.
            mount: Optional filesystem mount(s) to expose inside the sandbox.
            os: Optional callback for OS calls. Return `NOT_HANDLED` to fall back
                to Monty's default unhandled behavior.

        Returns:
            FunctionSnapshot if an external function call is pending,
            NameLookupSnapshot if more futures need to be resolved,
            FutureSnapshot if another name lookup is pending,
            MontyComplete if execution finished.

        Raises:
            TypeError: If result dict has invalid keys.
            RuntimeError: If execution has already completed.
            MontyRuntimeError: If the code raises an exception during execution
        """

    def dump(self) -> bytes:
        """
        Serialize the NameLookupSnapshot instance to a binary format.

        The serialized data can be restored with `load_snapshot()` or `load_repl_snapshot()`.
        This allows suspending execution and resuming later, potentially in a different process.

        Note: The `print_callback` is not serialized and must be re-provided to
        `load_snapshot()` or `load_repl_snapshot()` when the snapshot is restored.

        Returns:
            Bytes containing the serialized NameLookupSnapshot instance.

        Raises:
            ValueError: If serialization fails.
            RuntimeError: If the progress has already been resumed.
        """

    def __repr__(self) -> str: ...

@final
class FutureSnapshot:
    """
    Represents a paused execution waiting for multiple futures to be resolved.

    Contains information about the pending futures and allows resuming execution
    with the results.
    """

    @property
    def script_name(self) -> str:
        """The name of the script being executed."""

    @property
    def pending_call_ids(self) -> list[int]:
        """The call IDs of the pending futures.

        Raises an error if the snapshot has already been resumed.
        """

    def resume(
        self,
        results: dict[int, ExternalResult],
        *,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | None = None,
    ) -> FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete:
        """Resume execution with results for one or more futures.

        `resume` may only be called once on each FutureSnapshot instance.

        The GIL is released allowing parallel execution.

        When `mount` or `os` is provided, OS calls produced after the futures
        resolve are auto-dispatched until a non-OS event is reached, matching
        the semantics of `Monty.start(mount=..., os=...)`.

        Arguments:
            results: Dict mapping call_id to result dict. Each result dict must have
                either 'return_value' or 'exception' key (not both).
            mount: Optional filesystem mount(s) to expose inside the sandbox.
            os: Optional callback for OS calls. Return `NOT_HANDLED` to fall back
                to Monty's default unhandled behavior.

        Returns:
            FunctionSnapshot if an external function call is pending,
            NameLookupSnapshot if more futures need to be resolved,
            FutureSnapshot if more futures need to be resolved,
            MontyComplete if execution finished.

        Raises:
            TypeError: If result dict has invalid keys.
            RuntimeError: If execution has already completed.
            MontyRuntimeError: If the code raises an exception during execution
        """

    def dump(self) -> bytes:
        """
        Serialize the FutureSnapshot instance to a binary format.

        The serialized data can be restored with `load_snapshot()` or `load_repl_snapshot()`.
        This allows suspending execution and resuming later, potentially in a different process.

        Note: The `print_callback` is not serialized and must be re-provided to
        `load_snapshot()` or `load_repl_snapshot()` when the snapshot is restored.

        Returns:
            Bytes containing the serialized FutureSnapshot instance.

        Raises:
            ValueError: If serialization fails.
            RuntimeError: If the progress has already been resumed.
        """

    def __repr__(self) -> str: ...

@final
class MontyComplete:
    """The result of a completed code execution."""

    @property
    def output(self) -> Any:
        """The final output value from the executed code.

        Converted from Monty's internal representation to a Python object on
        each access. Callers that want to inspect the value repeatedly should
        save it to a local variable.
        """

    def output_json(self) -> str:
        """Serialize the output as a Monty-specific JSON string.

        This is **not** a drop-in wrapper around ``json.dumps(result.output)``:
        the shape is chosen to preserve types that plain JSON can't express,
        so consumers can round-trip richer Python values than CPython's
        stdlib would allow. JSON-native Python types (None, bool, int, float,
        str, list, and dict with string keys) become bare JSON values.
        Non-JSON-native types are wrapped in a single-key object with a
        ``$``-prefixed tag, for example:

        - tuple → ``{"$tuple": [...]}``
        - bytes → ``{"$bytes": [...]}``
        - set / frozenset → ``{"$set": [...]}`` / ``{"$frozenset": [...]}``
        - ``...`` → ``{"$ellipsis": "..."}``
        - ``nan`` / ``inf`` / ``-inf`` → ``{"$float": "nan" | "inf" | "-inf"}``
        - dict with any non-string key → ``{"$dict": [[k, v], ...]}``
        - dataclass → ``{"$dataclass": {...}, "name": "ClassName"}``

        Raises:
            RuntimeError: If serialization fails.
        """

    def __repr__(self) -> str: ...

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

    def display(self, format: Literal['type-msg', 'msg'] = 'msg') -> str:
        """Returns formatted exception string.

        Args:
            format: 'type-msg' - 'ExceptionType: message' format
                  'msg' - just the message
        """

@final
class MontyTypingError(MontyError):
    """Raised when type checking finds errors in the code.

    This exception is raised when static type analysis detects type errors
    before execution. Use `.display(format, color)` to render the diagnostics
    in different formats.

    Inherits exception(), __str__() from MontyError.
    Cannot be constructed directly from Python.
    """

    def display(
        self,
        format: Literal[
            'full', 'concise', 'azure', 'json', 'jsonlines', 'rdjson', 'pylint', 'gitlab', 'github'
        ] = 'full',
        color: bool = False,
    ) -> str:
        """Renders the type error diagnostics with the specified format and color.

        Args:
            format: Output format for the diagnostics. Defaults to 'full'.
            color: Whether to include ANSI color codes. Defaults to False.
        """

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

def load_snapshot(
    data: bytes,
    *,
    print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
    dataclass_registry: list[type] | None = None,
) -> FunctionSnapshot | NameLookupSnapshot | FutureSnapshot:
    """Load a non-REPL snapshot from serialized bytes.

    Auto-detects the snapshot type (FunctionSnapshot, NameLookupSnapshot, or
    FutureSnapshot) from the serialized data.

    Arguments:
        data: Serialized snapshot bytes from `.dump()`
        print_callback: Optional callback for print output
        dataclass_registry: Optional list of dataclass types to register

    Returns:
        The deserialized snapshot, ready to be resumed.

    Raises:
        ValueError: If deserialization fails or data contains a REPL snapshot
            (use `load_repl_snapshot` for those).
    """

def load_repl_snapshot(
    data: bytes,
    *,
    print_callback: Callable[[Literal['stdout'], str], None] | CollectStreams | CollectString | None = None,
    dataclass_registry: list[type] | None = None,
) -> tuple[FunctionSnapshot | NameLookupSnapshot | FutureSnapshot, MontyRepl]:
    """Load a REPL snapshot from serialized bytes.

    Returns both the snapshot and a reconstructed `MontyRepl` session.
    The snapshot's REPL variant is wired to the returned `MontyRepl`,
    so resuming the snapshot will update the REPL state.

    Arguments:
        data: Serialized snapshot bytes from `.dump()` on a REPL snapshot
        print_callback: Optional callback for print output
        dataclass_registry: Optional list of dataclass types to register

    Returns:
        A tuple of (snapshot, MontyRepl).

    Raises:
        ValueError: If deserialization fails.
    """
