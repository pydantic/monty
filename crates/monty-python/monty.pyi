from typing import Any, Callable, Literal, final, overload

from typing_extensions import Self, TypedDict

__all__ = [
    'Monty',
    'MontyComplete',
    'MontySnapshot',
    'ResourceLimits',
    'MontyError',
    'MontySyntaxError',
    'MontyRuntimeError',
    'Frame',
]

class MontyError(Exception):
    """Base exception for all Monty interpreter errors.

    Catching `MontyError` will catch both syntax and runtime errors from Monty.
    This exception is raised internally by Monty and cannot be constructed directly.
    """

    def exception(self) -> BaseException:
        """Returns the inner exception as a Python exception object."""

    def display(self, show: Literal['traceback', 'type-msg', 'msg'] = 'traceback') -> str:
        """Returns formatted exception string.

        Args:
            show: 'traceback' - full traceback with exception
                  'type-msg' - 'ExceptionType: message' format
                  'msg' - just the message
        """

    def __str__(self) -> str:
        """Returns display('msg'), just the exception message."""

@final
class MontySyntaxError(MontyError):
    """Raised when Python code has syntax errors or cannot be parsed by Monty.

    Inherits exception(), display(), __str__() from MontyError.
    """

@final
class MontyRuntimeError(MontyError):
    """Raised when Monty code fails during execution.

    Inherits exception(), display(), __str__() from MontyError.
    Additionally provides traceback() for runtime errors.
    """

    def traceback(self) -> list[Frame]:
        """Returns the Monty traceback as a list of Frame objects."""

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
        external_functions: list[str] | None = None,
    ) -> Self:
        """
        Create a new Monty interpreter by parsing the given code.

        Arguments:
            code: Python code to execute
            script_name: Name used in tracebacks and error messages
            inputs: List of input variable names available in the code
            external_functions: List of external function names the code can call

        Raises:
            MontySyntaxError: If the code cannot be parsed
        """

    def run(
        self,
        *,
        inputs: dict[str, Any] | None = None,
        limits: ResourceLimits | None = None,
        external_functions: dict[str, Callable[..., Any]] | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | None = None,
    ) -> Any:
        """
        Execute the code and return the result.

        Arguments:
            inputs: Dict of input variable values (must match names from __init__)
            limits: Optional resource limits configuration
            external_functions: Dict of external function callbacks (must match names from __init__)
            print_callback: Optional callback for print output

        Returns:
            The result of the last expression in the code

        Raises:
            MontyRuntimeError: If the code raises an exception during execution
        """

    def start(
        self,
        *,
        inputs: dict[str, Any] | None = None,
        limits: ResourceLimits | None = None,
        print_callback: Callable[[Literal['stdout'], str], None] | None = None,
    ) -> MontySnapshot | MontyComplete:
        """
        Start the code execution and return a progress object, or completion.

        This allows you to iteratively run code and parse/resume whenever an external function is called.

        Arguments:
            inputs: Dict of input variable values (must match names from __init__)
            limits: Optional resource limits configuration
            print_callback: Optional callback for print output

        Returns:
            MontySnapshot if an external function call is pending,
            MontyComplete if execution finished without external calls.

        Raises:
            MontyRuntimeError: If the code raises an exception during execution
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
    def load(data: bytes) -> 'Monty':
        """
        Deserialize a Monty instance from binary format.

        Arguments:
            data: The serialized Monty data from `dump()`

        Returns:
            A new Monty instance.

        Raises:
            ValueError: If deserialization fails.
        """

    def __repr__(self) -> str: ...

@final
class MontySnapshot:
    """
    Represents a paused execution waiting for an external function call return value.

    Contains information about the pending external function call and allows
    resuming execution with the return value.
    """

    @property
    def script_name(self) -> str:
        """The name of the script being executed."""

    @property
    def function_name(self) -> str:
        """The name of the external function being called."""

    @property
    def args(self) -> tuple[Any, ...]:
        """The positional arguments passed to the external function."""

    @property
    def kwargs(self) -> dict[str, Any]:
        """The keyword arguments passed to the external function."""

    @overload
    def resume(self, *, return_value: Any) -> MontySnapshot | MontyComplete:
        """Resume execution with a return value from the external function.

        Resume may only be called once on each MontySnapshot instance.

        Arguments:
            return_value: The value to return from the external function call.
            exception: An exception to raise in the Monty interpreter.

        Returns:
            MontySnapshot if another external function call is pending,
            MontyComplete if execution finished.

        Raises:
            TypeError: If both arguments are provided.
            RuntimeError: If execution has already completed.
            MontyRuntimeError: If the code raises an exception during execution
        """

    @overload
    def resume(self, *, exception: BaseException) -> MontySnapshot | MontyComplete:
        """Resume execution by raising the exception in the Monty interpreter.

        See docstring for the first overload for more information.
        """

    def dump(self) -> bytes:
        """
        Serialize the MontySnapshot instance to a binary format.

        The serialized data can be stored and later restored with `MontySnapshot.load()`.
        This allows suspending execution and resuming later, potentially in a different process.

        Note: The `print_callback` is not serialized and must be re-provided via
        `set_print_callback()` after loading if print output is needed.

        Returns:
            Bytes containing the serialized MontySnapshot instance.

        Raises:
            ValueError: If serialization fails.
            RuntimeError: If the progress has already been resumed.
        """

    @staticmethod
    def load(
        data: bytes,
        *,
        print_callback: Callable[[Literal['stdout'], str], None] | None = None,
    ) -> 'MontySnapshot':
        """
        Deserialize a MontySnapshot instance from binary format.

        Note: The `print_callback` is not preserved during serialization and must be
        re-provided as a keyword argument if print output is needed.

        Arguments:
            data: The serialized MontySnapshot data from `dump()`
            print_callback: Optional callback for print output

        Returns:
            A new MontySnapshot instance.

        Raises:
            ValueError: If deserialization fails.
        """

    def __repr__(self) -> str: ...

@final
class MontyComplete:
    """The result of a completed code execution."""

    @property
    def output(self) -> Any:
        """The final output value from the executed code."""

    def __repr__(self) -> str: ...

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
