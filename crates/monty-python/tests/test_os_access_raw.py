"""Tests for AbstractFileSystem implementation.

These tests verify that AbstractFileSystem can be subclassed to provide
a virtual filesystem that Monty code can interact with via Path methods.
"""

from pathlib import PurePosixPath

import pytest
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import AbstractOS, StatResult


class TestOS(AbstractOS):
    """A simple in-memory filesystem for testing."""

    __test__ = False

    def __init__(self) -> None:
        self.files: dict[str, bytes] = {}
        self.directories: set[str] = {'/'}

    def _ensure_parent_exists(self, path: str) -> None:
        """Ensure all parent directories exist."""
        parts = path.rstrip('/').split('/')
        for i in range(1, len(parts)):
            parent = '/'.join(parts[:i]) or '/'
            self.directories.add(parent)

    def path_exists(self, path: str) -> bool:
        return path in self.files or path in self.directories

    def path_is_file(self, path: str) -> bool:
        return path in self.files

    def path_is_dir(self, path: str) -> bool:
        return path in self.directories

    def path_is_symlink(self, path: str) -> bool:
        return False  # No symlink support in this simple implementation

    def path_read_text(self, path: str) -> str:
        if path not in self.files:
            raise FileNotFoundError(f'No such file: {path}')
        return self.files[path].decode('utf-8')

    def path_read_bytes(self, path: str) -> bytes:
        if path not in self.files:
            raise FileNotFoundError(f'No such file: {path}')
        return self.files[path]

    def path_write_text(self, path: str, data: str) -> None:
        self._ensure_parent_exists(path)
        self.files[path] = data.encode('utf-8')

    def path_write_bytes(self, path: str, data: bytes) -> None:
        self._ensure_parent_exists(path)
        self.files[path] = data

    def path_mkdir(self, path: str, parents: bool, exist_ok: bool) -> None:
        if path in self.directories:
            if not exist_ok:
                raise FileExistsError(f'Directory exists: {path}')
            return
        if parents:
            self._ensure_parent_exists(path)
        self.directories.add(path)

    def path_unlink(self, path: str) -> None:
        if path not in self.files:
            raise FileNotFoundError(f'No such file: {path}')
        del self.files[path]

    def path_rmdir(self, path: str) -> None:
        if path not in self.directories:
            raise FileNotFoundError(f'No such directory: {path}')
        # Check if directory is empty
        for f in self.files:
            if f.startswith(path + '/'):
                raise OSError(f'Directory not empty: {path}')
        for d in self.directories:
            if d != path and d.startswith(path + '/'):
                raise OSError(f'Directory not empty: {path}')
        self.directories.remove(path)

    def path_iterdir(self, path: str) -> list[PurePosixPath]:
        if path not in self.directories:
            raise FileNotFoundError(f'No such directory: {path}')
        result: list[PurePosixPath] = []
        prefix = path.rstrip('/') + '/'
        seen: set[str] = set()
        for f in self.files:
            if f.startswith(prefix):
                # Get immediate child name
                rest = f[len(prefix) :]
                child = rest.split('/')[0]
                if child and child not in seen:
                    seen.add(child)
                    result.append(PurePosixPath(child))
        for d in self.directories:
            if d.startswith(prefix) and d != path:
                rest = d[len(prefix) :]
                child = rest.split('/')[0]
                if child and child not in seen:
                    seen.add(child)
                    result.append(PurePosixPath(child))
        return sorted(result)

    def path_stat(self, path: str) -> StatResult:
        if path in self.files:
            return StatResult.file_stat(len(self.files[path]), 0o644, 0.0)
        elif path in self.directories:
            return StatResult.dir_stat(0o755, 0.0)
        else:
            raise FileNotFoundError(f'No such file or directory: {path}')

    def path_rename(self, path: str, target: str) -> None:
        if path in self.files:
            self._ensure_parent_exists(target)
            self.files[target] = self.files.pop(path)
        elif path in self.directories:
            self._ensure_parent_exists(target)
            self.directories.remove(path)
            self.directories.add(target)
            # Move all files under this directory
            prefix = path.rstrip('/') + '/'
            to_move = [(f, target + f[len(path) :]) for f in self.files if f.startswith(prefix)]
            for old, new in to_move:
                self.files[new] = self.files.pop(old)
        else:
            raise FileNotFoundError(f'No such file or directory: {path}')

    def path_resolve(self, path: str) -> str:
        # Simple implementation: just normalize the path
        parts: list[str] = []
        for part in path.split('/'):
            if part == '..':
                if parts:
                    parts.pop()
            elif part and part != '.':
                parts.append(part)
        return '/' + '/'.join(parts)

    def path_absolute(self, path: str) -> str:
        if path.startswith('/'):
            return path
        return '/' + path

    def getenv(self, key: str, default: str | None = None) -> str | None:
        # Simple virtual environment for testing
        env = {
            'TEST_VAR': 'test_value',
            'HOME': '/test/home',
        }
        return env.get(key, default)


# =============================================================================
# Basic AbstractFileSystem tests
# =============================================================================


def test_abstract_filesystem_exists():
    """AbstractFileSystem.path_exists() works with os."""
    fs = TestOS()
    fs.files['/test.txt'] = b'hello'

    m = pydantic_monty.Monty('from pathlib import Path; Path("/test.txt").exists()')
    result = m.run(os=fs)

    assert result is True


def test_abstract_filesystem_exists_missing():
    """AbstractFileSystem.path_exists() returns False for missing files."""
    fs = TestOS()

    m = pydantic_monty.Monty('from pathlib import Path; Path("/missing.txt").exists()')
    result = m.run(os=fs)

    assert result is False


def test_abstract_filesystem_is_file():
    """AbstractFileSystem.path_is_file() distinguishes files from directories."""
    fs = TestOS()
    fs.files['/file.txt'] = b'content'
    fs.directories.add('/mydir')

    code = """
from pathlib import Path
(Path('/file.txt').is_file(), Path('/mydir').is_file())
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot((True, False))


def test_abstract_filesystem_is_dir():
    """AbstractFileSystem.path_is_dir() distinguishes directories from files."""
    fs = TestOS()
    fs.files['/file.txt'] = b'content'
    fs.directories.add('/mydir')

    code = """
from pathlib import Path
(Path('/file.txt').is_dir(), Path('/mydir').is_dir())
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot((False, True))


def test_abstract_filesystem_read_text():
    """AbstractFileSystem.path_read_text() returns file contents."""
    fs = TestOS()
    fs.files['/hello.txt'] = b'Hello, World!'

    m = pydantic_monty.Monty('from pathlib import Path; Path("/hello.txt").read_text()')
    result = m.run(os=fs)

    assert result == snapshot('Hello, World!')


def test_abstract_filesystem_read_text_missing():
    """AbstractFileSystem.path_read_text() raises FileNotFoundError for missing files."""
    fs = TestOS()

    m = pydantic_monty.Monty('from pathlib import Path; Path("/missing.txt").read_text()')
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(os=fs)
    assert str(exc_info.value) == snapshot('FileNotFoundError: No such file: /missing.txt')
    assert isinstance(exc_info.value.exception(), FileNotFoundError)


def test_abstract_filesystem_read_bytes():
    """AbstractFileSystem.path_read_bytes() returns raw bytes."""
    fs = TestOS()
    fs.files['/data.bin'] = b'\x00\x01\x02\x03'

    m = pydantic_monty.Monty('from pathlib import Path; Path("/data.bin").read_bytes()')
    result = m.run(os=fs)

    assert result == snapshot(b'\x00\x01\x02\x03')


# =============================================================================
# stat() tests
# =============================================================================


def test_abstract_filesystem_stat_file():
    """AbstractFileSystem.path_stat() returns stat result for files."""
    fs = TestOS()
    fs.files['/file.txt'] = b'hello world'

    code = """
from pathlib import Path
s = Path('/file.txt').stat()
(s.st_size, s.st_mode)
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot((11, 0o100644))


def test_abstract_filesystem_stat_directory():
    """AbstractFileSystem.path_stat() returns stat result for directories."""
    fs = TestOS()
    fs.directories.add('/mydir')

    code = """
from pathlib import Path
s = Path('/mydir').stat()
s.st_mode
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot(0o040755)


def test_abstract_filesystem_stat_missing():
    """AbstractFileSystem.path_stat() raises FileNotFoundError for missing paths."""
    fs = TestOS()

    m = pydantic_monty.Monty('from pathlib import Path\nPath("/missing").stat()')
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(os=fs)

    assert str(exc_info.value) == snapshot('FileNotFoundError: No such file or directory: /missing')
    assert exc_info.value.display() == snapshot("""\
Traceback (most recent call last):
  File "main.py", line 2, in <module>
    Path("/missing").stat()
    ~~~~~~~~~~~~~~~~~~~~~~~
FileNotFoundError: No such file or directory: /missing\
""")


# =============================================================================
# iterdir() tests
# =============================================================================


def test_abstract_filesystem_iterdir():
    """AbstractFileSystem.path_iterdir() lists directory contents."""
    fs = TestOS()
    fs.directories.add('/mydir')
    fs.files['/mydir/a.txt'] = b'a'
    fs.files['/mydir/b.txt'] = b'b'
    fs.directories.add('/mydir/subdir')

    code = """
from pathlib import Path
list(Path('/mydir').iterdir())
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    # Result is a list of Path objects with child names joined to parent
    assert len(result) == 3
    names = sorted(str(p) for p in result)
    assert names == snapshot(['a.txt', 'b.txt', 'subdir'])


def test_abstract_filesystem_iterdir_empty():
    """AbstractFileSystem.path_iterdir() returns empty list for empty directory."""
    fs = TestOS()
    fs.directories.add('/empty')

    code = """
from pathlib import Path
list(Path('/empty').iterdir())
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot([])


# =============================================================================
# resolve() and absolute() tests
# =============================================================================


def test_abstract_filesystem_resolve():
    """AbstractFileSystem.path_resolve() normalizes paths."""
    fs = TestOS()

    code = """
from pathlib import Path
str(Path('/foo/bar/../baz').resolve())
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot('/foo/baz')


def test_abstract_filesystem_absolute():
    """AbstractFileSystem.path_absolute() returns absolute path."""
    fs = TestOS()

    code = """
from pathlib import Path
str(Path('/already/absolute').absolute())
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot('/already/absolute')


def test_abstract_filesystem_getenv():
    """AbstractFileSystem.getenv() returns environment variable value."""
    fs = TestOS()

    code = """
import os
os.getenv('TEST_VAR')
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot('test_value')


def test_abstract_filesystem_getenv_missing():
    """AbstractFileSystem.getenv() returns None for missing variable."""
    fs = TestOS()

    code = """
import os
os.getenv('NONEXISTENT')
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result is None


def test_abstract_filesystem_getenv_default():
    """AbstractFileSystem.getenv() returns default for missing variable."""
    fs = TestOS()

    code = """
import os
os.getenv('NONEXISTENT', 'my_default')
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=fs)

    assert result == snapshot('my_default')


# =============================================================================
# file_stat / dir_stat helper tests
# =============================================================================


def test_file_stat_helper():
    """file_stat() creates a proper stat result."""
    stat = StatResult.file_stat(1024, 0o644, 1700000000.0)

    # Check it has the expected structure (10 fields)
    assert len(stat) == snapshot(10)
    # Index access: st_mode=0, st_size=6, st_mtime=8
    assert stat[0] == snapshot(0o100644)  # st_mode - file_stat adds file type bits
    assert stat[6] == snapshot(1024)  # st_size
    assert stat[8] == snapshot(1700000000.0)  # st_mtime


def test_dir_stat_helper():
    """dir_stat() creates a proper stat result for directories."""
    stat = StatResult.dir_stat(0o755, 1700000000.0)

    assert len(stat) == snapshot(10)
    # Index access: st_mode=0, st_size=6, st_mtime=8
    assert stat[0] == snapshot(0o040755)  # st_mode - dir_stat adds directory type bits
    assert stat[6] == snapshot(4096)  # st_size - directories have fixed size
    assert stat[8] == snapshot(1700000000.0)  # st_mtime
