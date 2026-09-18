"""Tests for OS function calls dispatched to the `os=` callback.

These tests verify that filesystem, environment, and clock operations reach
the host `os=` callback with the right function name and arguments, and that
return values from the host are properly converted and used by Monty code.
"""

from __future__ import annotations

import datetime
import random
import time
from pathlib import PurePosixPath
from typing import Any
from zoneinfo import ZoneInfo

import pytest
from conftest import CALL_HOST, RunMonty
from inline_snapshot import snapshot

from pydantic_monty import NOT_HANDLED, Monty, MontyFileHandle, MontyRuntimeError, StatResult

# =============================================================================
# Basic os= callback dispatch
# =============================================================================


def test_os_basic(monty_run: RunMonty):
    """os receives function name and args, return value is used."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> bool:
        calls.append((name, args))
        return True

    result = monty_run('from pathlib import Path; Path("/tmp/test.txt").exists()', os=os_handler)

    assert result is True
    assert calls == snapshot([('Path.exists', (PurePosixPath('/tmp/test.txt'),))])


@pytest.mark.parametrize('cwd', ['/', '/data'])
def test_os_callback_paths_are_normalized(monty_run: RunMonty, cwd: str):
    """Callbacks receive canonical paths for relative inputs and both rename endpoints."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        calls.append((name, args))
        if name == 'Path.iterdir':
            return []
        if name == 'open':
            return MontyFileHandle(str(args[0]), 'r')
        if name == 'Path.read_text':
            return 'hello'
        return True

    result = monty_run(
        """
import os
from pathlib import Path
Path('sub/../file.txt').exists()
Path('/other//sub/../file.txt').exists()
os.listdir()
os.rename('./sub/../src', '../dst')
open('./sub//../file.txt').read()
""",
        cwd=cwd,
        os=os_handler,
    )
    assert result == 'hello'
    assert calls == [
        ('Path.exists', (PurePosixPath(cwd) / 'file.txt',)),
        ('Path.exists', (PurePosixPath('/other/file.txt'),)),
        ('Path.iterdir', (PurePosixPath(cwd),)),
        ('Path.rename', (PurePosixPath(cwd) / 'src', PurePosixPath('/dst'))),
        ('open', (PurePosixPath(cwd) / 'file.txt', 'r')),
        ('Path.read_text', (PurePosixPath(cwd) / 'file.txt',)),
    ]


def test_relative_paths_survive_os_callbacks(monty_run: RunMonty):
    """Python results retain relative spelling while callbacks see absolute requests."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        calls.append((name, args))
        if name == 'Path.iterdir':
            return [PurePosixPath('/data/file.txt')]
        assert name == 'open'
        return MontyFileHandle(str(args[0]), 'r')

    result = monty_run(
        """
from pathlib import Path
([str(p) for p in Path('.').iterdir()],
 [str(p) for p in Path('sub/..').iterdir()],
 open('./file.txt').name,
 Path('./file.txt').open().name,
 open(b'./file.txt').name)
""",
        cwd='/data',
        os=os_handler,
    )
    assert result == (['file.txt'], ['sub/../file.txt'], './file.txt', 'file.txt', b'./file.txt')
    assert calls == [
        ('Path.iterdir', (PurePosixPath('/data'),)),
        ('Path.iterdir', (PurePosixPath('/data'),)),
        ('open', (PurePosixPath('/data/file.txt'), 'r')),
        ('open', (PurePosixPath('/data/file.txt'), 'r')),
        ('open', (PurePosixPath('/data/file.txt'), 'r')),
    ]


@pytest.mark.parametrize('with_callback', [False, True])
@pytest.mark.parametrize(
    'operation, message',
    [
        ('open(path)', 'embedded null byte'),
        ('Path(path).read_text()', 'embedded null byte'),
        ('os.stat(path)', 'stat: embedded null character in path'),
        ('os.chdir(path)', 'chdir: embedded null character in path'),
        ("os.rename(path, 'dst')", 'rename: embedded null character in src'),
        ("os.rename('src', path)", 'rename: embedded null character in dst'),
    ],
)
def test_nul_paths_rejected_before_callback(monty_run: RunMonty, with_callback: bool, operation: str, message: str):
    """Cancelled NUL components raise without dispatching, even with no mounts."""
    calls: list[Any] = []

    def os_handler(*, args: tuple[Any, ...], **_: Any) -> bool:
        calls.append(args)
        return True

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(
            'import os\nfrom pathlib import Path\n' + operation,
            inputs={'path': 'bad\0/../x'},
            os=os_handler if with_callback else None,
        )
    assert str(exc_info.value) == f'ValueError: {message}'
    result = monty_run(
        'from pathlib import Path\np = Path(path)\n(p.exists(), p.is_file(), p.is_dir(), p.is_symlink())',
        inputs={'path': 'bad\0/../x'},
        os=os_handler if with_callback else None,
    )
    assert result == (False, False, False, False)
    assert calls == []


@pytest.mark.parametrize('with_callback', [False, True])
@pytest.mark.parametrize('code, path', [('import os\nos.listdir()', '/'), ("open('./x')", '/x'), ("open('')", '')])
def test_no_handler_uses_normalized_path(monty_run: RunMonty, with_callback: bool, code: str, path: str):
    """Missing and declining callbacks report the same normalized permission error."""

    def os_handler(**_: object) -> object:
        return NOT_HANDLED

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, os=os_handler if with_callback else None)
    assert str(exc_info.value) == f'PermissionError: Permission denied: {path!r}'


def test_path_concatenation(monty_run: RunMonty):
    """Path concatenation with / operator produces the correct path argument."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> bool:
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> bool:
        calls.append(name)
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> bool | str | None:
        calls.append(name)
        match name:
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'Path.stat':
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
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

    def os_callback(*, name: str, args: tuple[Any, ...], kwargs: dict[str, Any], **_: Any) -> object:
        del name, args, kwargs
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

    def os_callback(*, name: str, args: tuple[Any, ...], kwargs: dict[str, Any], **_: Any) -> object:
        del name, args, kwargs
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> str | None:
        calls.append((name, args))
        if name == 'os.getenv':
            key, default = args
            env = {'HOME': '/home/user', 'USER': 'testuser'}
            return env.get(key, default)
        return None

    result = monty_run('import os; os.getenv("HOME")', os=os_handler)
    assert result == snapshot('/home/user')
    assert calls == snapshot([('os.getenv', ('HOME', None))])


def test_os_getenv_callback_missing(monty_run: RunMonty):
    """os.getenv() returns None for missing env var when no default."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> str | None:
        if name == 'os.getenv':
            key, default = args
            env: dict[str, str] = {}
            return env.get(key, default)
        return None

    result = monty_run('import os; os.getenv("NONEXISTENT")', os=os_handler)
    assert result is None


def test_os_getenv_callback_with_default(monty_run: RunMonty):
    """os.getenv() forwards the default and uses it when the env var is missing."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> str | None:
        calls.append(args)
        if name == 'os.getenv':
            key, default = args
            env: dict[str, str] = {}
            return env.get(key, default)
        return None

    result = monty_run('import os; os.getenv("NONEXISTENT", "default_value")', os=os_handler)
    assert result == snapshot('default_value')
    assert calls == snapshot([('NONEXISTENT', 'default_value')])


# =============================================================================
# Clock functions (date.today / datetime.now), under `datetime='call_host'`
# =============================================================================


def test_date_today_callback(monty_run: RunMonty):
    """date.today() works through the direct os callback with no arguments."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> datetime.date | None:
        if name == 'date.today':
            assert args == ()
            return datetime.date(2024, 1, 15)
        return None

    result = monty_run('from datetime import date; date.today()', os=os_handler, checkout=CALL_HOST)
    assert (type(result).__name__, repr(result)) == snapshot(('date', 'datetime.date(2024, 1, 15)'))


def test_datetime_now_callback_naive(monty_run: RunMonty):
    """datetime.now() passes None as the timezone argument."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> datetime.datetime | None:
        if name == 'datetime.now':
            (tzinfo,) = args
            assert tzinfo is None
            return datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)
        return None

    result = monty_run('from datetime import datetime; datetime.now()', os=os_handler, checkout=CALL_HOST)
    assert (type(result).__name__, repr(result)) == snapshot(
        ('datetime', 'datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)')
    )


def test_datetime_now_callback_with_timezone(monty_run: RunMonty):
    """datetime.now() works through the direct os callback and receives tzinfo."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> datetime.datetime | None:
        if name == 'datetime.now':
            (tzinfo,) = args
            assert tzinfo == datetime.timezone.utc
            return datetime.datetime(2024, 1, 15, 10, 30, 5, 123456, tzinfo=tzinfo)
        return None

    code = 'from datetime import datetime, timezone; datetime.now(timezone.utc)'
    result = monty_run(code, os=os_handler, checkout=CALL_HOST)
    assert (type(result).__name__, repr(result)) == snapshot(
        (
            'datetime',
            'datetime.datetime(2024, 1, 15, 10, 30, 5, 123456, tzinfo=datetime.timezone.utc)',
        )
    )


def test_time_time_callback(monty_run: RunMonty):
    """time.time() reaches the callback with no arguments and returns a float."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> float | None:
        if name == 'time.time':
            assert args == ()
            return 1700000000.5
        return None

    assert monty_run('import time; time.time()', os=os_handler, checkout=CALL_HOST) == snapshot(1700000000.5)


# =============================================================================
# Sleeping (time.sleep / asyncio.sleep), under `sleep='call_host'`
# =============================================================================


def test_time_sleep_callback(monty_run: RunMonty):
    """time.sleep() passes the delay as float seconds and evaluates to None."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        calls.append((name, args))
        # the host decides how long to wait; waiting not at all is a valid choice
        return None

    assert monty_run('import time; time.sleep(1.5) is None', os=os_handler, checkout=CALL_HOST) == snapshot(True)
    assert calls == snapshot([('time.sleep', (1.5,))])


def test_time_sleep_can_be_refused(monty_run: RunMonty):
    """A host that declines the wait leaves the sandbox with monty's own error."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        return NOT_HANDLED

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('import time; time.sleep(30)', os=os_handler, checkout=CALL_HOST)
    assert str(exc_info.value) == snapshot("RuntimeError: 'time.sleep' is not supported in this environment")


def test_asyncio_sleep_callback(monty_run: RunMonty):
    """asyncio.sleep() passes only the delay; the await produces `result` whatever the host returns."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], is_async: bool, **_: Any) -> Any:
        calls.append((name, args))
        assert is_async is False
        return 'ignored'

    code = "import asyncio; asyncio.run(asyncio.sleep(0.25, 'woken'))"
    assert monty_run(code, os=os_handler, checkout=CALL_HOST) == snapshot('woken')
    assert calls == snapshot([('asyncio.sleep', (0.25,))])


def test_asyncio_sleep_result_stays_in_the_sandbox(monty_run: RunMonty):
    """`result` never crosses the host boundary, so values with no wire form survive."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        return None

    code = 'import asyncio\ndef f():\n    return 42\nasyncio.run(asyncio.sleep(0, f))()'
    assert monty_run(code, os=os_handler, checkout=CALL_HOST) == snapshot(42)


def test_async_os_callback_requires_async_monty(pool: Monty):
    """The sync pool has no event loop to run a coroutine answer on; the refusal poisons the checkout."""

    async def os_handler(**_: Any) -> Any:
        return None

    with pool.checkout(sleep='call_host') as session:
        with pytest.raises(RuntimeError) as exc_info:
            session.feed_run('import time; time.sleep(0)', os=os_handler)
        assert str(exc_info.value) == snapshot('async os callbacks require AsyncMonty')
        # the discarded checkout is not reusable
        with pytest.raises(RuntimeError):
            session.feed_run('1 + 1')


# =============================================================================
# Entropy (os.urandom / random)
# =============================================================================


def test_os_urandom_callback(monty_run: RunMonty):
    """os.urandom(n) reaches the host as `os.urandom` with the byte count."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> bytes:
        calls.append((name, args))
        return bytes(range(args[0]))

    result = monty_run('import os\nos.urandom(4)', os=os_handler)
    assert result == snapshot(b'\x00\x01\x02\x03')
    assert calls == snapshot([('os.urandom', (4,))])


def test_random_unseeded_draws_never_call_the_host(monty_run: RunMonty):
    """An unseeded generator seeds itself from the worker's entropy; two sessions disagree."""

    def os_handler(*, name: str, **_: Any) -> bytes:
        raise AssertionError(f'unexpected OS call {name}')

    code = 'import random\n[random.random(), random.Random().random()]'
    first = monty_run(code, os=os_handler)
    second = monty_run(code, os=os_handler)
    assert all(0.0 <= x < 1.0 for x in first + second)
    assert first != second


def test_random_seeded_never_calls_host(monty_run: RunMonty):
    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> bytes:
        raise AssertionError(f'unexpected OS call {name}')

    assert monty_run('import random\nrandom.seed(42)\nrandom.random()', os=os_handler) == snapshot(0.6394267984578837)


def test_random_seed_persists_across_feeds(pool: Monty):
    """The module-level generator is session state, like the globals."""
    with pool.checkout() as session:
        session.feed_run('import random\nrandom.seed(5)')
        assert session.feed_run('import random\nrandom.random()') == snapshot(0.6229016948897019)


@pytest.mark.parametrize('expression', ['random.Random', 'type(random.Random(1))'])
def test_random_type_returns_repr(monty_run: RunMonty, expression: str):
    """The sandbox's Random class crosses as a string, not a host constructor."""
    assert monty_run(f'import random\n{expression}') == "<class 'random.Random'>"


def test_random_instance_returns_repr(monty_run: RunMonty):
    """Returning a generator exposes only its repr."""
    result = monty_run('import random\nrandom.Random(1)')
    assert isinstance(result, str)
    assert result.startswith('<random.Random object at 0x')
    assert result.endswith('>')


# =============================================================================
# os.environ tests
# =============================================================================


def test_os_environ_key_access(monty_run: RunMonty):
    """os.environ['KEY'] works correctly after getting environ dict."""
    calls: list[str] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        calls.append(name)
        if name == 'os.environ':
            return {'HOME': '/home/user', 'USER': 'testuser'}
        return None

    result = monty_run("import os; os.environ['HOME']", os=os_handler)
    assert result == snapshot('/home/user')
    assert calls == snapshot(['os.environ'])


def test_os_environ_key_missing_raises(monty_run: RunMonty):
    """os.environ['MISSING'] raises KeyError."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'os.environ':
            return {}
        return None

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("import os; os.environ['MISSING']", os=os_handler)
    assert str(exc_info.value) == snapshot('KeyError: MISSING')


def test_os_environ_get_method(monty_run: RunMonty):
    """os.environ.get() works correctly."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'os.environ':
            return {'HOME': '/home/user'}
        return None

    result = monty_run("import os; os.environ.get('HOME')", os=os_handler)
    assert result == snapshot('/home/user')


def test_os_environ_get_with_default(monty_run: RunMonty):
    """os.environ.get() with default for missing key."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'os.environ':
            return {}
        return None

    result = monty_run("import os; os.environ.get('MISSING', 'default')", os=os_handler)
    assert result == snapshot('default')


def test_os_environ_len(monty_run: RunMonty):
    """len(os.environ) returns correct count."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'os.environ':
            return {'A': '1', 'B': '2', 'C': '3'}
        return None

    result = monty_run('import os; len(os.environ)', os=os_handler)
    assert result == snapshot(3)


def test_os_environ_contains(monty_run: RunMonty):
    """'KEY' in os.environ works correctly."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'os.environ':
            return {'HOME': '/home/user'}
        return None

    result = monty_run("import os; ('HOME' in os.environ, 'MISSING' in os.environ)", os=os_handler)
    assert result == snapshot((True, False))


def test_os_environ_keys(monty_run: RunMonty):
    """os.environ.keys() returns keys."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'os.environ':
            return {'HOME': '/home', 'USER': 'test'}
        return None

    result = monty_run('import os; list(os.environ.keys())', os=os_handler)
    assert set(result) == snapshot({'HOME', 'USER'})


def test_os_environ_values(monty_run: RunMonty):
    """os.environ.values() returns values."""

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if name == 'os.environ':
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> int | None:
        if name == 'Path.write_text':
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> int | None:
        calls.append((name, args))
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

    def os_handler(*, name: str, args: tuple[Any, ...], kwargs: dict[str, Any], **_: Any) -> None:
        calls.append((name, args, kwargs))
        return None

    monty_run(f'from pathlib import Path; Path("/tmp/newdir").{call}', os=os_handler)

    assert calls == [('Path.mkdir', (PurePosixPath('/tmp/newdir'),), expected_kwargs)]


def test_path_remove_and_rename_callbacks(monty_run: RunMonty):
    """unlink(), rmdir(), and rename() reach the callback with the right paths."""
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> None:
        calls.append((name, args))
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

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        operations.append((name, args))
        match name:
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


# =============================================================================
# Auto OS calls: the `checkout()` kwargs choosing what the sandbox answers itself
# =============================================================================


def test_datetime_default_reads_worker_clock(monty_run: RunMonty):
    """With no `os=` handler at all, the worker's own clock answers."""
    before = datetime.datetime.now()
    result = monty_run('from datetime import datetime\ndatetime.now()')
    after = datetime.datetime.now()
    assert before - datetime.timedelta(seconds=60) <= result <= after + datetime.timedelta(seconds=60)


def test_datetime_fixed_naive_is_utc(monty_run: RunMonty):
    """A naive datetime is the sandbox's wall clock, in UTC, so it comes back exactly."""
    frozen = datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)
    code = (
        'import time\nfrom datetime import date, datetime, timezone\n'
        '(datetime.now(), date.today(), time.time(), datetime.now(timezone.utc), datetime.now() == datetime.now())'
    )
    result = monty_run(code, checkout={'datetime': frozen})
    assert result == snapshot(
        (
            datetime.datetime(2024, 1, 15, 10, 30, 5, 123456),
            datetime.date(2024, 1, 15),
            1705314605.123456,
            datetime.datetime(2024, 1, 15, 10, 30, 5, 123456, tzinfo=datetime.timezone.utc),
            True,
        )
    )


def test_datetime_fixed_aware_uses_its_offset(monty_run: RunMonty):
    """An aware datetime is that instant, with its offset as the sandbox's local zone."""
    frozen = datetime.datetime(2024, 1, 15, 10, 30, 5, tzinfo=datetime.timezone(datetime.timedelta(hours=2)))
    code = 'import time\nfrom datetime import datetime, timezone\n(datetime.now(), datetime.now(timezone.utc), time.time())'
    result = monty_run(code, checkout={'datetime': frozen})
    assert result == snapshot(
        (
            datetime.datetime(2024, 1, 15, 10, 30, 5),
            datetime.datetime(2024, 1, 15, 8, 30, 5, tzinfo=datetime.timezone.utc),
            1705307405.0,
        )
    )


def test_datetime_fixed_zoneinfo(monty_run: RunMonty):
    """A zone that needs the date to resolve its offset works too."""
    frozen = datetime.datetime(2024, 7, 1, 12, 0, tzinfo=ZoneInfo('Europe/Paris'))
    code = 'from datetime import datetime, timezone\n(datetime.now(), datetime.now(timezone.utc))'
    result = monty_run(code, checkout={'datetime': frozen})
    assert result == snapshot(
        (
            datetime.datetime(2024, 7, 1, 12, 0),
            datetime.datetime(2024, 7, 1, 10, 0, tzinfo=datetime.timezone.utc),
        )
    )


@pytest.mark.parametrize(
    ('value', 'error', 'message'),
    [
        ('later', ValueError, "datetime must be 'system', 'call_host' or a datetime.datetime, got 'later'"),
        (123, TypeError, "datetime must be 'system', 'call_host' or a datetime.datetime, not int"),
        (
            datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(microseconds=500))),
            ValueError,
            'datetime utcoffset must be a whole number of seconds',
        ),
    ],
)
def test_datetime_invalid(pool: Monty, value: Any, error: type[Exception], message: str):
    with pytest.raises(error) as exc_info:
        pool.checkout(datetime=value)
    assert str(exc_info.value) == message


def test_sleep_zero_returns_at_once(monty_run: RunMonty):
    start = time.monotonic()
    code = "import asyncio, time\ntime.sleep(3600)\nasyncio.run(asyncio.sleep(3600, 'woken'))"
    assert monty_run(code, checkout={'sleep': 'zero'}) == snapshot('woken')
    assert time.monotonic() - start < 5


def test_sandbox_sleep_clamp(monty_run: RunMonty):
    """The default waits in the worker; the clamp cuts a long sleep short."""
    start = time.monotonic()
    code = "import asyncio, time\nt = time.time()\ntime.sleep(3600)\nasyncio.run(asyncio.sleep(3600, 'woken'))\ntime.time() >= t"
    assert monty_run(code, checkout={'sandbox_sleep_clamp': 0.001}) == snapshot(True)
    assert time.monotonic() - start < 5
    assert monty_run('import time\ntime.sleep(0.001)', checkout={'sandbox_sleep_clamp': float('inf')}) is None


def test_sandbox_sleeps_overlap(monty_run: RunMonty):
    """Gathered sandbox sleeps are timers served while the other tasks run, so they overlap."""
    code = (
        'import asyncio, time\n'
        'async def w(n):\n'
        '    await asyncio.sleep(0.05, n)\n'
        '    return n * 2\n'
        'async def main():\n'
        '    return await asyncio.gather(w(1), w(2), w(3))\n'
        't = time.time()\n'
        'r = asyncio.run(main())\n'
        '(r, time.time() - t < 0.14)'
    )
    assert monty_run(code) == snapshot(([2, 4, 6], True))


@pytest.mark.parametrize(
    ('value', 'error', 'message'),
    [
        (-1, ValueError, 'invalid sandbox_sleep_clamp: cannot convert float seconds to Duration: value is negative'),
        (
            float('nan'),
            ValueError,
            'invalid sandbox_sleep_clamp: cannot convert float seconds to Duration: value is either too big or NaN',
        ),
        ('1', TypeError, 'sandbox_sleep_clamp must be a number of seconds, not str'),
        (True, TypeError, 'sandbox_sleep_clamp must be a number of seconds, not bool'),
    ],
)
def test_sandbox_sleep_clamp_invalid(pool: Monty, value: Any, error: type[Exception], message: str):
    with pytest.raises(error) as exc_info:
        pool.checkout(sandbox_sleep_clamp=value)
    assert str(exc_info.value) == message


def test_sleep_call_host_reaches_os(monty_run: RunMonty):
    calls: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        calls.append((name, args))
        return None

    assert monty_run('import time\ntime.sleep(1.5)', os=os_handler, checkout={'sleep': 'call_host'}) is None
    assert calls == snapshot([('time.sleep', (1.5,))])


@pytest.mark.parametrize(
    ('value', 'error', 'message'),
    [
        ('forever', ValueError, "sleep must be 'sandbox_sleep', 'zero' or 'call_host', got 'forever'"),
        (0, TypeError, 'sleep must be a str'),
    ],
)
def test_sleep_invalid(pool: Monty, value: Any, error: type[Exception], message: str):
    with pytest.raises(error) as exc_info:
        pool.checkout(sleep=value)
    assert str(exc_info.value) == message


@pytest.mark.parametrize('seed', [42, -42, 2**70, 1.5, 'abc', b'abc'])
def test_random_start_seed_matches_random_seed(monty_run: RunMonty, seed: Any):
    """`{'seed': s}` starts the module generator exactly as `random.seed(s)` would."""
    expected = random.Random(seed)
    code = 'import random\n[random.random(), random.randint(1, 100)]'
    assert monty_run(code, checkout={'random_start': {'seed': seed}}) == [expected.random(), expected.randint(1, 100)]


def test_random_start_seed_persists_and_is_overridable(pool: Monty):
    """The seed applies to the first draw whichever feed makes it; `random.seed()` still wins."""
    with pool.checkout(random_start={'seed': 42}) as session:
        session.feed_run('import random')
        assert session.feed_run('random.random()') == snapshot(0.6394267984578837)
        session.feed_run('random.seed(5)')
        assert session.feed_run('random.random()') == snapshot(0.6229016948897019)


def test_random_start_seed_instances_are_deterministic(monty_run: RunMonty):
    """Unseeded instances take states derived from the seed: repeatable, but distinct."""
    code = 'import random\n[random.Random().random(), random.Random().random(), random.random()]'
    first = monty_run(code, checkout={'random_start': {'seed': 42}})
    second = monty_run(code, checkout={'random_start': {'seed': 42}})
    assert first == second
    assert len(set(first)) == 3
    assert first[2] == snapshot(0.6394267984578837)


@pytest.mark.parametrize(
    ('value', 'error', 'message'),
    [
        ('seeded', ValueError, "random_start must be 'random' or {'seed': int | float | str | bytes}, got 'seeded'"),
        (
            {'sead': 1},
            ValueError,
            "random_start must be 'random' or {'seed': int | float | str | bytes}, got {'sead': 1}",
        ),
        ({'seed': True}, TypeError, 'random_start seed must be an int, float, str or bytes, not bool'),
        ({'seed': None}, TypeError, 'random_start seed must be an int, float, str or bytes, not NoneType'),
        (1, TypeError, "random_start must be 'random' or {'seed': int | float | str | bytes}, not int"),
    ],
)
def test_random_start_invalid(pool: Monty, value: Any, error: type[Exception], message: str):
    with pytest.raises(error) as exc_info:
        pool.checkout(random_start=value)
    assert str(exc_info.value) == message
