"""Tests for MountDir filesystem mount support.

These test the Rust-backed mount system that handles filesystem operations
entirely in Rust, with optional Python fallback for non-filesystem ops via `os=`.
"""

import tempfile
from collections.abc import Generator
from pathlib import Path

import pytest
from inline_snapshot import snapshot

from pydantic_monty import Monty, MontyRepl, MontyRuntimeError, MountDir


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


def assert_mount_reusable(md: MountDir) -> None:
    """Assert that a previously used mount was returned to its shared slot."""
    result = Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()").run(mount=md)
    assert result == snapshot('nested content')


# =============================================================================
# MountDir validation
# =============================================================================


def test_mount_directory_repr(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    assert 'MountDir' in repr(md)
    assert '/data' in repr(md)


def test_mount_directory_invalid_mode():
    with pytest.raises(ValueError) as exc_info:
        MountDir('/data', '/tmp', mode='invalid')  # pyright: ignore[reportArgumentType]
    assert str(exc_info.value) == snapshot("Invalid mode 'invalid', expected 'read-only', 'read-write', or 'overlay'")


def test_mount_directory_attributes(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    assert md.virtual_path == '/data'
    assert md.mode == 'read-only'


def test_mount_directory_accepts_path_object(test_dir: Path):
    """MountDir should accept both str and Path for host_path."""
    md_str = MountDir('/data', str(test_dir), mode='read-only')
    md_path = MountDir('/data', test_dir, mode='read-only')
    assert md_path.virtual_path == '/data'
    assert md_path.host_path == md_str.host_path


def test_nonexistent_host_path():
    with pytest.raises(TypeError) as exc_info:
        MountDir('/data', '/nonexistent/path/that/does/not/exist')
    assert str(exc_info.value) == snapshot(
        "cannot canonicalize host path '/nonexistent/path/that/does/not/exist': No such file or directory (os error 2)"
    )


def test_non_absolute_virtual_path(test_dir: Path):
    with pytest.raises(TypeError) as exc_info:
        MountDir('relative', str(test_dir))
    assert str(exc_info.value) == snapshot("virtual path must be absolute, got: 'relative'")


# =============================================================================
# Read operations (read-only mount)
# =============================================================================


def test_read_text(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = Monty("from pathlib import Path; Path('/data/hello.txt').read_text()").run(mount=md)
    assert result == snapshot('hello world')


def test_read_bytes(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = Monty("from pathlib import Path; Path('/data/data.bin').read_bytes()").run(mount=md)
    assert result == snapshot(b'\x00\x01\x02')


def test_path_exists(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
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
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
(Path('/data/hello.txt').is_file(), Path('/data/hello.txt').is_dir(),
 Path('/data/subdir').is_file(), Path('/data/subdir').is_dir())
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot((True, False, False, True))


def test_iterdir(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
sorted([p.name for p in Path('/data').iterdir()])
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot(['data.bin', 'hello.txt', 'subdir'])


def test_stat(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
s = Path('/data/hello.txt').stat()
s.st_size
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot(11)


def test_read_nested(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()").run(mount=md)
    assert result == snapshot('nested content')


# =============================================================================
# Write operations
# =============================================================================


def test_write_read_only_blocked(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/data/new.txt').write_text('x')").run(mount=md)
    assert 'Read-only file system' in str(exc_info.value)


def test_write_read_write(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-write')
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
    md = MountDir('/data', str(test_dir), mode='overlay')
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
    md = MountDir('/data', str(test_dir), mode='overlay')
    result = Monty("from pathlib import Path; Path('/data/hello.txt').read_text()").run(mount=md)
    assert result == snapshot('hello world')


def test_overlay_persists_across_runs(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='overlay')
    Monty("from pathlib import Path; Path('/data/persistent.txt').write_text('run1')").run(mount=md)
    result = Monty("from pathlib import Path; Path('/data/persistent.txt').read_text()").run(mount=md)
    assert result == snapshot('run1')


def test_run_mount_released_after_runtime_error(test_dir: Path):
    """Monty.run() puts mounts back when execution raises after an OS call."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
1 / 0
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty(code).run(mount=md)
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)
    assert_mount_reusable(md)


def test_run_mount_released_after_resource_error(test_dir: Path):
    """Monty.run() puts mounts back when a resource limit trips after an OS call."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty(code).run(mount=md, limits={'max_memory': 100})
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


# =============================================================================
# Path operations
# =============================================================================


def test_mkdir_rmdir(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='overlay')
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
    md = MountDir('/data', str(test_dir), mode='overlay')
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
    md = MountDir('/data', str(test_dir), mode='overlay')
    code = """
from pathlib import Path
Path('/data/hello.txt').rename('/data/renamed.txt')
(Path('/data/hello.txt').exists(), Path('/data/renamed.txt').read_text())
"""
    result = Monty(code).run(mount=md)
    assert result == snapshot((False, 'hello world'))


def test_resolve(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = Monty("from pathlib import Path; str(Path('/data/subdir/../hello.txt').resolve())").run(mount=md)
    assert result == snapshot('/data/hello.txt')


# =============================================================================
# Security
# =============================================================================


def test_path_traversal_blocked(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/data/../../etc/passwd').read_text()").run(mount=md)
    assert 'Permission denied' in str(exc_info.value)


def test_unmounted_path_denied(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
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

    md = MountDir('/data', str(test_dir), mode='read-only')
    result = Monty("import os; os.getenv('MY_VAR')").run(mount=md, os=fallback)
    assert result == snapshot('my_value')


def test_no_fallback_not_implemented(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
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
            MountDir('/ro', str(test_dir), mode='read-only'),
            MountDir('/rw', str(p2), mode='read-write'),
        ]
        code = """
from pathlib import Path
a = Path('/ro/hello.txt').read_text()
b = Path('/rw/file2.txt').read_text()
(a, b)
"""
        result = Monty(code).run(mount=mounts)
        assert result == snapshot(('hello world', 'from mount2'))


# =============================================================================
# REPL mount support
# =============================================================================


def test_repl_feed_run_with_mount(test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    result = repl.feed_run("Path('/data/hello.txt').read_text()", mount=md)
    assert result == snapshot('hello world')


def test_repl_overlay_write_persists_across_feeds(test_dir: Path):
    """Overlay writes in one feed() call are visible in subsequent feed() calls."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    repl.feed_run("Path('/data/new.txt').write_text('from repl')", mount=md)
    result = repl.feed_run("Path('/data/new.txt').read_text()", mount=md)
    assert result == snapshot('from repl')
    # Host not modified
    assert not (test_dir / 'new.txt').exists()


def test_repl_overlay_overwrite_persists(test_dir: Path):
    """Overwriting an overlay file across feeds preserves the latest content."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    repl.feed_run("Path('/data/hello.txt').write_text('version1')", mount=md)
    repl.feed_run("Path('/data/hello.txt').write_text('version2')", mount=md)
    result = repl.feed_run("Path('/data/hello.txt').read_text()", mount=md)
    assert result == snapshot('version2')
    # Original host file unchanged
    assert (test_dir / 'hello.txt').read_text() == 'hello world'


def test_repl_overlay_delete_persists(test_dir: Path):
    """Deleting a file in overlay mode persists across feeds."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    repl.feed_run("Path('/data/hello.txt').unlink()", mount=md)
    result = repl.feed_run("Path('/data/hello.txt').exists()", mount=md)
    assert result is False
    # Host file still exists
    assert (test_dir / 'hello.txt').exists()


def test_repl_overlay_mkdir_persists(test_dir: Path):
    """Directories created in overlay mode persist across feeds."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    repl.feed_run("Path('/data/mydir').mkdir()", mount=md)
    repl.feed_run("Path('/data/mydir/file.txt').write_text('nested')", mount=md)
    result = repl.feed_run("Path('/data/mydir/file.txt').read_text()", mount=md)
    assert result == snapshot('nested')
    assert not (test_dir / 'mydir').exists()


def test_repl_overlay_iterdir_sees_overlay_files(test_dir: Path):
    """iterdir() reflects both host and overlay files."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    repl.feed_run("Path('/data/extra.txt').write_text('extra')", mount=md)
    result = repl.feed_run("sorted([p.name for p in Path('/data').iterdir()])", mount=md)
    assert result == snapshot(['data.bin', 'extra.txt', 'hello.txt', 'subdir'])


def test_repl_overlay_shared_between_repl_and_monty(test_dir: Path):
    """The same MountDir overlay state is shared between REPL and Monty.run()."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    # Write via REPL
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    repl.feed_run("Path('/data/shared.txt').write_text('from repl')", mount=md)
    # Read via Monty.run()
    result = Monty("from pathlib import Path; Path('/data/shared.txt').read_text()").run(mount=md)
    assert result == snapshot('from repl')


def test_repl_read_write_mount(test_dir: Path):
    """Read-write mounts in the REPL write to the host filesystem."""
    md = MountDir('/data', str(test_dir), mode='read-write')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    repl.feed_run("Path('/data/rw_file.txt').write_text('written')", mount=md)
    result = repl.feed_run("Path('/data/rw_file.txt').read_text()", mount=md)
    assert result == snapshot('written')
    # Host was actually modified
    assert (test_dir / 'rw_file.txt').read_text() == 'written'


def test_repl_read_only_mount_blocks_write(test_dir: Path):
    """Read-only mounts in the REPL reject write operations."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = MontyRepl()
    repl.feed_run('from pathlib import Path', mount=md)
    with pytest.raises(MontyRuntimeError) as exc_info:
        repl.feed_run("Path('/data/nope.txt').write_text('x')", mount=md)
    assert 'Read-only file system' in str(exc_info.value)


def test_repl_feed_run_mount_released_after_runtime_error(test_dir: Path):
    """MontyRepl.feed_run() puts mounts back when execution raises after an OS call."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = MontyRepl()
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
1 / 0
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        repl.feed_run(code, mount=md)
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)
    assert_mount_reusable(md)


def test_repl_feed_run_mount_released_after_resource_error(test_dir: Path):
    """MontyRepl.feed_run() puts mounts back when a resource limit trips after an OS call."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = MontyRepl(limits={'max_memory': 100})
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        repl.feed_run(code, mount=md)
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


def test_run_mount_released_after_callback_marshalling_error(test_dir: Path):
    """Monty.run() puts mounts back when the os= callback returns an unconvertible value.

    The fallback callback returns an object that cannot be converted back into a
    Monty value, so `py_to_monty` raises TypeError mid-OS-dispatch. The mount
    must still be returned to its shared slot.
    """
    md = MountDir('/data', str(test_dir), mode='read-only')

    def os_cb(func: object, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        return object()  # unconvertible — triggers TypeError in py_to_monty

    # Path is outside the mount so it falls through to the os= fallback.
    code = "from pathlib import Path; Path('/outside/path.txt').exists()"
    with pytest.raises(TypeError):
        Monty(code).run(mount=md, os=os_cb)
    assert_mount_reusable(md)


def test_repl_feed_run_mount_and_repl_released_after_callback_marshalling_error(test_dir: Path):
    """MontyRepl.feed_run() puts mount AND REPL back when os= callback returns an unconvertible value.

    Without this fix, the `?` propagation through `handle_repl_os_call` leaks
    both the mount slot (stuck `<in use>`) and the REPL session (mutex left
    `None`), so neither the `MountDir` nor the `MontyRepl` could be reused.
    """
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = MontyRepl()
    repl.feed_run('x = 42')

    def os_cb(func: object, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        return object()  # unconvertible — triggers TypeError in py_to_monty

    code = "from pathlib import Path; Path('/outside/path.txt').exists()"
    with pytest.raises(TypeError):
        repl.feed_run(code, mount=md, os=os_cb)
    # REPL state must still be intact — `x` is visible in a later snippet.
    assert repl.feed_run('x') == snapshot(42)
    assert_mount_reusable(md)
