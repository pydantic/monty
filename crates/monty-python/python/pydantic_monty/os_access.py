from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import TYPE_CHECKING, Any, Callable, Literal, Protocol

if TYPE_CHECKING:
    from ._monty import StatResult

__all__ = 'OsFunction', 'AbstractOS', 'AbstractFile', 'MemoryFile', 'CallbackFile', 'OSAccess'

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
]


class AbstractOS(ABC):
    """Abstract base class for implementing virtual filesystems and OS access.

    Subclass this and implement the abstract methods to provide a custom
    filesystem that Monty code can interact with via Path methods.

    Pass an instance as the `os` parameter to `Monty.run()`.
    """

    def __call__(self, function_name: OsFunction, args: tuple[Any, ...]) -> Any:
        """Dispatch a filesystem operation to the appropriate method.

        This is called by Monty when Monty code invokes Path methods.
        You typically don't need to override this method.

        Args:
            function_name: The Path method being called (e.g., 'Path.exists').
            args: The arguments passed to the method.

        Returns:
            The result of the filesystem operation.
        """
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
                return self.path_mkdir(*args)
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

    @abstractmethod
    def path_exists(self, path: str) -> bool:
        """Check if a path exists.

        Args:
            path: The path to check.

        Returns:
            True if the path exists, False otherwise.
        """
        raise NotImplementedError

    @abstractmethod
    def path_is_file(self, path: str) -> bool:
        """Check if a path is a regular file.

        Args:
            path: The path to check.

        Returns:
            True if the path is a regular file, False otherwise.
        """
        raise NotImplementedError

    @abstractmethod
    def path_is_dir(self, path: str) -> bool:
        """Check if a path is a directory.

        Args:
            path: The path to check.

        Returns:
            True if the path is a directory, False otherwise.
        """
        raise NotImplementedError

    @abstractmethod
    def path_is_symlink(self, path: str) -> bool:
        """Check if a path is a symbolic link.

        Args:
            path: The path to check.

        Returns:
            True if the path is a symbolic link, False otherwise.
        """
        raise NotImplementedError

    @abstractmethod
    def path_read_text(self, path: str) -> str:
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
    def path_read_bytes(self, path: str) -> bytes:
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
    def path_write_text(self, path: str, data: str) -> None:
        """Write text data to a file.

        Args:
            path: The path to the file.
            data: The text content to write.

        Raises:
            FileNotFoundError: If the parent directory does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_write_bytes(self, path: str, data: bytes) -> None:
        """Write binary data to a file.

        Args:
            path: The path to the file.
            data: The binary content to write.

        Raises:
            FileNotFoundError: If the parent directory does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_mkdir(self, path: str, parents: bool, exist_ok: bool) -> None:
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
    def path_unlink(self, path: str) -> None:
        """Remove a file.

        Args:
            path: The path to the file to remove.

        Raises:
            FileNotFoundError: If the file does not exist.
            IsADirectoryError: If the path is a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_rmdir(self, path: str) -> None:
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
    def path_iterdir(self, path: str) -> list[str]:
        """List the contents of a directory.

        Args:
            path: The path to the directory.

        Returns:
            A list of entry names (not full paths) in the directory.

        Raises:
            FileNotFoundError: If the directory does not exist.
            NotADirectoryError: If the path is not a directory.
        """
        raise NotImplementedError

    @abstractmethod
    def path_stat(self, path: str) -> StatResult:
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
    def path_rename(self, path: str, target: str) -> None:
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
    def path_resolve(self, path: str) -> str:
        """Resolve a path to an absolute path, resolving any symlinks.

        Args:
            path: The path to resolve.

        Returns:
            The resolved absolute path with symlinks resolved.
        """
        raise NotImplementedError

    @abstractmethod
    def path_absolute(self, path: str) -> str:
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


class AbstractFile(Protocol):
    path: PurePosixPath
    name: str
    permissions: int

    def read_content(self) -> str | bytes: ...

    def write_content(self, content: str | bytes) -> None: ...


@dataclass
class MemoryFile:
    path: PurePosixPath
    name: str
    content: str | bytes
    permissions: int = 0o644

    def __init__(self, path: str | PurePosixPath, content: str | bytes, *, permissions: int = 0o644) -> None:
        self.path = PurePosixPath(path)
        self.name = self.path.name
        self.content = content
        self.permissions = permissions

    def read_content(self) -> str | bytes:
        return self.content

    def write_content(self, content: str | bytes) -> None:
        self.content = content


_type_check_memory_file: AbstractFile = MemoryFile('test.txt', '')


class CallbackFile:
    path: PurePosixPath
    name: str
    read: Callable[[PurePosixPath], str | bytes]
    write: Callable[[PurePosixPath, str | bytes], None]
    permissions: int = 0o644

    def __init__(
        self,
        path: str | PurePosixPath,
        read: Callable[[PurePosixPath], str | bytes],
        write: Callable[[PurePosixPath, str | bytes], None],
        *,
        permissions: int = 0o644,
    ) -> None:
        self.path = PurePosixPath(path)
        self.name = self.path.name
        self.read = read
        self.write = write
        self.permissions = permissions

    def __hash__(self) -> int:
        return hash(self.path)

    def read_content(self) -> str | bytes:
        return self.read(self.path)

    def write_content(self, content: str | bytes) -> None:
        self.write(self.path, content)

    def __repr__(self) -> str:
        return f'CallbackFile(path={self.path}, read={self.read}, write={self.write}, permissions={self.permissions})'


_type_check_callback_file: AbstractFile = CallbackFile('test.txt', lambda _: '', lambda _, __: None)


@dataclass
class OSAccess(AbstractOS):
    files: list[AbstractFile] = field(default_factory=list)
    environ: dict[str, str] = field(default_factory=dict)
    _tree: dict[PurePosixPath, dict[str, AbstractFile]] = field(init=False, default_factory=dict)

    def __post_init__(self):
        for file in self.files:
            p = PurePosixPath(file.path)
            if not p.is_absolute():
                raise ValueError(f'Files must have absolute paths, {file.path} is not absolute')

            # add all sub-paths of the parent, but NOT the file path itself to dirs
            parent: dict[str, AbstractFile] | None = None
            for i in range(1, len(p.parts)):
                d = PurePosixPath(*p.parts[:i])
                parent = self._tree.setdefault(d, {})

            assert parent is not None
            parent[p.name] = file

    def path_exists(self, path: str) -> bool:
        return self.path_is_dir(path) or self.path_is_file(path)

    def path_is_file(self, path: str) -> bool:
        p = PurePosixPath(path)
        if d := self._tree.get(p.parent):
            return p.name in d
        else:
            return False

    def path_is_dir(self, path: str) -> bool:
        return PurePosixPath(path) in self._tree

    def path_is_symlink(self, path: str) -> bool:
        return False

    def path_read_text(self, path: str) -> str:
        if file := self.get_file(path):
            content = file.read_content()
            return content if isinstance(content, str) else content.decode()
        raise FileNotFoundError(f'[Errno 2] No such file or directory: {path!r}')

    def path_read_bytes(self, path: str) -> bytes:
        if file := self.get_file(path):
            content = file.read_content()
            return content if isinstance(content, bytes) else content.encode()
        raise FileNotFoundError(f'[Errno 2] No such file or directory: {path!r}')

    def path_write_text(self, path: str, data: str) -> None:
        # todo
        pass

    def path_write_bytes(self, path: str, data: bytes) -> None:
        # todo
        pass

    def path_mkdir(self, path: str, parents: bool, exist_ok: bool) -> None:
        # todo
        pass

    def path_unlink(self, path: str) -> None:
        # todo
        pass

    def path_rmdir(self, path: str) -> None:
        # todo
        pass

    def path_iterdir(self, path: str) -> list[str]:
        # todo
        pass

    def path_stat(self, path: str) -> StatResult:
        # todo
        pass

    def path_rename(self, path: str, target: str) -> None:
        # todo
        pass

    def path_resolve(self, path: str) -> str:
        # todo
        pass

    def path_absolute(self, path: str) -> str:
        # todo
        pass

    def getenv(self, key: str, default: str | None = None) -> str | None:
        # todo
        pass

    def get_file(self, path: str) -> AbstractFile | None:
        p = PurePosixPath(path)
        if d := self._tree.get(p.parent):
            return d.get(p.name)
