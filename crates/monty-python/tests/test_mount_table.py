"""Tests for MountDir filesystem mount support.

These test the Rust-backed mount system that services filesystem operations
on the host side of the worker pool, with optional Python fallback for
non-filesystem ops via `os=`.

Mounts are host-side: a `MountDir` only contributes its configuration
(virtual path, host path, mode, resource limits) — the pool builds a fresh mount
table per feed and answers the worker's filesystem OS calls itself.
`'overlay'` writes are visible within one `feed_run` call and discarded when
the feed ends; `'read-write'` mounts write through to the real host
directory.
"""

import tempfile
from collections.abc import Generator
from pathlib import Path

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import Monty, MontyFileHandle, MontyRuntimeError, MountDir


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


def assert_mount_reusable(monty_run: RunMonty, md: MountDir) -> None:
    """Assert that a previously used MountDir config works in a fresh run."""
    result = monty_run("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()", mount=md)
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
    assert md.memory_usage_limit == 100_000_000

    limited = MountDir('/limited', str(test_dir), memory_usage_limit=1234)
    assert limited.memory_usage_limit == 1234


def test_mount_directory_accepts_path_object(test_dir: Path):
    """MountDir should accept both str and Path for host_path."""
    md_str = MountDir('/data', str(test_dir), mode='read-only')
    md_path = MountDir('/data', test_dir, mode='read-only')
    assert md_path.virtual_path == '/data'
    assert md_path.host_path == md_str.host_path


def test_mount_memory_usage_limit_is_enforced(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', test_dir, mode='overlay', memory_usage_limit=1000)
    code = """
from pathlib import Path
p = Path('/data/retained.bin')
p.write_bytes(b'a' * 500)
p.read_bytes()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, mount=md)
    assert str(exc_info.value) == snapshot('MemoryError: mount memory usage limit of 1 KB exceeded')


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


def test_read_text(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = monty_run("from pathlib import Path; Path('/data/hello.txt').read_text()", mount=md)
    assert result == snapshot('hello world')


def test_read_bytes(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = monty_run("from pathlib import Path; Path('/data/data.bin').read_bytes()", mount=md)
    assert result == snapshot(b'\x00\x01\x02')


def test_path_exists(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
exists_file = Path('/data/hello.txt').exists()
exists_dir = Path('/data/subdir').exists()
exists_missing = Path('/data/nope.txt').exists()
(exists_file, exists_dir, exists_missing)
"""
    result = monty_run(code, mount=md)
    assert result == snapshot((True, True, False))


def test_is_file_is_dir(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
(Path('/data/hello.txt').is_file(), Path('/data/hello.txt').is_dir(),
 Path('/data/subdir').is_file(), Path('/data/subdir').is_dir())
"""
    result = monty_run(code, mount=md)
    assert result == snapshot((True, False, False, True))


def test_iterdir(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
sorted([p.name for p in Path('/data').iterdir()])
"""
    result = monty_run(code, mount=md)
    assert result == snapshot(['data.bin', 'hello.txt', 'subdir'])


def test_stat(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
s = Path('/data/hello.txt').stat()
s.st_size
"""
    result = monty_run(code, mount=md)
    assert result == snapshot(11)


def test_read_nested(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = monty_run("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()", mount=md)
    assert result == snapshot('nested content')


# =============================================================================
# Write operations
# =============================================================================


def test_write_read_only_blocked(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("from pathlib import Path; Path('/data/new.txt').write_text('x')", mount=md)
    assert 'Read-only file system' in str(exc_info.value)


def test_write_read_write(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-write')
    code = """
from pathlib import Path
Path('/data/new.txt').write_text('written by monty')
Path('/data/new.txt').read_text()
"""
    result = monty_run(code, mount=md)
    assert result == snapshot('written by monty')
    # Verify it was actually written to the host filesystem
    assert (test_dir / 'new.txt').read_text() == 'written by monty'


def test_overlay_write_doesnt_modify_host(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='overlay')
    code = """
from pathlib import Path
Path('/data/overlay_file.txt').write_text('overlay content')
Path('/data/overlay_file.txt').read_text()
"""
    result = monty_run(code, mount=md)
    assert result == snapshot('overlay content')
    # Verify host filesystem was NOT modified
    assert not (test_dir / 'overlay_file.txt').exists()


def test_overlay_read_falls_through(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='overlay')
    result = monty_run("from pathlib import Path; Path('/data/hello.txt').read_text()", mount=md)
    assert result == snapshot('hello world')


def test_overlay_discarded_across_runs(monty_run: RunMonty, test_dir: Path):
    """Overlay writes are worker-local: they do not persist to the next run."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    monty_run("from pathlib import Path; Path('/data/transient.txt').write_text('run1')", mount=md)
    result = monty_run("from pathlib import Path; Path('/data/transient.txt').exists()", mount=md)
    assert result is False
    # Host filesystem was never modified either
    assert not (test_dir / 'transient.txt').exists()


def test_mount_reusable_after_runtime_error(monty_run: RunMonty, test_dir: Path):
    """A MountDir config works again after a run that raised mid-execution."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
1 / 0
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, mount=md)
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)
    assert_mount_reusable(monty_run, md)


def test_mount_reusable_after_resource_error(monty_run: RunMonty, test_dir: Path):
    """A MountDir config works again after a run that hit a resource limit."""
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
        monty_run(code, mount=md, limits={'max_memory': 100})
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(monty_run, md)


# =============================================================================
# Path operations
# =============================================================================


def test_mkdir_rmdir(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='overlay')
    code = """
from pathlib import Path
Path('/data/newdir').mkdir()
exists = Path('/data/newdir').is_dir()
Path('/data/newdir').rmdir()
after = Path('/data/newdir').exists()
(exists, after)
"""
    result = monty_run(code, mount=md)
    assert result == snapshot((True, False))


def test_unlink(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='overlay')
    code = """
from pathlib import Path
Path('/data/hello.txt').unlink()
Path('/data/hello.txt').exists()
"""
    result = monty_run(code, mount=md)
    assert result is False
    # Host file should still exist (overlay mode)
    assert (test_dir / 'hello.txt').exists()


def test_rename(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='overlay')
    code = """
from pathlib import Path
Path('/data/hello.txt').rename('/data/renamed.txt')
(Path('/data/hello.txt').exists(), Path('/data/renamed.txt').read_text())
"""
    result = monty_run(code, mount=md)
    assert result == snapshot((False, 'hello world'))


def test_resolve(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    result = monty_run("from pathlib import Path; str(Path('/data/subdir/../hello.txt').resolve())", mount=md)
    assert result == snapshot('/data/hello.txt')


# =============================================================================
# Security
# =============================================================================


def test_path_traversal_blocked(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("from pathlib import Path; Path('/data/../../etc/passwd').read_text()", mount=md)
    assert 'Permission denied' in str(exc_info.value)


def test_unmounted_path_denied(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("from pathlib import Path; Path('/other/file.txt').exists()", mount=md)
    assert 'Permission denied' in str(exc_info.value)


# =============================================================================
# Fallback via os= for non-filesystem ops
# =============================================================================


def test_fallback_for_getenv(monty_run: RunMonty, test_dir: Path):
    def fallback(function_name: str, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        if function_name == 'os.getenv':
            return 'my_value' if args[0] == 'MY_VAR' else None
        return None

    md = MountDir('/data', str(test_dir), mode='read-only')
    result = monty_run("import os; os.getenv('MY_VAR')", mount=md, os=fallback)
    assert result == snapshot('my_value')


def test_no_fallback_not_implemented(monty_run: RunMonty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("import os; os.getenv('PATH')", mount=md)
    assert 'is not supported in this environment' in str(exc_info.value)


def test_mounted_calls_do_not_reach_os_callback(monty_run: RunMonty, test_dir: Path):
    """Filesystem calls covered by a mount are serviced by the pool and
    never reach the `os=` callback."""
    calls: list[str] = []

    def fallback(function_name: str, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        calls.append(function_name)
        return None

    md = MountDir('/data', str(test_dir), mode='read-only')
    result = monty_run("from pathlib import Path; Path('/data/hello.txt').read_text()", mount=md, os=fallback)
    assert result == snapshot('hello world')
    assert calls == []


# =============================================================================
# Multiple mounts
# =============================================================================


def test_multiple_mounts_different_modes(monty_run: RunMonty, test_dir: Path):
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
        result = monty_run(code, mount=mounts)
        assert result == snapshot(('hello world', 'from mount2'))


# =============================================================================
# Session (multi-feed) mount support
# =============================================================================


def test_session_feed_run_with_mount(pool: Monty, test_dir: Path):
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pool.checkout() as session:
        session.feed_run('from pathlib import Path', mount=md)
        result = session.feed_run("Path('/data/hello.txt').read_text()", mount=md)
    assert result == snapshot('hello world')


def test_overlay_visible_within_feed(pool: Monty, test_dir: Path):
    """Overlay writes, deletes, and mkdirs are all visible within one feed."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    code = """
from pathlib import Path
Path('/data/new.txt').write_text('version1')
Path('/data/new.txt').write_text('version2')
content = Path('/data/new.txt').read_text()
Path('/data/mydir').mkdir()
Path('/data/mydir/file.txt').write_text('nested')
nested = Path('/data/mydir/file.txt').read_text()
Path('/data/hello.txt').unlink()
deleted = Path('/data/hello.txt').exists()
listing = sorted([p.name for p in Path('/data').iterdir()])
(content, nested, deleted, listing)
"""
    with pool.checkout() as session:
        result = session.feed_run(code, mount=md)
    assert result == snapshot(('version2', 'nested', False, ['data.bin', 'mydir', 'new.txt', 'subdir']))
    # Host filesystem is completely untouched
    assert not (test_dir / 'new.txt').exists()
    assert not (test_dir / 'mydir').exists()
    assert (test_dir / 'hello.txt').read_text() == 'hello world'


def test_overlay_discarded_between_feeds(pool: Monty, test_dir: Path):
    """Overlay writes are discarded when the feed ends — the next feed in the
    same session sees the original host contents again."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    with pool.checkout() as session:
        session.feed_run('from pathlib import Path', mount=md)
        session.feed_run("Path('/data/new.txt').write_text('from feed1')", mount=md)
        assert session.feed_run("Path('/data/new.txt').exists()", mount=md) is False
        # a host file deleted in overlay mode reappears in the next feed
        session.feed_run("Path('/data/hello.txt').unlink()", mount=md)
        assert session.feed_run("Path('/data/hello.txt').read_text()", mount=md) == snapshot('hello world')
    # Host not modified
    assert not (test_dir / 'new.txt').exists()
    assert (test_dir / 'hello.txt').read_text() == 'hello world'


def test_session_read_write_mount(pool: Monty, test_dir: Path):
    """Read-write mounts write through to the host, so writes persist across feeds."""
    md = MountDir('/data', str(test_dir), mode='read-write')
    with pool.checkout() as session:
        session.feed_run('from pathlib import Path', mount=md)
        session.feed_run("Path('/data/rw_file.txt').write_text('written')", mount=md)
        result = session.feed_run("Path('/data/rw_file.txt').read_text()", mount=md)
    assert result == snapshot('written')
    # Host was actually modified
    assert (test_dir / 'rw_file.txt').read_text() == 'written'


def test_session_read_only_mount_blocks_write(pool: Monty, test_dir: Path):
    """Read-only mounts in a session reject write operations."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    with pool.checkout() as session:
        session.feed_run('from pathlib import Path', mount=md)
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run("Path('/data/nope.txt').write_text('x')", mount=md)
        assert 'Read-only file system' in str(exc_info.value)


# =============================================================================
# os= callback error paths
# =============================================================================


def test_os_callback_marshalling_error(monty_run: RunMonty, test_dir: Path):
    """An os= callback returning an unconvertible value surfaces inside Monty
    as a `TypeError` wrapped in `MontyRuntimeError`, and the MountDir remains
    usable afterwards."""
    md = MountDir('/data', str(test_dir), mode='read-only')

    def os_cb(func: object, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        return object()  # unconvertible — surfaces inside Monty as TypeError

    # Path is outside the mount so it falls through to the os= fallback.
    code = "from pathlib import Path; Path('/outside/path.txt').exists()"
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, mount=md, os=os_cb)
    assert isinstance(exc_info.value.exception(), TypeError)
    assert_mount_reusable(monty_run, md)


def test_os_callback_lone_surrogate_return_surfaces_inside_monty(monty_run: RunMonty, test_dir: Path):
    """An os= callback returning a lone-surrogate string surfaces inside Monty
    as a catchable `ValueError` rather than escaping as a raw `UnicodeEncodeError`."""
    md = MountDir('/data', str(test_dir), mode='read-only')

    def os_cb(func: object, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        return '\ud83d'  # unconvertible UTF-8 — surfaces as ValueError inside Monty

    # Catching inside Monty proves the error arrives as an in-VM exception rather
    # than propagating out as a raw PyErr.
    code = (
        'from pathlib import Path\n'
        'try:\n'
        "    Path('/outside/path.txt').read_text()\n"
        "    result = 'no error'\n"
        'except ValueError as e:\n'
        "    result = 'caught'\n"
        'result'
    )
    assert monty_run(code, mount=md, os=os_cb) == snapshot('caught')


def test_session_survives_os_callback_marshalling_error(pool: Monty, test_dir: Path):
    """A session keeps its state when an os= callback returns an unconvertible
    value — the error surfaces as MontyRuntimeError but the session survives."""
    md = MountDir('/data', str(test_dir), mode='read-only')

    def os_cb(func: object, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        return object()  # unconvertible — surfaces inside Monty as TypeError

    with pool.checkout() as session:
        session.feed_run('x = 42')
        code = "from pathlib import Path; Path('/outside/path.txt').exists()"
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run(code, mount=md, os=os_cb)
        assert isinstance(exc_info.value.exception(), TypeError)
        # Session state must still be intact — `x` is visible in a later snippet.
        assert session.feed_run('x') == snapshot(42)


# =============================================================================
# open() file handles across the boundary
# =============================================================================


def test_open_returns_monty_file_handle(monty_run: RunMonty, test_dir: Path):
    """`open()` returned across the boundary surfaces as `MontyFileHandle`, not a stringified repr."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    f = monty_run("open('/data/hello.txt')", mount=md)
    assert isinstance(f, MontyFileHandle)
    assert f.path == snapshot('/data/hello.txt')
    assert f.mode == snapshot('r')
    assert f.position == snapshot(0)
    assert (f.binary, f.readable, f.writable) == snapshot((False, True, False))
    assert repr(f) == snapshot("MontyFileHandle(path='/data/hello.txt', mode='r')")


def test_open_binary_write_handle_attrs(monty_run: RunMonty, test_dir: Path):
    """Mode-derived attributes reflect the open() mode string for binary/write opens."""
    md = MountDir('/data', str(test_dir), mode='read-write')
    f = monty_run("open('/data/out.bin', 'wb')", mount=md)
    assert isinstance(f, MontyFileHandle)
    assert f.mode == snapshot('wb')
    assert (f.binary, f.readable, f.writable) == snapshot((True, False, True))
