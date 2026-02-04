from __future__ import annotations

from abc import ABC, abstractmethod
from pathlib import PurePosixPath
from typing import TYPE_CHECKING, Any, Callable, Literal, NamedTuple, Protocol, Sequence, TypeAlias, TypeGuard

if TYPE_CHECKING:
    # Self is 3.11+, hence this
    from typing import Self

__all__ = 'OsFunction', 'AbstractOS', 'AbstractFile', 'MemoryFile', 'CallbackFile', 'OSAccess', 'StatResult'

OsFunction = Literal[
    'Path.exists',
    'Path.is_file',
    'Path.is_dir',
    'Path.is_symlink',
    'Path.read_text',
    'Path.read_bytes',
    'Path.write_text',
    'Path.write_bytes',
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
]


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
        import time

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
        import time

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
    """Abstract base class for implementing virtual filesystems and OS access.

    Subclass this and implement the abstract methods to provide a custom
    filesystem that Monty code can interact with via Path methods.

    Pass an instance as the `os` parameter to `Monty.run()`.
    """

    def __call__(self, function_name: OsFunction, args: tuple[Any, ...], kwargs: dict[str, Any] | None = None) -> Any:
        """Dispatch a filesystem operation to the appropriate method.

        This is called by Monty when Monty code invokes Path methods.
        You typically don't need to override this method.

        Args:
            function_name: The Path method being called (e.g., 'Path.exists').
            args: The arguments passed to the method.
            kwargs: The keyword arguments passed to the method.

        Returns:
            The result of the filesystem operation.
        """
        kwargs = kwargs or {}
        match function_name:
            case 'Path.exists':
                return self.path_exists(*args)
            case 'Path.is_file':
                return self.path_is_file(*args)
            case 'Path.is_dir':
                return self.path_is_dir(*args)
            case 'Path.is_symlink':
                return self.path_is_symlink(*args)
            case 'Path.read_text':
                return self.path_read_text(*args)
            case 'Path.read_bytes':
                return self.path_read_bytes(*args)
            case 'Path.write_text':
                return self.path_write_text(*args)
            case 'Path.write_bytes':
                return self.path_write_bytes(*args)
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

    @abstractmethod
    def path_read_text(self, path: PurePosixPath) -> str:
        """Read the contents of a file as text.

        Args:
            path: The path to the file.

        Returns:
            The file contents as a string.

        Raises:
            FileNotFoundError: If the file does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_read_bytes(self, path: PurePosixPath) -> bytes:
        """Read the contents of a file as bytes.

        Args:
            path: The path to the file.

        Returns:
            The file contents as bytes.

        Raises:
            FileNotFoundError: If the file does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_write_text(self, path: PurePosixPath, data: str) -> int:
        """Write text data to a file.

        Args:
            path: The path to the file.
            data: The text content to write.

        Returns:
            The number of characters written.

        Raises:
            FileNotFoundError: If the parent directory does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_write_bytes(self, path: PurePosixPath, data: bytes) -> int:
        """Write binary data to a file.

        Args:
            path: The path to the file.
            data: The binary content to write.

        Returns:
            The number of bytes written.

        Raises:
            FileNotFoundError: If the parent directory does not exist.
            IsADirectoryError: If the path is a directory.
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
    `MemoryFile`) and cannot access the real filesystem.

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

        Raises:
            AssertionError: If root_dir is not an absolute path.
            ValueError: If a file path conflicts with another file (e.g., trying
                to create a file inside another file's path).
        """
        self.files = list(files) if files else []
        self.environ = environ or {}
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

    def path_read_text(self, path: PurePosixPath) -> str:
        file = self._get_file(path)
        content = file.read_content()
        return content if isinstance(content, str) else content.decode()

    def path_read_bytes(self, path: PurePosixPath) -> bytes:
        file = self._get_file(path)
        content = file.read_content()
        return content if isinstance(content, bytes) else content.encode()

    def path_write_text(self, path: PurePosixPath, data: str) -> int:
        self._write_file(path, data)
        return len(data)

    def path_write_bytes(self, path: PurePosixPath, data: bytes) -> int:
        self._write_file(path, data)
        return len(data)

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
