"""Tests for MountDirectory filesystem mount support.

These test the Rust-backed mount system that handles filesystem operations
entirely in Rust, with optional Python fallback for non-filesystem ops via `os=`.
"""

import tempfile
from collections.abc import Generator
from pathlib import Path

import pytest
from inline_snapshot import snapshot

from pydantic_monty import Monty, MontyRuntimeError, MountDirectory


@pytest.fixture
def test_dir() -> Generator[Path, None, None]:
    """Creates a temporary directory with test files."""
    with tempfile.TemporaryDirectory() as tmpdir:
        p = Path(tmpdir)
        (p / 'hello.txt').write_text('hello world')
        (p / 'data.bin').write_bytes(b'\x00\x01\x02')
        (p / 'subdir').mkdir()
        (p / 'subdir' / 'nested.txt').write_text('nested content')
        yield p


# =============================================================================
# MountDirectory validation
# =============================================================================


def test_mount_directory_repr(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    assert 'MountDirectory' in repr(md)
    assert '/data' in repr(md)


def test_mount_directory_invalid_mode():
    with pytest.raises(ValueError) as exc_info:
        MountDirectory('/data', '/tmp', 'invalid')  # pyright: ignore[reportArgumentType]
    assert str(exc_info.value) == snapshot("Invalid mode 'invalid', expected 'read-only', 'read-write', or 'overlay'")


def test_mount_directory_attributes(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    assert md.virtual_path == '/data'
    assert md.mode == 'read-only'


def test_nonexistent_host_path():
    with pytest.raises(ValueError):
        MountDirectory('/data', '/nonexistent/path/that/does/not/exist')


def test_non_absolute_virtual_path(test_dir: Path):
    with pytest.raises(ValueError):
        MountDirectory('relative', str(test_dir))


# =============================================================================
# Read operations (read-only mount)
# =============================================================================


def test_read_text(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    result = Monty("from pathlib import Path; Path('/data/hello.txt').read_text()").run(mount=md)
    assert result == snapshot('hello world')


def test_read_bytes(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    result = Monty("from pathlib import Path; Path('/data/data.bin').read_bytes()").run(mount=md)
    assert result == snapshot(b'\x00\x01\x02')


def test_path_exists(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    code = """
from pathlib import Path
exists_file = Path('/data/hello.txt').exists()
exists_dir = Path('/data/subdir').exists()
exists_missing = Path('/data/nope.txt').exists()
(exists_file, exists_dir, exists_missing)
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot((True, True, False))


def test_is_file_is_dir(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    code = """
from pathlib import Path
(Path('/data/hello.txt').is_file(), Path('/data/hello.txt').is_dir(),
 Path('/data/subdir').is_file(), Path('/data/subdir').is_dir())
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot((True, False, False, True))


def test_iterdir(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    code = """
from pathlib import Path
sorted([p.name for p in Path('/data').iterdir()])
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot(['data.bin', 'hello.txt', 'subdir'])


def test_stat(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    code = """
from pathlib import Path
s = Path('/data/hello.txt').stat()
s.st_size
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot(11)


def test_read_nested(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    result = Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()").run(mount=md)
    assert result == snapshot('nested content')


# =============================================================================
# Write operations
# =============================================================================


def test_write_read_only_blocked(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/data/new.txt').write_text('x')").run(mount=md)
    assert 'Read-only file system' in str(exc_info.value)


def test_write_read_write(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-write')
    code = """
from pathlib import Path
Path('/data/new.txt').write_text('written by monty')
Path('/data/new.txt').read_text()
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot('written by monty')
    # Verify it was actually written to the host filesystem
    assert (test_dir / 'new.txt').read_text() == 'written by monty'


def test_overlay_write_doesnt_modify_host(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'overlay')
    code = """
from pathlib import Path
Path('/data/overlay_file.txt').write_text('overlay content')
Path('/data/overlay_file.txt').read_text()
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot('overlay content')
    # Verify host filesystem was NOT modified
    assert not (test_dir / 'overlay_file.txt').exists()


def test_overlay_read_falls_through(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'overlay')
    result = Monty("from pathlib import Path; Path('/data/hello.txt').read_text()").run(mount=md)
    assert result == snapshot('hello world')


def test_overlay_persists_across_runs(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'overlay')
    Monty("from pathlib import Path; Path('/data/persistent.txt').write_text('run1')").run(mount=md)
    result = Monty("from pathlib import Path; Path('/data/persistent.txt').read_text()").run(mount=md)
    assert result == snapshot('run1')


# =============================================================================
# Path operations
# =============================================================================


def test_mkdir_rmdir(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'overlay')
    code = """
from pathlib import Path
Path('/data/newdir').mkdir()
exists = Path('/data/newdir').is_dir()
Path('/data/newdir').rmdir()
after = Path('/data/newdir').exists()
(exists, after)
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot((True, False))


def test_unlink(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'overlay')
    code = """
from pathlib import Path
Path('/data/hello.txt').unlink()
Path('/data/hello.txt').exists()
"""
    result = Monty(code).run(mount=md)
    assert result is False
    # Host file should still exist (overlay mode)
    assert (test_dir / 'hello.txt').exists()


def test_rename(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'overlay')
    code = """
from pathlib import Path
Path('/data/hello.txt').rename('/data/renamed.txt')
(Path('/data/hello.txt').exists(), Path('/data/renamed.txt').read_text())
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot((False, 'hello world'))


def test_resolve(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    result = Monty("from pathlib import Path; str(Path('/data/subdir/../hello.txt').resolve())").run(mount=md)
    assert result == snapshot('/data/hello.txt')


# =============================================================================
# Security
# =============================================================================


def test_path_traversal_blocked(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/data/../../etc/passwd').read_text()").run(mount=md)
    assert 'Permission denied' in str(exc_info.value)


def test_unmounted_path_denied(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/other/file.txt').exists()").run(mount=md)
    assert 'Permission denied' in str(exc_info.value)


# =============================================================================
# Fallback via os= for non-filesystem ops
# =============================================================================


def test_fallback_for_getenv(test_dir: Path):
    def fallback(function_name: str, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        if function_name == 'os.getenv':
            return 'my_value' if args[0] == 'MY_VAR' else None
        return None

    md = MountDirectory('/data', str(test_dir), 'read-only')
    result = Monty("import os; os.getenv('MY_VAR')").run(mount=md, os=fallback)
    assert result == snapshot('my_value')


def test_no_fallback_not_implemented(test_dir: Path):
    md = MountDirectory('/data', str(test_dir), 'read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("import os; os.getenv('PATH')").run(mount=md)
    assert 'is not supported in this environment' in str(exc_info.value)


# =============================================================================
# Multiple mounts
# =============================================================================


def test_multiple_mounts_different_modes(test_dir: Path):
    with tempfile.TemporaryDirectory() as tmpdir2:
        p2 = Path(tmpdir2)
        (p2 / 'file2.txt').write_text('from mount2')

        mounts = [
            MountDirectory('/ro', str(test_dir), 'read-only'),
            MountDirectory('/rw', str(p2), 'read-write'),
        ]
        code = """
from pathlib import Path
a = Path('/ro/hello.txt').read_text()
b = Path('/rw/file2.txt').read_text()
(a, b)
"""
        result = Monty(code).run(mount=mounts)
        assert result == snapshot(('hello world', 'from mount2'))
