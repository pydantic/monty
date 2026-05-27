"""
Shared CPython-side fixtures for the monty-datatest harness.

Holds two unrelated bundles of helpers that are both injected into CPython
test globals via the `exported_globals` dict at the bottom of this file:

1. External-function implementations for iter mode tests (the historical
   contents of this file), which mirror `dispatch_external_call` in the
   Rust runner so iter-mode tests produce identical results on both sides.
2. The `_test_cm` synthetic context manager — a CPython equivalent of
   Monty's `test-hooks` builtin (see `crates/monty/src/types/test_cm.rs`).
   **REMOVE** the `_TestCm` class, the `test_cm` wrapper, and the
   `'_test_cm'` entry in `exported_globals` once a real production
   context manager covers the same paths.

This module is shared between:
- scripts/run_traceback.py (for traceback tests)
- crates/monty-datatest/src/main.rs (imported via pyo3 for CPython execution)
"""

from __future__ import annotations

import asyncio
import os
import stat as stat_module
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType


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

    def sum(self) -> int:
        return self.x + self.y

    def add(self, dx: int, dy: int) -> 'Point':
        return Point(x=self.x + dx, y=self.y + dy)

    def scale(self, factor: int) -> 'Point':
        return Point(x=self.x * factor, y=self.y * factor)

    def describe(self, label: str = 'point') -> str:
        return f'{label}({self.x}, {self.y})'


def make_point() -> Point:
    return Point(x=1, y=2)


@dataclass
class MutablePoint:
    x: int
    y: int

    def sum(self) -> int:
        return self.x + self.y

    def shift(self, dx: int, dy: int) -> None:
        self.x += dx
        self.y += dy


def make_mutable_point() -> MutablePoint:
    return MutablePoint(x=1, y=2)


@dataclass(frozen=True)
class User:
    name: str
    active: bool = True

    def greeting(self) -> str:
        return f'Hello, {self.name}!'


def make_user(name: str) -> User:
    return User(name=name, active=True)


@dataclass
class Empty:
    pass


def make_empty() -> Empty:
    return Empty()


# Non-function constants for NameLookup tests.
# These mirror the values in the Rust test runner's NameLookup handler.
CONST_INT = 42
CONST_STR = 'hello'
CONST_FLOAT = 3.14
CONST_BOOL = True
CONST_LIST = [1, 2, 3]
CONST_NONE = None


def async_call(x: object) -> 'asyncio.Future[object]':
    """Returns a resolved `asyncio.Future` of the given value.

    Mirrors Monty's host-managed `ExternalFuture`: awaiting returns `x` and
    re-awaiting returns the same cached `x` (matching `Future` semantics in
    both runtimes). Implemented as a `Future` rather than `async def` so
    callers can re-await without raising "cannot reuse already awaited
    coroutine".
    """
    fut: asyncio.Future[object] = asyncio.get_running_loop().create_future()
    fut.set_result(x)
    return fut


def async_fail(exc_type: str, message: str) -> 'asyncio.Future[None]':
    """Returns a Future that raises `exc_type(message)` when awaited.

    Mirrors `raise_error` for the async path. Returning a Future (rather
    than a coroutine raising the exception in its body) lets re-await
    replay the cached exception, matching Monty's `ExternalFuture::Failed`
    behaviour.
    """
    exc_types: dict[str, type[Exception]] = {
        'ValueError': ValueError,
        'TypeError': TypeError,
        'KeyError': KeyError,
        'RuntimeError': RuntimeError,
    }
    fut: asyncio.Future[None] = asyncio.get_running_loop().create_future()
    fut.set_exception(exc_types[exc_type](message))
    return fut


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

    def write_text(
        self,
        data: str,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
    ) -> int:
        path_str = str(self)
        if is_virtual_path(path_str):
            content = data.encode(encoding or 'utf-8')
            VIRTUAL_FILES[path_str] = (content, 0o644)
            # Add to parent directory contents
            _add_to_parent_dir(path_str)
            return len(content)
        return super().write_text(data, encoding=encoding, errors=errors, newline=newline)

    def write_bytes(self, data: bytes) -> int:  # pyright: ignore[reportIncompatibleMethodOverride]
        path_str = str(self)
        if is_virtual_path(path_str):
            VIRTUAL_FILES[path_str] = (data, 0o644)
            # Add to parent directory contents
            _add_to_parent_dir(path_str)
            return len(data)
        return super().write_bytes(data)

    def mkdir(self, mode: int = 0o777, parents: bool = False, exist_ok: bool = False) -> None:
        path_str = str(self)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_DIRS:
                if exist_ok:
                    return
                raise FileExistsError(17, 'File exists', path_str)
            if path_str in VIRTUAL_FILES:
                raise FileExistsError(17, 'File exists', path_str)

            # Check if parent exists
            parent_str = str(self.parent)
            if parent_str and parent_str not in VIRTUAL_DIRS:
                if parents:
                    VirtualPath(parent_str).mkdir(mode=mode, parents=True, exist_ok=True)
                else:
                    raise FileNotFoundError(2, 'No such file or directory', path_str)

            VIRTUAL_DIRS.add(path_str)
            _add_to_parent_dir(path_str)
            # Initialize empty directory contents
            if path_str not in VIRTUAL_DIR_CONTENTS:
                VIRTUAL_DIR_CONTENTS[path_str] = []
            return
        super().mkdir(mode=mode, parents=parents, exist_ok=exist_ok)

    def unlink(self, missing_ok: bool = False) -> None:
        path_str = str(self)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_FILES:
                del VIRTUAL_FILES[path_str]
                _remove_from_parent_dir(path_str)
                return
            if not missing_ok:
                raise FileNotFoundError(2, 'No such file or directory', path_str)
            return
        super().unlink(missing_ok=missing_ok)

    def rmdir(self) -> None:
        path_str = str(self)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_DIRS:
                VIRTUAL_DIRS.remove(path_str)
                if path_str in VIRTUAL_DIR_CONTENTS:
                    del VIRTUAL_DIR_CONTENTS[path_str]
                _remove_from_parent_dir(path_str)
                return
            raise FileNotFoundError(2, 'No such file or directory', path_str)
        super().rmdir()

    def rename(self, target: 'VirtualPath | str') -> 'VirtualPath':  # pyright: ignore[reportIncompatibleMethodOverride]
        path_str = str(self)
        target_str = str(target)
        if is_virtual_path(path_str):
            if path_str in VIRTUAL_FILES:
                content, mode = VIRTUAL_FILES[path_str]
                del VIRTUAL_FILES[path_str]
                _remove_from_parent_dir(path_str)
                VIRTUAL_FILES[target_str] = (content, mode)
                _add_to_parent_dir(target_str)
                return VirtualPath(target_str)
            if path_str in VIRTUAL_DIRS:
                VIRTUAL_DIRS.remove(path_str)
                _remove_from_parent_dir(path_str)
                VIRTUAL_DIRS.add(target_str)
                _add_to_parent_dir(target_str)
                return VirtualPath(target_str)
            raise FileNotFoundError(2, 'No such file or directory', path_str)
        return VirtualPath(super().rename(target))

    # __truediv__ is NOT overridden - the parent class already uses type(self)
    # to create new paths, which will be VirtualPath instances


def _add_to_parent_dir(path_str: str) -> None:
    """Add a path to its parent directory's contents."""
    parent = str(Path(path_str).parent)
    if parent in VIRTUAL_DIR_CONTENTS:
        if path_str not in VIRTUAL_DIR_CONTENTS[parent]:
            VIRTUAL_DIR_CONTENTS[parent].append(path_str)


def _remove_from_parent_dir(path_str: str) -> None:
    """Remove a path from its parent directory's contents."""
    parent = str(Path(path_str).parent)
    if parent in VIRTUAL_DIR_CONTENTS and path_str in VIRTUAL_DIR_CONTENTS[parent]:
        VIRTUAL_DIR_CONTENTS[parent].remove(path_str)


# Monkey-patch pathlib.Path to use VirtualPath
# This is done so tests can use `from pathlib import Path` and get VirtualPath behavior
_original_path_new = Path.__new__


def _virtual_path_new(cls: type[Path], *args: object, **kwargs: object) -> Path:
    """Custom __new__ that returns VirtualPath for paths starting with /virtual or /nonexistent.

    Only virtual paths get the VirtualPath treatment. All other paths use the
    standard pathlib behavior (PosixPath/WindowsPath).

    We must also handle ``cls is VirtualPath`` (not just ``cls is Path``)
    because pathlib internally calls ``type(self)(*pathsegments)`` from
    methods like ``with_segments`` / ``parent``, which re-enters this
    patched ``__new__`` with the subclass as *cls*.  Without this guard
    the fallback to ``_original_path_new`` triggers infinite recursion in
    Python 3.14+.
    """
    if args and isinstance(args[0], str):
        path_str = args[0]
        if path_str.startswith('/virtual') or path_str.startswith('/nonexistent'):
            return object.__new__(VirtualPath)
    if issubclass(cls, VirtualPath):
        return object.__new__(VirtualPath)
    return _original_path_new(cls, *args, **kwargs)  # pyright: ignore[reportArgumentType]


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

# Store original os functions before monkey-patching
# Check if already patched (happens when module is re-executed in same interpreter)
if not hasattr(os, '_monty_original_getenv'):
    os._monty_original_getenv = os.getenv  # pyright: ignore[reportAttributeAccessIssue]
    os._monty_original_environ = os.environ  # pyright: ignore[reportAttributeAccessIssue]

_original_getenv = os._monty_original_getenv  # pyright: ignore[reportAttributeAccessIssue,reportUnknownVariableType,reportUnknownMemberType]
_original_environ = os._monty_original_environ  # pyright: ignore[reportAttributeAccessIssue,reportUnknownVariableType,reportUnknownMemberType]


def _virtual_getenv(key: str, default: str | None = None) -> str | None:
    """Virtual os.getenv that returns predefined values for VIRTUAL_* keys.

    For keys starting with 'VIRTUAL_', returns the virtual environment value
    or None if not in the virtual env (ignoring default for these keys to match Monty behavior).
    For all other keys, falls through to the real os.getenv.
    """
    # Check key type first to match CPython's behavior
    if not isinstance(key, str):  # pyright: ignore[reportUnnecessaryIsInstance]
        # to get the real error
        return _original_getenv(key)  # pyright: ignore[reportUnknownVariableType]

    if key.startswith('VIRTUAL_') or key in ('NONEXISTENT', 'ALSO_MISSING', 'MISSING'):
        value = VIRTUAL_ENV.get(key)
        if value is not None:
            return value
        return default
    return _original_getenv(key, default)  # pyright: ignore[reportUnknownVariableType]


# Monkey-patch os.getenv to use virtual environment for test keys
os.getenv = _virtual_getenv


class VirtualEnviron:
    """Wrapper around os.environ that provides virtual environment variables.

    For keys in VIRTUAL_ENV or test-specific keys (NONEXISTENT, etc.), returns
    virtual values. For all other keys, falls through to real os.environ.

    This ensures tests using `os.environ['VIRTUAL_HOME']` work identically
    in both Monty (virtual env) and CPython (real env + virtual overlay).
    """

    def __getitem__(self, key: str) -> str:
        if key in VIRTUAL_ENV:
            return VIRTUAL_ENV[key]
        if key.startswith('VIRTUAL_') or key in ('NONEXISTENT', 'ALSO_MISSING', 'MISSING'):
            raise KeyError(key)
        return _original_environ[key]  # pyright: ignore[reportUnknownVariableType]

    def __contains__(self, key: object) -> bool:
        if isinstance(key, str):
            if key in VIRTUAL_ENV:
                return True
            if key.startswith('VIRTUAL_') or key in ('NONEXISTENT', 'ALSO_MISSING', 'MISSING'):
                return False
        return key in _original_environ

    def __len__(self) -> int:
        # Return only virtual env length for tests that check len(os.environ)
        return len(VIRTUAL_ENV)

    def get(self, key: str, default: str | None = None) -> str | None:
        # Check key type first - pass through to original environ to get proper error
        if not isinstance(key, str):  # pyright: ignore[reportUnnecessaryIsInstance]
            return _original_environ.get(key, default)  # pyright: ignore[reportArgumentType,reportUnknownMemberType,reportUnknownVariableType]
        if key in VIRTUAL_ENV:
            return VIRTUAL_ENV[key]
        if key.startswith('VIRTUAL_') or key in ('NONEXISTENT', 'ALSO_MISSING', 'MISSING'):
            return default
        return _original_environ.get(key, default)  # pyright: ignore[reportUnknownMemberType,reportUnknownVariableType]

    def keys(self):
        """Return keys from virtual environment only (for test isolation)."""
        return VIRTUAL_ENV.keys()

    def values(self):
        """Return values from virtual environment only (for test isolation)."""
        return VIRTUAL_ENV.values()

    def items(self):
        """Return items from virtual environment only (for test isolation)."""
        return VIRTUAL_ENV.items()


# Monkey-patch os.environ to use virtual environment for test keys
os.environ = VirtualEnviron()


# =============================================================================
# Synthetic context manager — CPython mirror of Monty's `_test_cm()` test-hook
# builtin. **Remove** alongside `crates/monty/src/types/test_cm.rs` once a
# real production context manager covers the same paths.
# =============================================================================


class _TestCm:
    """Synthetic context manager — see `crates/monty/src/types/test_cm.rs`.

    Mirrors the Monty-side `TestContextManager` semantics exactly:

    | behavior          | payload | effect                                              |
    | ----------------- | ------- | --------------------------------------------------- |
    | (none)            | —       | passthrough: returns self on enter, None on exit    |
    | `"suppress"`      | (none)  | `__exit__` returns True on the exception path       |
    | `"enter_value"`   | int     | `__enter__` returns the int instead of self         |
    | `"raise_on_enter"`| str     | `__enter__` raises `ValueError(payload)`            |
    | `"raise_on_exit"` | str     | `__exit__` raises `ValueError(payload)`             |

    Type validation matches Monty's error messages so traceback-equivalence
    tests pass on both sides.
    """

    __slots__ = ('_behavior', '_payload')

    def __init__(self, behavior: object = None, payload: object = None) -> None:
        if behavior is not None and not isinstance(behavior, str):
            # Match the Monty-side TypeError text exactly so error-path
            # tests pass against both interpreters.
            raise TypeError(f'_test_cm() behavior must be str, not {type(behavior).__name__}')
        if behavior is None:
            if payload is not None:
                raise TypeError('_test_cm() payload requires a leading behavior argument')
            self._behavior: str | None = None
            self._payload: object = None
            return

        if behavior == 'suppress':
            if payload is not None:
                raise TypeError("_test_cm('suppress') takes no payload")
        elif behavior == 'enter_value':
            if payload is None:
                raise TypeError("_test_cm('enter_value', n) requires an int payload")
            # bool is a subclass of int in Python — reject it to keep
            # parity with Monty, where `Value::Bool` and `Value::Int` are
            # distinct variants and only Int is accepted.
            if not isinstance(payload, int) or isinstance(payload, bool):
                raise TypeError(f"_test_cm('enter_value', n) requires int payload, not {type(payload).__name__}")
        elif behavior in ('raise_on_enter', 'raise_on_exit'):
            if payload is None:
                raise TypeError(f"_test_cm('{behavior}', msg) requires a str payload")
            if not isinstance(payload, str):
                raise TypeError(f"_test_cm('{behavior}', msg) requires str payload, not {type(payload).__name__}")
        else:
            raise TypeError(f"_test_cm() unknown behavior '{behavior}'")

        self._behavior = behavior
        self._payload = payload

    def __enter__(self) -> object:
        if self._behavior == 'raise_on_enter':
            assert isinstance(self._payload, str)
            raise ValueError(self._payload)
        if self._behavior == 'enter_value':
            return self._payload
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> bool | None:
        if self._behavior == 'raise_on_exit':
            assert isinstance(self._payload, str)
            raise ValueError(self._payload)
        # The suppress flag only matters when an exception is propagating;
        # CPython ignores the return value on the normal-exit path. The
        # Monty implementation always returns Bool(False)/None on the
        # normal path for the same reason.
        if self._behavior == 'suppress' and exc_type is not None:
            return True
        return None


def test_cm(*args: object) -> _TestCm:
    """Mirrors the Monty `_test_cm()` builtin's positional-only signature.

    Exposed in `exported_globals` under the underscore-prefixed name
    `_test_cm` to match the Monty builtin.
    """
    if len(args) <= 2:
        return _TestCm(*args)
    raise TypeError(f'_test_cm() takes at most 2 positional arguments ({len(args)} given)')


# =============================================================================
# Names exported into every CPython test's globals.
# =============================================================================
#
# Both the iter-mode helpers and the synthetic context manager live here.
# The iter-mode helpers don't strictly need to be visible in non-iter-mode
# tests, but the names are unlikely to collide and the simpler "inject one
# dict for every test" model removes a branch in the test runner. Tests
# that don't use these names see no behavioral difference.
exported_globals: dict[str, object] = {
    # Iter-mode external function implementations.
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
    'async_fail': async_fail,
    # Non-function constants resolved by the Rust runner's NameLookup
    # handler. Tests reference these by bare name, so they must be
    # injected into the CPython test's globals to keep parity.
    'CONST_INT': CONST_INT,
    'CONST_STR': CONST_STR,
    'CONST_FLOAT': CONST_FLOAT,
    'CONST_BOOL': CONST_BOOL,
    'CONST_LIST': CONST_LIST,
    'CONST_NONE': CONST_NONE,
    # Synthetic context manager — see the "REMOVE THIS" note at the top.
    '_test_cm': test_cm,
}
