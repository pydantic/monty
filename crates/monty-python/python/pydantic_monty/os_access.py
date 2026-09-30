from __future__ import annotations

import asyncio
import datetime
import functools
import inspect
import os
import time
from abc import ABC, abstractmethod
from pathlib import PurePosixPath
from typing import (
    TYPE_CHECKING,
    Any,
    Callable,
    Coroutine,
    Literal,
    NamedTuple,
    Protocol,
    Sequence,
    TypeAlias,
    TypeGuard,
)

from ._monty import NOT_HANDLED, MontyFileHandle

if TYPE_CHECKING:
    # Self is 3.11+, hence this
    from typing import Self

    # only an annotation here; importing at runtime would be circular
    from . import TimeCaller

__all__ = (
    'OsFunction',
    'AbstractOS',
    'AbstractFile',
    'MemoryFile',
    'CallbackFile',
    'OSAccess',
    'StatResult',
    'path_from_arg',
)

OsFunction = Literal[
    'Path.exists',
    'Path.is_file',
    'Path.is_dir',
    'Path.is_symlink',
    'open',
    'Path.read_text',
    'Path.read_bytes',
    'Path.write_text',
    'Path.write_bytes',
    'Path.append_text',
    'Path.append_bytes',
    'Path.mkdir',
    'Path.unlink',
    'Path.rmdir',
    'Path.iterdir',
    'Path.stat',
    'Path.rename',
    'Path.resolve',
    'Path.absolute',
    'os.getenv',
    'os.environ',
    'date.today',
    'datetime.now',
    'os.urandom',
    'time.time',
    'time.sleep',
    'asyncio.sleep',
    'system.sleep',
    'system.async_sleep',
]

MAX_URANDOM_BYTES_DEFAULT: int = 1_048_576
"""Default maximum host allocation per `urandom()` call. 1 MiB."""


class StatResult(NamedTuple):
    """Equivalent to os.stat_result."""

    @classmethod
    def file_stat(cls, size: int, mode: int = 0o644, mtime: float | None = None) -> Self:
        """Creates a stat_result namedtuple for a regular file.

        Use this when responding to Path.stat() OS calls.

        Args:
            size: File size in bytes
            mode: File permissions as octal (e.g., 0o644) or full mode with file type
            mtime: Modification time as Unix timestamp, defaults to Now.
        """
        # If only permission bits provided (no file type), add regular file type
        if mode < 0o1000:
            mode = mode | 0o100_000
        mtime = time.time() if mtime is None else mtime
        return cls(mode, 0, 0, 1, 0, 0, size, mtime, mtime, mtime)

    @classmethod
    def dir_stat(cls, mode: int = 0o755, mtime: float | None = None) -> Self:
        """Creates a stat_result namedtuple for a directory.

        Use this when responding to Path.stat() OS calls on directories.

        Args:
            mode: Directory permissions as octal (e.g., 0o755) or full mode with file type
            mtime: Modification time as Unix timestamp, defaults to Now.

        Returns:
            A namedtuple with stat_result fields
        """
        # If only permission bits provided (no file type), add directory type
        if mode < 0o1000:
            mode = mode | 0o040_000

        mtime = time.time() if mtime is None else mtime
        return cls(mode, 0, 0, 2, 0, 0, 4096, mtime, mtime, mtime)

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


class AbstractOS(ABC):
    """Base class for virtual filesystems and host OS callbacks.

    Implement the abstract methods and pass an instance to `feed_run(code, os=...)`.
    Clock and sleep callbacks require `os_policy` to select `'call_host'`.
    """

    max_urandom_bytes: int = MAX_URANDOM_BYTES_DEFAULT
    """Maximum host allocation per `urandom()` call; defaults to 1 MiB."""

    max_sleep: float | None = 10
    """Maximum seconds per host sleep; `None` waits the full requested time.
    Applies to `'call_host'` sleeps and manually dispatched `feed_start` sleeps.
    Default `'system'` sleeps use the pool's `sleep_system_max` instead."""

    def __call__(
        self,
        *,
        name: OsFunction,
        args: tuple[Any, ...],
        kwargs: dict[str, Any],
        is_async: bool,
        **_future_kwargs: Any,
    ) -> Any:
        """The `OsHandler` entrypoint Monty calls; see `OsHandler`.

        Override `dispatch()` when you want to customize routing or return
        `NOT_HANDLED`.

        Returns:
            The OS operation result, or `NOT_HANDLED` to let Monty apply its
            standard unhandled-operation behavior.
        """
        try:
            if _dispatch_takes_is_async(type(self)):
                return self.dispatch(name, args, kwargs, is_async=is_async)
            # an override with the older three-argument signature never sees
            # `is_async`, so its sleeps block as they did before it existed
            return self.dispatch(name, args, kwargs)
        except NotImplementedError:
            return NOT_HANDLED

    def dispatch(
        self,
        function_name: OsFunction,
        args: tuple[Any, ...],
        kwargs: dict[str, Any] | None = None,
        *,
        is_async: bool = False,
    ) -> Any:
        """Dispatch an OS operation to the appropriate method.

        This handles Monty's built-in `pathlib.Path`, `os`, and host clock
        operations. Subclasses can override it for custom behavior or return
        `NOT_HANDLED` to delegate back to Monty's default fallback errors.

        Args:
            function_name: The OS operation being called (e.g., 'Path.exists').
            args: The arguments passed to the method.
            kwargs: The keyword arguments passed to the method.
            is_async: Whether the caller can await a coroutine answer; see `OsHandler`.

        Returns:
            The result of the OS operation.
        """
        kwargs = kwargs or {}
        # `read`/`write`/`append` handlers receive either a `PurePosixPath`
        # (from `pathlib.Path` methods) or a `MontyFileHandle` (from a file
        # opened via `open()`). Each handler is responsible for coercing
        # its argument — `path_from_arg()` is the canonical one-liner;
        # subclasses that want the full `MontyFileHandle` (e.g. for
        # `id`-keyed caches) inspect the argument type directly instead.
        match function_name:
            case 'Path.exists':
                return self.path_exists(*args)
            case 'Path.is_file':
                return self.path_is_file(*args)
            case 'Path.is_dir':
                return self.path_is_dir(*args)
            case 'Path.is_symlink':
                return self.path_is_symlink(*args)
            case 'open':
                return self.path_open(*args)
            case 'Path.read_text':
                return self.path_read_text(*args)
            case 'Path.read_bytes':
                return self.path_read_bytes(*args)
            case 'Path.write_text':
                return self.path_write_text(*args)
            case 'Path.write_bytes':
                return self.path_write_bytes(*args)
            case 'Path.append_text':
                return self.path_append_text(*args)
            case 'Path.append_bytes':
                return self.path_append_bytes(*args)
            case 'Path.mkdir':
                assert len(kwargs) <= 2, f'Unexpected keyword arguments: {kwargs}'
                parents = kwargs.get('parents', False)
                exist_ok = kwargs.get('exist_ok', False)
                return self.path_mkdir(*args, parents=parents, exist_ok=exist_ok)
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
            case 'os.getenv':
                return self.getenv(*args)
            case 'os.environ':
                return self.get_environ()
            case 'date.today':
                return self.date_today()
            case 'datetime.now':
                return self.datetime_now(*args)
            case 'os.urandom':
                return self.urandom(*args)
            case 'time.time':
                return self.time(*args)
            # `feed_start` callers can dispatch system sleeps manually.
            case 'time.sleep' | 'system.sleep':
                return self.sleep(*args)
            case 'asyncio.sleep' | 'system.async_sleep':
                return self.async_sleep(*args, is_async=is_async)
            case _:  # pyright: ignore[reportUnnecessaryComparison]
                raise NotImplementedError(f'Unknown OS function: {function_name}')

    @abstractmethod
    def path_exists(self, path: PurePosixPath) -> bool:
        """Check if a path exists.

        Args:
            path: The path to check.

        Returns:
            True if the path exists, False otherwise.
        """
        raise NotImplementedError

    @abstractmethod
    def path_is_file(self, path: PurePosixPath) -> bool:
        """Check if a path is a regular file.

        Args:
            path: The path to check.

        Returns:
            True if the path is a regular file, False otherwise.
        """
        raise NotImplementedError

    @abstractmethod
    def path_is_dir(self, path: PurePosixPath) -> bool:
        """Check if a path is a directory.

        Args:
            path: The path to check.

        Returns:
            True if the path is a directory, False otherwise.
        """
        raise NotImplementedError

    @abstractmethod
    def path_is_symlink(self, path: PurePosixPath) -> bool:
        """Check if a path is a symbolic link.

        Args:
            path: The path to check.

        Returns:
            True if the path is a symbolic link, False otherwise.
        """
        raise NotImplementedError

    def path_open(self, path: PurePosixPath, mode: str) -> MontyFileHandle:
        """Perform the open-time effect for `open(path, mode)`.

        Monty issues this OS call from the `open()` builtin. The handler is
        responsible for the side-effect that matches `mode` and for returning
        a `MontyFileHandle` Monty can wrap into an `_io.*` file object:

        - `'r'` / `'r+'` (text/binary): verify the file exists and is not a
          directory; raise `FileNotFoundError` / `IsADirectoryError` if not.
        - `'w'` / `'w+'`: truncate the file (creating it if missing).
        - `'a'` / `'a+'`: create the file if missing, leaving existing
          content untouched.

        The returned `MontyFileHandle` becomes the first argument of any
        subsequent `Path.read_text` / `Path.write_text` / `Path.append_text`
        (and bytes variants) OS calls; `dispatch()` normalizes it back to the
        underlying `PurePosixPath` so the existing `path_*` handlers continue
        to work without modification.
        """
        raise NotImplementedError

    @abstractmethod
    def path_read_text(self, path: PurePosixPath | MontyFileHandle) -> str:
        """Read the contents of a file as text.

        Args:
            path: The path to the file, either as a `PurePosixPath` (from
                `pathlib.Path` methods) or a `MontyFileHandle` (after the
                file was opened via `open()`). Use `path_from_arg()` to
                collapse both shapes if you don't need the extra handle
                metadata.

        Returns:
            The file contents as a string.

        Raises:
            FileNotFoundError: If the file does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_read_bytes(self, path: PurePosixPath | MontyFileHandle) -> bytes:
        """Read the contents of a file as bytes.

        Args:
            path: The path to the file, either as a `PurePosixPath` (from
                `pathlib.Path` methods) or a `MontyFileHandle` (after the
                file was opened via `open()`). Use `path_from_arg()` to
                collapse both shapes if you don't need the extra handle
                metadata.

        Returns:
            The file contents as bytes.

        Raises:
            FileNotFoundError: If the file does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_write_text(self, path: PurePosixPath | MontyFileHandle, data: str) -> int:
        """Write text data to a file.

        Args:
            path: The path to the file, either as a `PurePosixPath` (from
                `pathlib.Path` methods) or a `MontyFileHandle` (after the
                file was opened via `open()`). Use `path_from_arg()` to
                collapse both shapes if you don't need the extra handle
                metadata.
            data: The text content to write.

        Returns:
            The number of characters written.

        Raises:
            FileNotFoundError: If the parent directory does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_write_bytes(self, path: PurePosixPath | MontyFileHandle, data: bytes) -> int:
        """Write binary data to a file.

        Args:
            path: The path to the file, either as a `PurePosixPath` (from
                `pathlib.Path` methods) or a `MontyFileHandle` (after the
                file was opened via `open()`). Use `path_from_arg()` to
                collapse both shapes if you don't need the extra handle
                metadata.
            data: The binary content to write.

        Returns:
            The number of bytes written.

        Raises:
            FileNotFoundError: If the parent directory does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    def path_append_text(self, path: PurePosixPath | MontyFileHandle, data: str) -> int:
        """Append text data to a file and return the number of characters written.

        Accepts either a `PurePosixPath` or a `MontyFileHandle` (see
        `path_read_text` for details).
        """
        raise NotImplementedError

    def path_append_bytes(self, path: PurePosixPath | MontyFileHandle, data: bytes) -> int:
        """Append binary data to a file and return the number of bytes written.

        Accepts either a `PurePosixPath` or a `MontyFileHandle` (see
        `path_read_text` for details).
        """
        raise NotImplementedError

    @abstractmethod
    def path_mkdir(self, path: PurePosixPath, parents: bool, exist_ok: bool) -> None:
        """Create a directory.

        Args:
            path: The path of the directory to create.
            parents: If True, create parent directories as needed.
            exist_ok: If True, don't raise an error if the directory exists.

        Raises:
            FileNotFoundError: If parents is False and parent directory doesn't exist.
            FileExistsError: If exist_ok is False and the directory already exists.
        """
        raise NotImplementedError

    @abstractmethod
    def path_unlink(self, path: PurePosixPath) -> None:
        """Remove a file.

        Args:
            path: The path to the file to remove.

        Raises:
            FileNotFoundError: If the file does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_rmdir(self, path: PurePosixPath) -> None:
        """Remove an empty directory.

        Args:
            path: The path to the directory to remove.

        Raises:
            FileNotFoundError: If the directory does not exist.
            NotADirectoryError: If the path is not a directory.
            OSError: If the directory is not empty.
        """
        raise NotImplementedError

    @abstractmethod
    def path_iterdir(self, path: PurePosixPath) -> list[PurePosixPath]:
        """List the contents of a directory.

        Args:
            path: The path to the directory.

        Returns:
            A list of full paths (as PurePosixPath) for entries in the directory.

        Raises:
            FileNotFoundError: If the directory does not exist.
            NotADirectoryError: If the path is not a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_stat(self, path: PurePosixPath) -> StatResult:
        """Get file status information.

        Use file_stat(), dir_stat(), or symlink_stat() helpers to create the return value.

        Args:
            path: The path to stat.

        Returns:
            A StatResult with file metadata.

        Raises:
            FileNotFoundError: If the path does not exist.
        """
        raise NotImplementedError

    @abstractmethod
    def path_rename(self, path: PurePosixPath, target: PurePosixPath) -> None:
        """Rename a file or directory.

        Args:
            path: The current path.
            target: The new path.

        Raises:
            FileNotFoundError: If the source path does not exist.
            FileExistsError: If the target already exists (platform-dependent).
        """
        raise NotImplementedError

    @abstractmethod
    def path_resolve(self, path: PurePosixPath) -> str:
        """Resolve a path to an absolute path, resolving any symlinks.

        Args:
            path: The path to resolve.

        Returns:
            The resolved absolute path with symlinks resolved.
        """
        raise NotImplementedError

    @abstractmethod
    def path_absolute(self, path: PurePosixPath) -> str:
        """Convert a path to an absolute path without resolving symlinks.

        Args:
            path: The path to convert.

        Returns:
            The absolute path.
        """
        raise NotImplementedError

    @abstractmethod
    def getenv(self, key: str, default: str | None = None) -> str | None:
        """Get an environment variable value.

        Args:
            key: The name of the environment variable.
            default: The value to return if the environment variable is not set.

        Returns:
            The value of the environment variable, or `default` if not set.
        """
        raise NotImplementedError

    @abstractmethod
    def get_environ(self) -> dict[str, str]:
        """Get the entire environment as a dictionary.

        Returns:
            A dictionary containing all environment variables.
        """
        raise NotImplementedError

    def date_today(self) -> datetime.date:
        """Return the host's date when `os_policy` routes the clock or zone to the host.

        Use `os_policy` directly to configure a fixed clock.
        """
        return datetime.date.today()

    def datetime_now(self, tz: datetime.tzinfo | None = None) -> datetime.datetime:
        """Return host `datetime.now(tz)` when `os_policy` routes the clock or zone to the host.

        Use `os_policy` directly to configure a fixed clock.
        """
        return datetime.datetime.now(tz=tz)

    def urandom(self, size: int) -> bytes:
        """Return `size` random bytes for Monty's `os.urandom(size)` host callback.

        Raises `MemoryError` before allocating if `size` exceeds `max_urandom_bytes`.
        Under `random_start='call_host'`, an unseeded generator requests 2496 bytes on its first draw.
        An override that allocates host memory must apply its own limit.
        """
        if size > self.max_urandom_bytes:
            raise MemoryError(f'os.urandom() size exceeds max_urandom_bytes ({self.max_urandom_bytes})')
        return os.urandom(size)

    def time(self, caller: TimeCaller = 'time.time') -> float:
        """Return the epoch seconds for Monty's `time` module clocks.

        Reached only under `os_policy={'datetime': 'call_host'}`; override it
        alongside `date_today()` and `datetime_now()` for a virtual clock.
        An override must accept `caller`: the dispatcher passes it positionally.

        Args:
            caller: The `time` function that asked, e.g. `'time.monotonic'`; ignore it
                unless each clock should read differently.
        """
        return time.time()

    def sleep(self, seconds: float) -> None:
        """Block this thread for at most `max_sleep` seconds.

        Used for `'call_host'` sleeps and manually dispatched `feed_start` system sleeps.
        Override to scale or refuse waits, by raising or returning `NOT_HANDLED`.
        """
        time.sleep(self._capped(seconds))

    def async_sleep(self, delay: float, *, is_async: bool) -> Coroutine[Any, Any, None] | None:
        """Handle host-routed or manually dispatched `asyncio.sleep()`, capped by `max_sleep`.

        Returns a coroutine under `AsyncMonty`, allowing gathered sleeps to overlap; otherwise blocks via `sleep()`.
        Overrides may return `None` after waiting, or a coroutine when `is_async` (not a `Future` or `Task`).
        The coroutine's return is ignored; the sandbox retains its `asyncio.sleep()` result argument.
        """
        if is_async:
            return asyncio.sleep(self._capped(delay))
        return self.sleep(delay)

    def _capped(self, seconds: float) -> float:
        """`seconds` cut down to `max_sleep`, when there is one.

        Fails closed: a NaN cap is returned rather than the request (`min()` would keep
        the request), so the sleep raises instead of running uncapped.
        """
        cap = self.max_sleep
        return seconds if cap is None or seconds <= cap else cap


@functools.cache
def _dispatch_takes_is_async(cls: type[AbstractOS]) -> bool:
    """Whether `cls.dispatch` accepts `is_async`, cached per subclass.

    Overrides written against the three-argument `dispatch` predate the keyword
    and would fail on every call if it were passed; `**kwargs` counts as taking it.
    """
    params = inspect.signature(cls.dispatch).parameters
    is_async = params.get('is_async')
    by_keyword = {inspect.Parameter.KEYWORD_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD}
    return (is_async is not None and is_async.kind in by_keyword) or any(
        p.kind is inspect.Parameter.VAR_KEYWORD for p in params.values()
    )


class AbstractFile(Protocol):
    """Protocol defining the interface for files used with OSAccess.

    This protocol allows custom file implementations to be used with OSAccess.
    The built-in implementations are:

    - `MemoryFile`: Stores content in memory (recommended for sandboxed execution)
    - `CallbackFile`: Delegates to custom callbacks (use with caution - see its docstring)

    Security Note:
        Custom implementations of this protocol run in the host Python environment.
        The `read_content()` and `write_content()` methods can execute arbitrary code,
        including accessing the real filesystem. Only use implementations you trust.

        For sandboxed execution where Monty code should not access real files,
        use `MemoryFile` which stores all content in memory.

    Attributes:
        path: The virtual path of the file within the OSAccess filesystem.
        name: The filename (basename) extracted from path.
        permissions: Unix-style permission bits (e.g., 0o644).
        deleted: Whether the file has been marked as deleted.
    """

    path: PurePosixPath
    name: str
    permissions: int
    deleted: bool

    def read_content(self) -> str | bytes:
        """Read and return the file's content."""
        ...

    def write_content(self, content: str | bytes) -> None:
        """Write content to the file."""
        ...

    def delete(self) -> None:
        """Mark the file as deleted."""
        ...


Tree: TypeAlias = 'dict[str, AbstractFile | Tree]'


def _is_file(entry: None | AbstractFile | Tree) -> TypeGuard[AbstractFile]:
    return hasattr(entry, 'path')


def _is_dir(entry: None | AbstractFile | Tree) -> TypeGuard[Tree]:
    return isinstance(entry, dict)


class MemoryFile:
    """An in-memory virtual file for use with OSAccess.

    This is the recommended file type for sandboxed Monty execution. Content is
    stored entirely in Python memory with no access to the real filesystem.

    When Monty code reads from this file, it receives the stored content.
    When Monty code writes to this file, the content attribute is updated.

    Example::

        from pydantic_monty import Monty, OSAccess, MemoryFile

        fs = OSAccess(
            [
                MemoryFile('/config.json', '{"debug": true}'),
                MemoryFile('/data.bin', b'\\x00\\x01\\x02'),
            ]
        )

        result = Monty('''
            from pathlib import Path
            Path('/config.json').read_text()
        ''').run(os=fs)
        # result == '{"debug": true}'

    Attributes:
        path: The virtual path of the file within the OSAccess filesystem.
        name: The filename (basename) extracted from path.
        content: The file content (str for text, bytes for binary).
        permissions: Unix-style permission bits (default: 0o644).
        deleted: Whether the file has been marked as deleted.
    """

    path: PurePosixPath
    name: str
    content: str | bytes
    permissions: int = 0o644
    deleted: bool

    def __init__(self, path: str | PurePosixPath, content: str | bytes, *, permissions: int = 0o644) -> None:
        """Create an in-memory virtual file.

        Args:
            path: The virtual path for this file in the OSAccess filesystem.
            content: The initial file content (str for text, bytes for binary).
            permissions: Unix-style permission bits (default: 0o644).
        """
        self.path = PurePosixPath(path)
        self.name = self.path.name
        self.content = content
        self.permissions = permissions
        self.deleted = False

    def read_content(self) -> str | bytes:
        """Return the stored content."""
        return self.content

    def write_content(self, content: str | bytes) -> None:
        """Update the stored content."""
        self.content = content

    def delete(self) -> None:
        """Mark the file as deleted."""
        self.deleted = True

    def __repr__(self) -> str:
        repr_content = "'...'" if isinstance(self.content, str) else "b'...'"
        return f'MemoryFile(path={self.path}, content={repr_content}, permissions={self.permissions})'


_type_check_memory_file: AbstractFile = MemoryFile('test.txt', '')


class CallbackFile:
    """A virtual file backed by custom read/write callbacks.

    This class allows you to create files whose content is dynamically generated
    or persisted through custom logic. When Monty code reads or writes to this file,
    the provided callbacks are invoked.

    Security Warning:
        The callbacks execute in the host Python environment with FULL access to
        the real filesystem, network, and all system resources. A callback that
        accesses the real filesystem effectively breaks the Monty sandbox.

        Example of UNSAFE usage that breaks the sandbox::

            # DON'T DO THIS - allows Monty to read real files!
            CallbackFile(
                '/config.txt',
                read=lambda p: open('/etc/passwd').read(),
                write=lambda p, c: open('/tmp/out', 'w').write(c),
            )

        For sandboxed execution, use `MemoryFile` instead, which stores content
        purely in memory with no external access.

    Safe use cases for CallbackFile:
        - Returning dynamically computed content (e.g., current timestamp)
        - Logging writes without persisting them
        - Validating/transforming content before storage in memory
        - Integration testing with controlled external resources

    Attributes:
        path: The virtual path of the file within the OSAccess filesystem.
        name: The filename (basename) extracted from path.
        read: Callback invoked when the file is read. Receives the path and
            must return str or bytes.
        write: Callback invoked when the file is written. Receives the path
            and content (str or bytes).
        permissions: Unix-style permission bits (default: 0o644).
        deleted: Whether the file has been marked as deleted.
    """

    path: PurePosixPath
    name: str
    read: Callable[[PurePosixPath], str | bytes]
    write: Callable[[PurePosixPath, str | bytes], None]
    permissions: int = 0o644
    deleted: bool

    def __init__(
        self,
        path: str | PurePosixPath,
        read: Callable[[PurePosixPath], str | bytes],
        write: Callable[[PurePosixPath, str | bytes], None],
        *,
        permissions: int = 0o644,
    ) -> None:
        """Create a callback-backed virtual file.

        Args:
            path: The virtual path for this file in the OSAccess filesystem.
            read: Callback to generate content when the file is read.
            write: Callback to handle content when the file is written.
            permissions: Unix-style permission bits (default: 0o644).
        """
        self.path = PurePosixPath(path)
        self.name = self.path.name
        self.read = read
        self.write = write
        self.permissions = permissions
        self.deleted = False

    def read_content(self) -> str | bytes:
        """Read content by invoking the read callback."""
        return self.read(self.path)

    def write_content(self, content: str | bytes) -> None:
        """Write content by invoking the write callback."""
        self.write(self.path, content)

    def delete(self) -> None:
        """Mark the file as deleted."""
        self.deleted = True

    def __repr__(self) -> str:
        return f'CallbackFile(path={self.path}, read={self.read}, write={self.write}, permissions={self.permissions})'


_type_check_callback_file: AbstractFile = CallbackFile('test.txt', lambda _: '', lambda _, __: None)


class OSAccess(AbstractOS):
    """In-memory virtual filesystem for sandboxed Monty execution.

    OSAccess provides a complete virtual filesystem that Monty code can interact
    with via `pathlib.Path` methods. Files exist only in memory (when using
    `MemoryFile`) and cannot access the real filesystem. Environment access is
    isolated to the provided `environ` mapping. `date.today()` and
    `datetime.now()` default to the host clock; override those methods in a
    subclass if you need a deterministic or virtual clock.

    Security Model:
        When using `MemoryFile` objects, OSAccess is fully sandboxed:

        - Monty code can only access files explicitly registered with OSAccess
        - Path traversal (e.g., `../../etc/passwd`) cannot escape to real files
        - All file content is stored in Python memory, not on disk
        - Environment variables are isolated to the provided `environ` dict

        However, if `CallbackFile` is used, the callbacks run in the host
        environment and CAN access real resources. See `CallbackFile` docstring.

    Attributes:
        files: List of AbstractFile objects registered with this filesystem.
        environ: Dictionary of environment variables accessible via os.getenv().
        max_urandom_bytes: Maximum bytes per host entropy request.
    """

    files: list[AbstractFile]
    environ: dict[str, str]
    _tree: Tree

    def __init__(
        self,
        files: Sequence[AbstractFile] | None = None,
        environ: dict[str, str] | None = None,
        *,
        root_dir: str | PurePosixPath = '/',
        max_urandom_bytes: int = MAX_URANDOM_BYTES_DEFAULT,
        max_sleep: float | None = 10,
    ):
        """Create a virtual filesystem with the given files.

        Args:
            files: Files to register in the virtual filesystem. Use `MemoryFile`
                for sandboxed in-memory files, or `CallbackFile` for custom logic
                (with security caveats - see its docstring).
            environ: Environment variables accessible to Monty code via os.getenv().
                Isolated from the real environment.
            root_dir: Base directory for normalizing relative file paths. Relative
                paths in files will be prefixed with this. Default is '/'.
            max_urandom_bytes: Maximum bytes per `os.urandom()` call, defaulting to 1 MiB.
                Zero rejects nonempty requests, including unseeded `random` draws.
            max_sleep: Longest wait a `time.sleep()` or `asyncio.sleep()` performs,
                in seconds (default 10); longer sleeps are cut short, `None` waits
                the full time.

        Raises:
            AssertionError: If root_dir is not an absolute path.
            ValueError: If a file path conflicts with another file (e.g., trying
                to create a file inside another file's path), or `max_urandom_bytes` is negative.
                Also if `max_sleep` is negative or `nan`, either of which would disable the cap.
            TypeError: If `max_urandom_bytes` is not an int (a float `nan` would disable the cap),
                or `max_sleep` is not a number or `None`.
        """
        if not isinstance(max_urandom_bytes, int):  # pyright: ignore[reportUnnecessaryIsInstance]
            raise TypeError(f'max_urandom_bytes must be an int, not {type(max_urandom_bytes).__name__}')
        if max_urandom_bytes < 0:
            raise ValueError('max_urandom_bytes must be non-negative')
        self.max_urandom_bytes = max_urandom_bytes
        self.files = list(files) if files else []
        self.environ = environ or {}
        if max_sleep is not None:
            if isinstance(max_sleep, bool) or not isinstance(max_sleep, (int, float)):  # pyright: ignore[reportUnnecessaryIsInstance]
                raise TypeError(f'max_sleep must be a number or None, not {type(max_sleep).__name__}')
            # `not >=` rather than `<` so `nan`, which compares false both ways, is rejected
            if not max_sleep >= 0:
                raise ValueError('max_sleep must be non-negative')
        self.max_sleep = max_sleep
        # Initialize tree with root directory - / is always present
        self._tree = {'/': {}}
        root_dir = PurePosixPath(root_dir)
        assert root_dir.is_absolute(), f'Root directory must be absolute, got {root_dir}'
        for file in self.files:
            if not file.path.is_absolute():
                file.path = root_dir / file.path

            subtree = self._tree
            *dir_parts, name = file.path.parts
            for part in dir_parts:
                entry = subtree.setdefault(part, {})
                if _is_dir(entry):
                    subtree = entry
                else:
                    raise ValueError(f'Cannot put file {file} within sub-directory of file {entry}')

            subtree[name] = file

    def __repr__(self) -> str:
        return f'OSAccess(files={self.files}, environ={self.environ})'

    def path_exists(self, path: PurePosixPath) -> bool:
        return self._get_entry(path) is not None

    def path_is_file(self, path: PurePosixPath) -> bool:
        return _is_file(self._get_entry(path))

    def path_is_dir(self, path: PurePosixPath) -> bool:
        return _is_dir(self._get_entry(path))

    def path_is_symlink(self, path: PurePosixPath) -> bool:
        return False

    def path_open(self, path: PurePosixPath, mode: str) -> MontyFileHandle:
        """Perform the `open(path, mode)` open-time effect against the in-memory tree.

        - `'r'` / `'rb'`: verify the file exists and is not a directory;
          raise `FileNotFoundError` / `IsADirectoryError` if not.
        - `'w'` / `'wb'`: truncate (or create empty) via `_write_file`.
        - `'a'` / `'ab'`: create the file if missing; leave existing
          content untouched. Raises `IsADirectoryError` if the path is a
          directory.

        The returned `MontyFileHandle` carries the canonicalized mode
        (`'rt'` → `'r'`, `'r+b'` → `'rb+'`) and becomes the first argument
        of any subsequent read/write/append OS calls Monty issues for this
        file.

        The handle is constructed **before** any side effect so that a
        malformed `mode` raises `ValueError` without touching the
        filesystem. Direct callers (not routed through Monty, which
        pre-validates) could otherwise pass e.g. `'wxyz'` and silently
        trigger the truncate/create branch before the eventual mode-parse
        failure.
        """
        handle = MontyFileHandle(str(path), mode)
        canonical = handle.mode
        # `b`/`+` are orthogonal to the open-time effect — only the leading
        # action (`r`/`w`/`a`) matters here. The binary flag only decides
        # whether the empty seed is `b''` or `''`.
        action = canonical[0]
        empty: bytes | str = b'' if handle.binary else ''
        if action == 'r':
            entry = self._get_entry(path)
            if entry is None:
                raise FileNotFoundError(f'[Errno 2] No such file or directory: {str(path)!r}')
            if _is_dir(entry):
                raise IsADirectoryError(f'[Errno 21] Is a directory: {str(path)!r}')
        elif action == 'w':
            # Truncate (or create empty). _write_file handles both cases.
            self._write_file(path, empty)
        else:
            # `a`: create if missing, leave existing content untouched. Any
            # other action character is unreachable because `MontyFileHandle`
            # already rejected it above.
            assert action == 'a', f'unexpected canonical mode action: {canonical!r}'
            if self._get_entry(path) is None:
                self._write_file(path, empty)
            elif _is_dir(self._get_entry(path)):
                raise IsADirectoryError(f'[Errno 21] Is a directory: {str(path)!r}')
        return handle

    def path_read_text(self, path: PurePosixPath | MontyFileHandle) -> str:
        file = self._get_file(path_from_arg(path))
        content = file.read_content()
        return content if isinstance(content, str) else content.decode()

    def path_read_bytes(self, path: PurePosixPath | MontyFileHandle) -> bytes:
        file = self._get_file(path_from_arg(path))
        content = file.read_content()
        return content if isinstance(content, bytes) else content.encode()

    def path_write_text(self, path: PurePosixPath | MontyFileHandle, data: str) -> int:
        self._write_file(path_from_arg(path), data)
        return len(data)

    def path_write_bytes(self, path: PurePosixPath | MontyFileHandle, data: bytes) -> int:
        self._write_file(path_from_arg(path), data)
        return len(data)

    def path_append_text(self, path: PurePosixPath | MontyFileHandle, data: str) -> int:
        # Append text to whatever the file currently holds without changing
        # its storage type. A text-backed MemoryFile keeps str content; a
        # bytes-backed file (e.g. a CallbackFile that returned bytes) gets
        # appended bytes. Routing through path_append_bytes would convert
        # text storage to bytes, which would surprise the direct API and any
        # custom AbstractFile.write_content implementation.
        self._append(path_from_arg(path), data)
        return len(data)

    def path_append_bytes(self, path: PurePosixPath | MontyFileHandle, data: bytes) -> int:
        self._append(path_from_arg(path), data)
        return len(data)

    def _append(self, path: PurePosixPath, data: bytes | str) -> None:
        entry = self._get_entry(path)
        if _is_file(entry):
            content = entry.read_content()
            if isinstance(data, str):
                # path_append_text: write back str so a text-backed file
                # stays text-backed. Bytes-backed files are decoded first
                # so the storage type tracks the most recent write rather
                # than silently converting all writes to bytes.
                text_content = content if isinstance(content, str) else content.decode()
                entry.write_content(text_content + data)
            else:
                bytes_content = content if isinstance(content, bytes) else content.encode()
                entry.write_content(bytes_content + data)
        elif _is_dir(entry):
            raise IsADirectoryError(f'[Errno 21] Is a directory: {str(path)!r}')
        else:
            self._write_file(path, data)

    def _write_file(self, path: PurePosixPath, data: bytes | str) -> None:
        entry = self._get_entry(path)
        if _is_file(entry):
            entry.write_content(data)
            return
        elif _is_dir(entry):
            raise IsADirectoryError(f'[Errno 21] Is a directory: {str(path)!r}')

        # write a new file if the parent directory exists
        parent_entry = self._parent_entry(path)
        if _is_dir(parent_entry):
            file_path = PurePosixPath(path)
            parent_entry[file_path.name] = new_file = MemoryFile(file_path, data)
            self.files.append(new_file)
        else:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {str(path)!r}')

    def path_mkdir(self, path: PurePosixPath, parents: bool, exist_ok: bool) -> None:
        entry = self._get_entry(path)
        if _is_file(entry):
            raise FileExistsError(f'[Errno 17] File exists: {str(path)!r}')
        elif _is_dir(entry):
            if exist_ok:
                return
            else:
                raise FileExistsError(f'[Errno 17] File exists: {str(path)!r}')

        parent_entry = self._parent_entry(path)
        if _is_dir(parent_entry):
            parent_entry[PurePosixPath(path).name] = {}
            return
        elif _is_file(parent_entry):
            raise NotADirectoryError(f'[Errno 20] Not a directory: {str(path)!r}')
        elif parents:
            subtree = self._tree
            for part in PurePosixPath(path).parts:
                entry = subtree.setdefault(part, {})
                if _is_dir(entry):
                    subtree = entry
                else:
                    raise NotADirectoryError(f'[Errno 20] Not a directory: {str(path)!r}')
        else:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {str(path)!r}')

    def path_unlink(self, path: PurePosixPath) -> None:
        file = self._get_file(path)
        file.delete()
        # remove from parent
        parent_dir = self._parent_entry(path)
        assert _is_dir(parent_dir), f'Expected parent of a file to always be a directory, got {parent_dir}'
        del parent_dir[file.name]

    def path_rmdir(self, path: PurePosixPath) -> None:
        dir = self._get_dir(path)
        if dir:
            raise OSError(f'[Errno 39] Directory not empty: {str(path)!r}')
        # remove from parent
        parent_dir = self._parent_entry(path)
        assert _is_dir(parent_dir), f'Expected parent of a file to always be a directory, got {parent_dir}'
        del parent_dir[PurePosixPath(path).name]

    def path_iterdir(self, path: PurePosixPath) -> list[PurePosixPath]:
        # Return full paths as PurePosixPath objects (will be converted to MontyObject::Path)
        dir_path = PurePosixPath(path)
        return [dir_path / name for name in self._get_dir(path).keys()]

    def path_stat(self, path: PurePosixPath) -> StatResult:
        entry = self._get_entry_exists(path)
        if _is_file(entry):
            content = entry.read_content()
            size = len(content) if isinstance(content, bytes) else len(content.encode())
            return StatResult.file_stat(size=size, mode=entry.permissions)
        else:
            return StatResult.dir_stat()

    def path_rename(self, path: PurePosixPath, target: PurePosixPath) -> None:
        src_entry = self._get_entry(path)
        if src_entry is None:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {str(path)!r} -> {str(target)!r}')

        parent_dir = self._parent_entry(path)
        assert _is_dir(parent_dir), f'Expected parent of a file to always be a directory, got {parent_dir}'

        target_parent = self._parent_entry(target)
        if not _is_dir(target_parent):
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {str(path)!r} -> {str(target)!r}')
        target_entry = self._get_entry(target)

        if _is_file(src_entry):
            if _is_dir(target_entry):
                raise IsADirectoryError(f'[Errno 21] Is a directory: {str(path)!r} -> {str(target)!r}')
            if _is_file(target_entry):
                # need to mark the target as deleted as it'll be overwritten
                target_entry.delete()

            src_name = src_entry.path.name
            target_name = PurePosixPath(target).name
            # remove it from the old directory
            del parent_dir[src_name]
            # and put it in the new directory
            target_parent[target_name] = src_entry
        else:
            assert _is_dir(src_entry), 'src path must be a directory here'
            if _is_file(target_entry):
                raise NotADirectoryError(f'[Errno 20] Not a directory: {str(path)!r} -> {str(target)!r}')
            elif _is_dir(target_entry) and target_entry:
                raise OSError(f'[Errno 66] Directory not empty: {str(path)!r} -> {str(target)!r}')

            src_name = PurePosixPath(path).name
            target_name = PurePosixPath(target).name
            # remove it from the old directory
            del parent_dir[src_name]
            # and put it in the new directory
            target_parent[target_name] = src_entry

            # Update paths for all files in the renamed directory
            self._update_paths_recursive(src_entry, PurePosixPath(path), PurePosixPath(target))

    def path_resolve(self, path: PurePosixPath) -> str:
        # No symlinks in OSAccess, so resolve is same as absolute with normalization
        return self.path_absolute(path)

    def path_absolute(self, path: PurePosixPath) -> str:
        p = PurePosixPath(path)
        if p.is_absolute():
            return str(p)
        # In this virtual filesystem, we treat '/' as the working directory
        return str(PurePosixPath('/') / p)

    def getenv(self, key: str, default: str | None = None) -> str | None:
        return self.environ.get(key, default)

    def get_environ(self) -> dict[str, str]:
        return self.environ

    def _get_entry(self, path: PurePosixPath) -> Tree | AbstractFile | None:
        dir = self._tree

        *dir_parts, name = PurePosixPath(path).parts

        for part in dir_parts:
            entry = dir.get(part)
            if _is_dir(entry):
                dir = entry
            else:
                return None

        return dir.get(name)

    def _get_entry_exists(self, path: PurePosixPath) -> Tree | AbstractFile:
        entry = self._get_entry(path)
        if entry is None:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {str(path)!r}')
        else:
            return entry

    def _get_file(self, path: PurePosixPath) -> AbstractFile:
        entry = self._get_entry_exists(path)
        if _is_file(entry):
            return entry
        else:
            raise IsADirectoryError(f'[Errno 21] Is a directory: {str(path)!r}')

    def _get_dir(self, path: PurePosixPath) -> Tree:
        entry = self._get_entry_exists(path)
        if _is_dir(entry):
            return entry
        else:
            raise NotADirectoryError(f'[Errno 20] Not a directory: {str(path)!r}')

    def _parent_entry(self, path: PurePosixPath) -> Tree | AbstractFile | None:
        return self._get_entry(PurePosixPath(path).parent)

    def _update_paths_recursive(self, tree: Tree, old_prefix: PurePosixPath, new_prefix: PurePosixPath) -> None:
        """Update path attributes for all files in a tree after directory rename.

        When a directory is renamed, the internal tree structure is moved but
        AbstractFile objects still have their old paths. This method recursively
        updates all file paths by replacing old_prefix with new_prefix.
        """
        for entry in tree.values():
            if _is_file(entry):
                # Replace old prefix with new prefix in file path
                relative = entry.path.relative_to(old_prefix)
                entry.path = new_prefix / relative
            elif _is_dir(entry):
                self._update_paths_recursive(entry, old_prefix, new_prefix)


def path_from_arg(arg: PurePosixPath | MontyFileHandle) -> PurePosixPath:
    """Coerce a read/write/append argument to a virtual `PurePosixPath`."""
    if isinstance(arg, MontyFileHandle):
        return PurePosixPath(arg.path)
    else:
        return arg
