"""Tests for OS function calls dispatched to the `os=` callback.

These tests verify that filesystem, environment, and clock operations reach
the host `os=` callback with the right function name and arguments, and that
return values from the host are properly converted and used by Monty code.
"""

from __future__ import annotations

import datetime
from pathlib import PurePosixPath
from typing import Any

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import NOT_HANDLED, MontyRuntimeError, StatResult

# =============================================================================
# Basic os= callback dispatch
# =============================================================================


def test_os_basic(monty_run: RunMonty):
    """os receives function name and args, return value is used."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        calls.append((function_name, args))
        return True

    result = monty_run('from pathlib import Path; Path("/tmp/test.txt").exists()', os=os_handler)

    assert result is True
    assert calls == snapshot([('Path.exists', (PurePosixPath('/tmp/test.txt'),))])


def test_path_concatenation(monty_run: RunMonty):
    """Path concatenation with / operator produces the correct path argument."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        calls.append(args)
        return False

    code = """
from pathlib import Path
base = Path('/home')
full = base / 'user' / 'documents' / 'file.txt'
full.exists()
"""
    monty_run(code, os=os_handler)
    assert calls == snapshot([(PurePosixPath('/home/user/documents/file.txt'),)])


def test_multiple_path_calls(monty_run: RunMonty):
    """Multiple Path method calls reach the callback in sequence."""
    calls: list[str] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        calls.append(function_name)
        return True

    code = """
from pathlib import Path
p = Path('/tmp/test.txt')
exists = p.exists()
is_file = p.is_file()
(exists, is_file)
"""
    result = monty_run(code, os=os_handler)
    assert result == snapshot((True, True))
    assert calls == snapshot(['Path.exists', 'Path.is_file'])


def test_os_multiple_calls(monty_run: RunMonty):
    """os is called for each OS operation, including inside conditionals."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool | str | None:
        calls.append(function_name)
        match function_name:
            case 'Path.exists':
                return True
            case 'Path.read_text':
                return 'file contents'
            case _:
                return None

    code = """
from pathlib import Path
p = Path('/tmp/test.txt')
if p.exists():
    result = p.read_text()
else:
    result = 'not found'
result
"""
    result = monty_run(code, os=os_handler)

    assert result == snapshot('file contents')
    assert calls == snapshot(['Path.exists', 'Path.read_text'])


# =============================================================================
# stat() result round-trip (Python -> Monty -> Python)
# =============================================================================


def test_os_stat(monty_run: RunMonty):
    """os can return stat_result for Path.stat(), accessible by field and index."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'Path.stat':
            return StatResult.file_stat(1024, 0o644, 1234567890.0)
        return None

    code = """
from pathlib import Path
info = Path('/tmp/file.txt').stat()
(info.st_mode, info.st_size, info[6])
"""
    result = monty_run(code, os=os_handler)

    assert result == snapshot((0o100_644, 1024, 1024))


def test_stat_result_returned_from_monty(monty_run: RunMonty):
    """stat_result returned from Monty is accessible in Python."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        return StatResult.file_stat(2048, 0o100_755, 1700000000.0)

    stat_result = monty_run('from pathlib import Path\nPath("/tmp/file.txt").stat()', os=os_handler)

    # Access attributes on the returned namedtuple
    assert stat_result.st_mode == snapshot(0o100_755)
    assert stat_result.st_size == snapshot(2048)
    assert stat_result.st_mtime == snapshot(1700000000.0)

    # Index access works too
    assert stat_result[0] == snapshot(0o100_755)  # st_mode
    assert stat_result[6] == snapshot(2048)  # st_size


def test_stat_result_repr(monty_run: RunMonty):
    """stat_result repr shows field names and values."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        return StatResult.file_stat(512, 0o644, 0.0)

    result = monty_run('from pathlib import Path\nPath("/tmp/file.txt").stat()', os=os_handler)

    assert repr(result) == snapshot(
        'StatResult(st_mode=33188, st_ino=0, st_dev=0, st_nlink=1, st_uid=0, st_gid=0, st_size=512, st_atime=0.0, st_mtime=0.0, st_ctime=0.0)'
    )
    # Should be a tuple subclass
    assert len(result) == 10
    assert isinstance(result, tuple)


# =============================================================================
# Unhandled OS calls
# =============================================================================


def test_os_not_provided_error(monty_run: RunMonty):
    """The per-call default error is raised when an OS call is made without os."""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('from pathlib import Path; Path("/tmp").exists()')
    assert str(exc_info.value) == snapshot("PermissionError: Permission denied: '/tmp'")


def test_not_callable(monty_run: RunMonty):
    """Passing a non-callable os raises TypeError."""
    with pytest.raises(TypeError) as exc_info:
        monty_run('from pathlib import Path; Path("/tmp/test.txt").exists()', os=123)  # pyright: ignore[reportArgumentType]
    assert exc_info.value.args[0] == snapshot("'int' object is not callable")


def test_not_handled_sentinel_filesystem_callback(monty_run: RunMonty):
    """Returning NOT_HANDLED from an os callback uses the filesystem fallback error."""

    def os_callback(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> object:
        del function_name, args, kwargs
        return NOT_HANDLED

    code = """
from pathlib import Path
message = None
try:
    Path('/tmp').exists()
except PermissionError as exc:
    message = str(exc)
message
"""
    result = monty_run(code, os=os_callback)

    assert result == snapshot("Permission denied: '/tmp'")


def test_not_handled_sentinel_non_filesystem_callback(monty_run: RunMonty):
    """Returning NOT_HANDLED from an os callback uses the non-filesystem fallback error."""

    def os_callback(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> object:
        del function_name, args, kwargs
        return NOT_HANDLED

    code = """
import os
message = None
try:
    os.getenv('HOME')
except RuntimeError as exc:
    message = str(exc)
message
"""
    result = monty_run(code, os=os_callback)

    assert result == snapshot("'os.getenv' is not supported in this environment")


# =============================================================================
# os.getenv() tests
# =============================================================================


def test_os_getenv_callback(monty_run: RunMonty):
    """os.getenv() forwards key (and None default) to the callback."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> str | None:
        calls.append((function_name, args))
        if function_name == 'os.getenv':
            key, default = args
            env = {'HOME': '/home/user', 'USER': 'testuser'}
            return env.get(key, default)
        return None

    result = monty_run('import os; os.getenv("HOME")', os=os_handler)
    assert result == snapshot('/home/user')
    assert calls == snapshot([('os.getenv', ('HOME', None))])


def test_os_getenv_callback_missing(monty_run: RunMonty):
    """os.getenv() returns None for missing env var when no default."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> str | None:
        if function_name == 'os.getenv':
            key, default = args
            env: dict[str, str] = {}
            return env.get(key, default)
        return None

    result = monty_run('import os; os.getenv("NONEXISTENT")', os=os_handler)
    assert result is None


def test_os_getenv_callback_with_default(monty_run: RunMonty):
    """os.getenv() forwards the default and uses it when the env var is missing."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> str | None:
        calls.append(args)
        if function_name == 'os.getenv':
            key, default = args
            env: dict[str, str] = {}
            return env.get(key, default)
        return None

    result = monty_run('import os; os.getenv("NONEXISTENT", "default_value")', os=os_handler)
    assert result == snapshot('default_value')
    assert calls == snapshot([('NONEXISTENT', 'default_value')])


# =============================================================================
# Clock functions (date.today / datetime.now)
# =============================================================================


def test_date_today_callback(monty_run: RunMonty):
    """date.today() works through the direct os callback with no arguments."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> datetime.date | None:
        if function_name == 'date.today':
            assert args == ()
            return datetime.date(2024, 1, 15)
        return None

    result = monty_run('from datetime import date; date.today()', os=os_handler)
    assert (type(result).__name__, repr(result)) == snapshot(('date', 'datetime.date(2024, 1, 15)'))


def test_datetime_now_callback_naive(monty_run: RunMonty):
    """datetime.now() passes None as the timezone argument."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> datetime.datetime | None:
        if function_name == 'datetime.now':
            (tzinfo,) = args
            assert tzinfo is None
            return datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)
        return None

    result = monty_run('from datetime import datetime; datetime.now()', os=os_handler)
    assert (type(result).__name__, repr(result)) == snapshot(
        ('datetime', 'datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)')
    )


def test_datetime_now_callback_with_timezone(monty_run: RunMonty):
    """datetime.now() works through the direct os callback and receives tzinfo."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> datetime.datetime | None:
        if function_name == 'datetime.now':
            (tzinfo,) = args
            assert tzinfo == datetime.timezone.utc
            return datetime.datetime(2024, 1, 15, 10, 30, 5, 123456, tzinfo=tzinfo)
        return None

    result = monty_run('from datetime import datetime, timezone; datetime.now(timezone.utc)', os=os_handler)
    assert (type(result).__name__, repr(result)) == snapshot(
        (
            'datetime',
            'datetime.datetime(2024, 1, 15, 10, 30, 5, 123456, tzinfo=datetime.timezone.utc)',
        )
    )


# =============================================================================
# os.environ tests
# =============================================================================


def test_os_environ_key_access(monty_run: RunMonty):
    """os.environ['KEY'] works correctly after getting environ dict."""
    calls: list[str] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        calls.append(function_name)
        if function_name == 'os.environ':
            return {'HOME': '/home/user', 'USER': 'testuser'}
        return None

    result = monty_run("import os; os.environ['HOME']", os=os_handler)
    assert result == snapshot('/home/user')
    assert calls == snapshot(['os.environ'])


def test_os_environ_key_missing_raises(monty_run: RunMonty):
    """os.environ['MISSING'] raises KeyError."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'os.environ':
            return {}
        return None

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("import os; os.environ['MISSING']", os=os_handler)
    assert str(exc_info.value) == snapshot('KeyError: MISSING')


def test_os_environ_get_method(monty_run: RunMonty):
    """os.environ.get() works correctly."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'os.environ':
            return {'HOME': '/home/user'}
        return None

    result = monty_run("import os; os.environ.get('HOME')", os=os_handler)
    assert result == snapshot('/home/user')


def test_os_environ_get_with_default(monty_run: RunMonty):
    """os.environ.get() with default for missing key."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'os.environ':
            return {}
        return None

    result = monty_run("import os; os.environ.get('MISSING', 'default')", os=os_handler)
    assert result == snapshot('default')


def test_os_environ_len(monty_run: RunMonty):
    """len(os.environ) returns correct count."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'os.environ':
            return {'A': '1', 'B': '2', 'C': '3'}
        return None

    result = monty_run('import os; len(os.environ)', os=os_handler)
    assert result == snapshot(3)


def test_os_environ_contains(monty_run: RunMonty):
    """'KEY' in os.environ works correctly."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'os.environ':
            return {'HOME': '/home/user'}
        return None

    result = monty_run("import os; ('HOME' in os.environ, 'MISSING' in os.environ)", os=os_handler)
    assert result == snapshot((True, False))


def test_os_environ_keys(monty_run: RunMonty):
    """os.environ.keys() returns keys."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'os.environ':
            return {'HOME': '/home', 'USER': 'test'}
        return None

    result = monty_run('import os; list(os.environ.keys())', os=os_handler)
    assert set(result) == snapshot({'HOME', 'USER'})


def test_os_environ_values(monty_run: RunMonty):
    """os.environ.values() returns values."""

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        if function_name == 'os.environ':
            return {'A': '1', 'B': '2'}
        return None

    result = monty_run('import os; list(os.environ.values())', os=os_handler)
    assert set(result) == snapshot({'1', '2'})


# =============================================================================
# Path write operations
# =============================================================================


def test_path_write_text_callback(monty_run: RunMonty):
    """Path.write_text() with os callback works correctly."""
    written_files: dict[str, str] = {}

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> int | None:
        if function_name == 'Path.write_text':
            path, content = args
            written_files[str(path)] = content
            return len(content.encode('utf-8'))
        return None

    result = monty_run('from pathlib import Path; Path("/tmp/test.txt").write_text("test content")', os=os_handler)

    assert result == snapshot(12)
    assert written_files == snapshot({'/tmp/test.txt': 'test content'})


def test_path_write_bytes_callback(monty_run: RunMonty):
    """Path.write_bytes() reaches the callback with the path and raw bytes."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> int | None:
        calls.append((function_name, args))
        return 3

    result = monty_run('from pathlib import Path; Path("/tmp/data.bin").write_bytes(b"\\x00\\x01\\x02")', os=os_handler)

    assert result == snapshot(3)
    assert calls == snapshot([('Path.write_bytes', (PurePosixPath('/tmp/data.bin'), b'\x00\x01\x02'))])


@pytest.mark.parametrize(
    ('call', 'expected_kwargs'),
    [
        ('mkdir()', {'parents': False, 'exist_ok': False}),
        ('mkdir(parents=True)', {'parents': True, 'exist_ok': False}),
        ('mkdir(exist_ok=True)', {'parents': False, 'exist_ok': True}),
        ('mkdir(parents=True, exist_ok=True)', {'parents': True, 'exist_ok': True}),
    ],
)
def test_path_mkdir_kwargs_callback(monty_run: RunMonty, call: str, expected_kwargs: dict[str, bool]):
    """Path.mkdir() always reaches the host with both `parents` and `exist_ok`
    populated — defaults filled in — so the host never has to know CPython's
    defaults to interpret the call."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> None:
        calls.append((function_name, args, kwargs))
        return None

    monty_run(f'from pathlib import Path; Path("/tmp/newdir").{call}', os=os_handler)

    assert calls == [('Path.mkdir', (PurePosixPath('/tmp/newdir'),), expected_kwargs)]


def test_path_remove_and_rename_callbacks(monty_run: RunMonty):
    """unlink(), rmdir(), and rename() reach the callback with the right paths."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> None:
        calls.append((function_name, args))
        return None

    code = """
from pathlib import Path
Path('/tmp/to_delete.txt').unlink()
Path('/tmp/empty_dir').rmdir()
Path('/tmp/old.txt').rename(Path('/tmp/new.txt'))
"""
    monty_run(code, os=os_handler)

    assert calls == snapshot(
        [
            ('Path.unlink', (PurePosixPath('/tmp/to_delete.txt'),)),
            ('Path.rmdir', (PurePosixPath('/tmp/empty_dir'),)),
            ('Path.rename', (PurePosixPath('/tmp/old.txt'), PurePosixPath('/tmp/new.txt'))),
        ]
    )


def test_write_operations_callback(monty_run: RunMonty):
    """Multiple write operations work with os callback."""
    operations: list[tuple[str, tuple[Any, ...]]] = []

    def os_handler(function_name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        operations.append((function_name, args))
        match function_name:
            case 'Path.mkdir':
                return None
            case 'Path.write_text':
                return len(args[1].encode('utf-8'))
            case 'Path.exists':
                return True
            case 'Path.read_text':
                return 'file content'
            case _:
                return None

    code = """
from pathlib import Path
Path('/tmp/mydir').mkdir()
Path('/tmp/mydir/file.txt').write_text('hello')
Path('/tmp/mydir/file.txt').read_text()
"""
    result = monty_run(code, os=os_handler)

    assert result == snapshot('file content')
    assert operations == snapshot(
        [
            ('Path.mkdir', (PurePosixPath('/tmp/mydir'),)),
            ('Path.write_text', (PurePosixPath('/tmp/mydir/file.txt'), 'hello')),
            ('Path.read_text', (PurePosixPath('/tmp/mydir/file.txt'),)),
        ]
    )
