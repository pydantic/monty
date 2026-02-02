from __future__ import annotations

from abc import ABC, abstractmethod
from pathlib import PurePosixPath
from typing import TYPE_CHECKING, Any, Callable, Literal, NamedTuple, Protocol, TypeAlias, TypeGuard

from typing_extensions import Sequence

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
    deleted: bool

    def read_content(self) -> str | bytes: ...

    def write_content(self, content: str | bytes) -> None: ...

    def delete(self) -> None: ...


Tree: TypeAlias = 'dict[str, AbstractFile | Tree]'


def _is_file(entry: None | AbstractFile | Tree) -> TypeGuard[AbstractFile]:
    return hasattr(entry, 'path')


def _is_dir(entry: None | AbstractFile | Tree) -> TypeGuard[Tree]:
    return isinstance(entry, dict)


class MemoryFile:
    path: PurePosixPath
    name: str
    content: str | bytes
    permissions: int = 0o644
    deleted: bool

    def __init__(self, path: str | PurePosixPath, content: str | bytes, *, permissions: int = 0o644) -> None:
        self.path = PurePosixPath(path)
        self.name = self.path.name
        self.content = content
        self.permissions = permissions
        self.deleted = False

    def read_content(self) -> str | bytes:
        return self.content

    def write_content(self, content: str | bytes) -> None:
        self.content = content

    def delete(self) -> None:
        self.deleted = True

    def __repr__(self) -> str:
        repr_content = "'...'" if isinstance(self.content, str) else "b'...'"
        return f'MemoryFile(path={self.path}, content={repr_content}, permissions={self.permissions})'


_type_check_memory_file: AbstractFile = MemoryFile('test.txt', '')


class CallbackFile:
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
        self.path = PurePosixPath(path)
        self.name = self.path.name
        self.read = read
        self.write = write
        self.permissions = permissions
        self.deleted = False

    def read_content(self) -> str | bytes:
        return self.read(self.path)

    def write_content(self, content: str | bytes) -> None:
        self.write(self.path, content)

    def delete(self) -> None:
        self.deleted = True

    def __repr__(self) -> str:
        return f'CallbackFile(path={self.path}, read={self.read}, write={self.write}, permissions={self.permissions})'


_type_check_callback_file: AbstractFile = CallbackFile('test.txt', lambda _: '', lambda _, __: None)


class OSAccess(AbstractOS):
    """High level type for giving Monty access to a pseudo OS."""

    files: list[AbstractFile]
    environ: dict[str, str]
    _tree: Tree

    def __init__(self, files: Sequence[AbstractFile] | None = None, environ: dict[str, str] | None = None):
        self.files = list(files) if files else []
        self.environ = environ or {}
        self._tree = {}
        for file in self.files:
            if not file.path.is_absolute():
                raise ValueError(f'Files must have absolute paths, {file.path} is not absolute')

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

    def path_exists(self, path: str) -> bool:
        return self._get_entry(path) is not None

    def path_is_file(self, path: str) -> bool:
        return _is_file(self._get_entry(path))

    def path_is_dir(self, path: str) -> bool:
        return _is_dir(self._get_entry(path))

    def path_is_symlink(self, path: str) -> bool:
        return False

    def path_read_text(self, path: str) -> str:
        file = self._get_file(path)
        content = file.read_content()
        return content if isinstance(content, str) else content.decode()

    def path_read_bytes(self, path: str) -> bytes:
        file = self._get_file(path)
        content = file.read_content()
        return content if isinstance(content, bytes) else content.encode()

    def path_write_text(self, path: str, data: str) -> None:
        self._write_file(path, data)

    def path_write_bytes(self, path: str, data: bytes) -> None:
        self._write_file(path, data)

    def _write_file(self, path: str, data: bytes | str) -> None:
        entry = self._get_entry(path)
        if _is_file(entry):
            entry.write_content(data)
        elif _is_dir(entry):
            raise IsADirectoryError(f'[Errno 21] Is a directory: {path!r}')

        # write a new file if the parent directory exists
        parent_entry = self._parent_entry(path)
        if _is_dir(parent_entry):
            file_path = PurePosixPath(path)
            parent_entry[file_path.name] = new_file = MemoryFile(file_path, data)
            self.files.append(new_file)
        else:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {path!r}')

    def path_mkdir(self, path: str, parents: bool, exist_ok: bool) -> None:
        entry = self._get_entry(path)
        if _is_file(entry):
            raise FileExistsError(f'[Errno 17] File exists: {path!r}')
        elif _is_dir(entry):
            if exist_ok:
                return
            else:
                raise FileExistsError(f'[Errno 17] File exists: {path!r}')

        parent_entry = self._parent_entry(path)
        if _is_dir(parent_entry):
            parent_entry[PurePosixPath(path).name] = {}
            return
        elif _is_file(parent_entry):
            raise NotADirectoryError(f'[Errno 20] Not a directory: {path!r}')
        elif parents:
            subtree = self._tree
            for part in PurePosixPath(path).parts:
                entry = subtree.setdefault(part, {})
                if _is_dir(entry):
                    subtree = entry
                else:
                    raise NotADirectoryError(f'[Errno 20] Not a directory: {path!r}')
        else:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {path!r}')

    def path_unlink(self, path: str) -> None:
        file = self._get_file(path)
        file.delete()
        # remove from parent
        parent_dir = self._parent_entry(path)
        assert _is_dir(parent_dir), f'Expected parent of a file to always be a directory, got {parent_dir}'
        del parent_dir[file.name]

    def path_rmdir(self, path: str) -> None:
        dir = self._get_dir(path)
        if dir:
            raise OSError(f'[Errno 39] Directory not empty: {path!r}')
        # remove from parent
        parent_dir = self._parent_entry(path)
        assert _is_dir(parent_dir), f'Expected parent of a file to always be a directory, got {parent_dir}'
        del parent_dir[PurePosixPath(path).name]

    def path_iterdir(self, path: str) -> list[str]:
        return list(self._get_dir(path).keys())

    def path_stat(self, path: str) -> StatResult:
        entry = self._get_entry_exists(path)
        if _is_file(entry):
            content = entry.read_content()
            size = len(content) if isinstance(content, bytes) else len(content.encode())
            return StatResult.file_stat(size=size, mode=entry.permissions)
        else:
            return StatResult.dir_stat()

    def path_rename(self, path: str, target: str) -> None:
        src_entry = self._get_entry(path)
        if src_entry is None:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {path} -> {target}')

        parent_dir = self._parent_entry(path)
        assert _is_dir(parent_dir), f'Expected parent of a file to always be a directory, got {parent_dir}'

        target_parent = self._parent_entry(target)
        if not _is_dir(target_parent):
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {path} -> {target}')
        target_entry = self._get_entry(target)

        if _is_file(src_entry):
            if _is_dir(target_entry):
                raise IsADirectoryError(f'[Errno 21] Is a directory: {path} -> {target}')
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
                raise NotADirectoryError(f'[Errno 20] Not a directory: {path} -> {target}')
            elif _is_dir(target_entry) and target_entry:
                raise OSError(f'[Errno 66] Directory not empty: {path} -> {target}')

            src_name = PurePosixPath(path).name
            target_name = PurePosixPath(target).name
            # remove it from the old directory
            del parent_dir[src_name]
            # and put it in the new directory
            target_parent[target_name] = src_entry

            # Update paths for all files in the renamed directory
            self._update_paths_recursive(src_entry, PurePosixPath(path), PurePosixPath(target))

    def path_resolve(self, path: str) -> str:
        # No symlinks in OSAccess, so resolve is same as absolute with normalization
        return self.path_absolute(path)

    def path_absolute(self, path: str) -> str:
        p = PurePosixPath(path)
        if p.is_absolute():
            return str(p)
        # In this virtual filesystem, we treat '/' as the working directory
        return str(PurePosixPath('/') / p)

    def getenv(self, key: str, default: str | None = None) -> str | None:
        return self.environ.get(key, default)

    def _get_entry(self, path: str) -> Tree | AbstractFile | None:
        dir = self._tree

        *dir_parts, name = PurePosixPath(path).parts

        for part in dir_parts:
            entry = dir.get(part)
            if _is_dir(entry):
                dir = entry
            else:
                return None

        return dir.get(name)

    def _get_entry_exists(self, path: str) -> Tree | AbstractFile:
        entry = self._get_entry(path)
        if entry is None:
            raise FileNotFoundError(f'[Errno 2] No such file or directory: {path!r}')
        else:
            return entry

    def _get_file(self, path: str) -> AbstractFile:
        entry = self._get_entry_exists(path)
        if _is_file(entry):
            return entry
        else:
            raise IsADirectoryError(f'[Errno 21] Is a directory: {path!r}')

    def _get_dir(self, path: str) -> Tree:
        entry = self._get_entry_exists(path)
        if _is_dir(entry):
            return entry
        else:
            raise NotADirectoryError(f'[Errno 20] Not a directory: {path!r}')

    def _parent_entry(self, path: str) -> Tree | AbstractFile | None:
        return self._get_entry(str(PurePosixPath(path).parent))

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
