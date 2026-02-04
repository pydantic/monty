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

    def path_exists(self, path: PurePosixPath) -> bool:
        p = str(path)
        return p in self.files or p in self.directories

    def path_is_file(self, path: PurePosixPath) -> bool:
        return str(path) in self.files

    def path_is_dir(self, path: PurePosixPath) -> bool:
        return str(path) in self.directories

    def path_is_symlink(self, path: PurePosixPath) -> bool:
        return False  # No symlink support in this simple implementation

    def path_read_text(self, path: PurePosixPath) -> str:
        p = str(path)
        if p not in self.files:
            raise FileNotFoundError(f'No such file: {p}')
        return self.files[p].decode('utf-8')

    def path_read_bytes(self, path: PurePosixPath) -> bytes:
        p = str(path)
        if p not in self.files:
            raise FileNotFoundError(f'No such file: {p}')
        return self.files[p]

    def path_write_text(self, path: PurePosixPath, data: str) -> int:
        p = str(path)
        self._ensure_parent_exists(p)
        self.files[p] = data.encode('utf-8')
        return len(data)

    def path_write_bytes(self, path: PurePosixPath, data: bytes) -> int:
        p = str(path)
        self._ensure_parent_exists(p)
        self.files[p] = data
        return len(data)

    def path_mkdir(self, path: PurePosixPath, parents: bool, exist_ok: bool) -> None:
        p = str(path)
        if p in self.directories:
            if not exist_ok:
                raise FileExistsError(f'Directory exists: {p}')
            return
        if parents:
            self._ensure_parent_exists(p)
        self.directories.add(p)

    def path_unlink(self, path: PurePosixPath) -> None:
        p = str(path)
        if p not in self.files:
            raise FileNotFoundError(f'No such file: {p}')
        del self.files[p]

    def path_rmdir(self, path: PurePosixPath) -> None:
        p = str(path)
        if p not in self.directories:
            raise FileNotFoundError(f'No such directory: {p}')
        # Check if directory is empty
        for f in self.files:
            if f.startswith(p + '/'):
                raise OSError(f'Directory not empty: {p}')
        for d in self.directories:
            if d != p and d.startswith(p + '/'):
                raise OSError(f'Directory not empty: {p}')
        self.directories.remove(p)

    def path_iterdir(self, path: PurePosixPath) -> list[PurePosixPath]:
        p = str(path)
        if p not in self.directories:
            raise FileNotFoundError(f'No such directory: {p}')
        result: list[PurePosixPath] = []
        prefix = p.rstrip('/') + '/'
        seen: set[str] = set()
        for f in self.files:
            if f.startswith(prefix):
                # Get immediate child name
                rest = f[len(prefix) :]
                child = rest.split('/')[0]
                if child and child not in seen:
                    seen.add(child)
                    result.append(PurePosixPath(prefix + child))
        for d in self.directories:
            if d.startswith(prefix) and d != p:
                rest = d[len(prefix) :]
                child = rest.split('/')[0]
                if child and child not in seen:
                    seen.add(child)
                    result.append(PurePosixPath(prefix + child))
        return sorted(result)

    def path_stat(self, path: PurePosixPath) -> StatResult:
        p = str(path)
        if p in self.files:
            return StatResult.file_stat(len(self.files[p]), 0o644, 0.0)
        elif p in self.directories:
            return StatResult.dir_stat(0o755, 0.0)
        else:
            raise FileNotFoundError(f'No such file or directory: {p}')

    def path_rename(self, path: PurePosixPath, target: PurePosixPath) -> None:
        p = str(path)
        t = str(target)
        if p in self.files:
            self._ensure_parent_exists(t)
            self.files[t] = self.files.pop(p)
        elif p in self.directories:
            self._ensure_parent_exists(t)
            self.directories.remove(p)
            self.directories.add(t)
            # Move all files under this directory
            prefix = p.rstrip('/') + '/'
            to_move = [(f, t + f[len(p) :]) for f in self.files if f.startswith(prefix)]
            for old, new in to_move:
                self.files[new] = self.files.pop(old)
        else:
            raise FileNotFoundError(f'No such file or directory: {p}')

    def path_resolve(self, path: PurePosixPath) -> str:
        # Simple implementation: just normalize the path
        p = str(path)
        parts: list[str] = []
        for part in p.split('/'):
            if part == '..':
                if parts:
                    parts.pop()
            elif part and part != '.':
                parts.append(part)
        return '/' + '/'.join(parts)

    def path_absolute(self, path: PurePosixPath) -> str:
        p = str(path)
        if p.startswith('/'):
            return p
        return '/' + p

    def getenv(self, key: str, default: str | None = None) -> str | None:
        # Simple virtual environment for testing
        env = {
            'TEST_VAR': 'test_value',
            'HOME': '/test/home',
        }
        return env.get(key, default)

    def get_environ(self) -> dict[str, str]:
        return {
            'TEST_VAR': 'test_value',
            'HOME': '/test/home',
        }


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
    assert names == snapshot(['/mydir/a.txt', '/mydir/b.txt', '/mydir/subdir'])


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


def test_path_monty_to_py():
    m = pydantic_monty.Monty('from pathlib import Path; Path("/foo/bar/thing.txt")')
    result = m.run()
    assert result == PurePosixPath('/foo/bar/thing.txt')
    assert type(result) is PurePosixPath


def test_path_py_to_monty():
    p = PurePosixPath('/foo/bar/thing.txt')
    m = pydantic_monty.Monty('f"type={type(p)} {p=}"', inputs=['p'])
    result = m.run(inputs={'p': p})
    assert result == snapshot("type=<class 'PosixPath'> p=PosixPath('/foo/bar/thing.txt')")
