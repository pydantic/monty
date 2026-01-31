"""
External function implementations for iter mode tests.

These implementations mirror the behavior of `dispatch_external_call` in the Rust test runner
so that iter mode tests produce identical results in both Monty and CPython.

This module is shared between:
- scripts/run_traceback.py (for traceback tests)
- crates/monty/tests/datatest_runner.rs (via include_str! for CPython execution)
"""

from __future__ import annotations

import os
import stat as stat_module
from dataclasses import dataclass
from pathlib import Path


def add_ints(a: int, b: int) -> int:
    return a + b


def concat_strings(a: str, b: str) -> str:
    return a + b


def return_value(x: object) -> object:
    return x


def get_list() -> list[int]:
    return [1, 2, 3]


def raise_error(exc_type: str, message: str) -> None:
    exc_types: dict[str, type[Exception]] = {
        'ValueError': ValueError,
        'TypeError': TypeError,
        'KeyError': KeyError,
        'RuntimeError': RuntimeError,
    }
    raise exc_types[exc_type](message)


@dataclass(frozen=True)
class Point:
    x: int
    y: int


def make_point() -> Point:
    return Point(x=1, y=2)


@dataclass
class MutablePoint:
    x: int
    y: int


def make_mutable_point() -> MutablePoint:
    return MutablePoint(x=1, y=2)


@dataclass(frozen=True)
class User:
    name: str
    active: bool = True


def make_user(name: str) -> User:
    return User(name=name, active=True)


@dataclass
class Empty:
    pass


def make_empty() -> Empty:
    return Empty()


async def async_call(x: object) -> object:
    """Async function that returns its argument.

    This is a coroutine - it returns a future that resolves to the given value.
    Used for testing async external function calls.
    """
    return x


# =============================================================================
# Virtual Filesystem for OS Call Tests
# =============================================================================

# Virtual filesystem modification time (matches Rust constant)
VFS_MTIME: float = 1700000000.0

# Virtual files: path -> (content, mode)
VIRTUAL_FILES: dict[str, tuple[bytes, int]] = {
    '/virtual/file.txt': (b'hello world\n', 0o644),
    '/virtual/data.bin': (b'\x00\x01\x02\x03', 0o644),
    '/virtual/empty.txt': (b'', 0o644),
    '/virtual/subdir/nested.txt': (b'nested content', 0o644),
    '/virtual/subdir/deep/file.txt': (b'deep', 0o644),
    '/virtual/readonly.txt': (b'readonly', 0o444),
}

# Virtual directories
VIRTUAL_DIRS: set[str] = {'/virtual', '/virtual/subdir', '/virtual/subdir/deep'}

# Directory contents: parent_path -> list of child paths
VIRTUAL_DIR_CONTENTS: dict[str, list[str]] = {
    '/virtual': [
        '/virtual/file.txt',
        '/virtual/data.bin',
        '/virtual/empty.txt',
        '/virtual/subdir',
        '/virtual/readonly.txt',
    ],
    '/virtual/subdir': ['/virtual/subdir/nested.txt', '/virtual/subdir/deep'],
    '/virtual/subdir/deep': ['/virtual/subdir/deep/file.txt'],
}


class VirtualStatResult:
    """Mock stat_result for virtual filesystem.

    Mimics os.stat_result structure with named attributes and index access.
    """

    def __init__(self, st_mode: int, st_size: int):
        self.st_mode = st_mode
        self.st_ino = 0
        self.st_dev = 0
        # nlink is 1 for files, 2 for directories
        self.st_nlink = 1 if stat_module.S_ISREG(st_mode) else 2
        self.st_uid = 0
        self.st_gid = 0
        self.st_size = st_size
        self.st_atime = VFS_MTIME
        self.st_mtime = VFS_MTIME
        self.st_ctime = VFS_MTIME

    def __getitem__(self, index: int) -> int | float:
        """Support index access like real stat_result."""
        fields = [
            self.st_mode,
            self.st_ino,
            self.st_dev,
            self.st_nlink,
            self.st_uid,
            self.st_gid,
            self.st_size,
            self.st_atime,
            self.st_mtime,
            self.st_ctime,
        ]
        return fields[index]


def is_virtual_path(path: str) -> bool:
    """Check if a path should use the virtual filesystem."""
    return path.startswith('/virtual') or path.startswith('/nonexistent')


class VirtualPath(type(Path())):
    """Path subclass that uses virtual filesystem for /virtual/ and /nonexistent paths.

    Inherits from the concrete Path class (PosixPath or WindowsPath) and overrides
    filesystem methods to use the virtual filesystem when appropriate.
    """

    def exists(self, *, follow_symlinks: bool = True) -> bool:
        path_str = str(self)
        if is_virtual_path(path_str):
            return path_str in VIRTUAL_FILES or path_str in VIRTUAL_DIRS
        return super().exists(follow_symlinks=follow_symlinks)

    def is_file(self, *, follow_symlinks: bool = True) -> bool:
        path_str = str(self)
        if is_virtual_path(path_str):
            return path_str in VIRTUAL_FILES
        return super().is_file(follow_symlinks=follow_symlinks)

    def is_dir(self, *, follow_symlinks: bool = True) -> bool:
        path_str = str(self)
        if is_virtual_path(path_str):
            return path_str in VIRTUAL_DIRS
        return super().is_dir(follow_symlinks=follow_symlinks)

    def is_symlink(self) -> bool:
        path_str = str(self)
        if is_virtual_path(path_str):
            return False  # No symlinks in virtual fs
        return super().is_symlink()

    def read_text(self, encoding: str | None = None, errors: str | None = None, newline: str | None = None) -> str:
        path_str = str(self)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_FILES:
                content, _ = VIRTUAL_FILES[path_str]
                return content.decode('utf-8')
            raise FileNotFoundError(2, 'No such file or directory', path_str)
        return super().read_text(encoding=encoding, errors=errors, newline=newline)

    def read_bytes(self) -> bytes:
        path_str = str(self)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_FILES:
                content, _ = VIRTUAL_FILES[path_str]
                return content
            raise FileNotFoundError(2, 'No such file or directory', path_str)
        return super().read_bytes()

    def stat(  # pyright: ignore[reportIncompatibleMethodOverride]
        self, *, follow_symlinks: bool = True
    ) -> VirtualStatResult | os.stat_result:
        path_str = str(self)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_FILES:
                content, mode = VIRTUAL_FILES[path_str]
                # Add regular file type bits
                st_mode = mode | stat_module.S_IFREG
                return VirtualStatResult(st_mode, len(content))
            if path_str in VIRTUAL_DIRS:
                # Directory: 0o755 with directory type bits
                st_mode = 0o755 | stat_module.S_IFDIR
                return VirtualStatResult(st_mode, 4096)
            raise FileNotFoundError(2, 'No such file or directory', path_str)
        return super().stat(follow_symlinks=follow_symlinks)

    def iterdir(self):  # pyright: ignore[reportUnknownParameterType]
        path_str = str(self)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_DIR_CONTENTS:
                for child_path in VIRTUAL_DIR_CONTENTS[path_str]:
                    yield VirtualPath(child_path)
                return
            raise FileNotFoundError(2, 'No such file or directory', path_str)
        yield from super().iterdir()

    def resolve(self, strict: bool = False) -> 'VirtualPath':
        path_str = str(self)
        if is_virtual_path(path_str):
            # For virtual paths, just return as-is (already absolute)
            return VirtualPath(path_str)
        return VirtualPath(super().resolve(strict=strict))

    def absolute(self) -> 'VirtualPath':
        path_str = str(self)
        if is_virtual_path(path_str):
            # For virtual paths, return as-is
            return VirtualPath(path_str)
        return VirtualPath(super().absolute())

    # __truediv__ is NOT overridden - the parent class already uses type(self)
    # to create new paths, which will be VirtualPath instances


# Monkey-patch pathlib.Path to use VirtualPath
# This is done so tests can use `from pathlib import Path` and get VirtualPath behavior
_original_path_new = Path.__new__


def _virtual_path_new(cls: type, *args: object, **kwargs: object) -> Path:
    """Custom __new__ that returns VirtualPath for paths starting with /virtual or /nonexistent.

    Only virtual paths get the VirtualPath treatment. All other paths use the
    standard pathlib behavior (PosixPath/WindowsPath).
    """
    if cls is Path and args and isinstance(args[0], str):
        path_str = args[0]
        if path_str.startswith('/virtual') or path_str.startswith('/nonexistent'):
            return object.__new__(VirtualPath)
    return _original_path_new(cls, *args, **kwargs)  # pyright: ignore[reportUnknownVariableType,reportArgumentType]


# Apply the monkey-patch
Path.__new__ = _virtual_path_new


# =============================================================================
# Virtual Environment for os.getenv Tests
# =============================================================================

# Virtual environment variables (matches Rust test constants)
VIRTUAL_ENV: dict[str, str] = {
    'VIRTUAL_HOME': '/virtual/home',
    'VIRTUAL_USER': 'testuser',
    'VIRTUAL_EMPTY': '',
}

# Store the original os.getenv function
_original_getenv = os.getenv


def _virtual_getenv(key: str, default: str | None = None) -> str | None:
    """Virtual os.getenv that returns predefined values for VIRTUAL_* keys.

    For keys starting with 'VIRTUAL_', returns the virtual environment value
    or None if not in the virtual env (ignoring default for these keys to match Monty behavior).
    For all other keys, falls through to the real os.getenv.
    """
    # Check key type first to match CPython's behavior
    if not isinstance(key, str):  # pyright: ignore[reportUnnecessaryIsInstance]
        # to get the real error
        return _original_getenv(key)

    if key.startswith('VIRTUAL_') or key in ('NONEXISTENT', 'ALSO_MISSING', 'MISSING'):
        value = VIRTUAL_ENV.get(key)
        if value is not None:
            return value
        return default
    return _original_getenv(key, default)


# Monkey-patch os.getenv to use virtual environment for test keys
os.getenv = _virtual_getenv


# All external functions available to iter mode tests
ITER_MODE_GLOBALS: dict[str, object] = {
    'add_ints': add_ints,
    'concat_strings': concat_strings,
    'return_value': return_value,
    'get_list': get_list,
    'raise_error': raise_error,
    'make_point': make_point,
    'make_mutable_point': make_mutable_point,
    'make_user': make_user,
    'make_empty': make_empty,
    'async_call': async_call,
}
