"""Tests for OS function calls (Path methods) via the start/resume API.

These tests verify that Path filesystem methods correctly yield OS calls
with the right function name and arguments, and that return values from
the host are properly converted and used by Monty code.
"""

from pathlib import PurePosixPath
from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import StatResult

# =============================================================================
# Basic OS call yielding
# =============================================================================


def test_path_exists_yields_oscall():
    """Path.exists() yields an OS call with correct function and path."""
    m = pydantic_monty.Monty('from pathlib import Path; Path("/tmp/test.txt").exists()')
    result = m.start()

    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.is_os_function is True
    assert result.function_name == snapshot('Path.exists')
    assert result.args == snapshot((PurePosixPath('/tmp/test.txt'),))
    assert result.kwargs == snapshot({})


def test_path_stat_yields_oscall():
    """Path.stat() yields an OS call."""
    m = pydantic_monty.Monty('from pathlib import Path; Path("/etc/passwd").stat()')
    result = m.start()

    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.is_os_function is True
    assert result.function_name == snapshot('Path.stat')
    assert result.args == snapshot((PurePosixPath('/etc/passwd'),))


def test_path_read_text_yields_oscall():
    """Path.read_text() yields an OS call."""
    m = pydantic_monty.Monty('from pathlib import Path; Path("/tmp/hello.txt").read_text()')
    result = m.start()

    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.is_os_function is True
    assert result.function_name == snapshot('Path.read_text')
    assert result.args == snapshot((PurePosixPath('/tmp/hello.txt'),))


# =============================================================================
# Path construction and concatenation
# =============================================================================


def test_path_concatenation():
    """Path concatenation with / operator produces correct path string."""
    code = """
from pathlib import Path
base = Path('/home')
full = base / 'user' / 'documents' / 'file.txt'
full.exists()
"""
    m = pydantic_monty.Monty(code)
    result = m.start()

    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.args == snapshot((PurePosixPath('/home/user/documents/file.txt'),))


# =============================================================================
# Resume with return values
# =============================================================================


def test_exists_resume():
    """Resuming exists() with bool returns it to Monty code."""
    m = pydantic_monty.Monty('from pathlib import Path; Path("/tmp/test.txt").exists()')
    snapshot_result = m.start()

    assert isinstance(snapshot_result, pydantic_monty.MontySnapshot)
    result = snapshot_result.resume(return_value=True)

    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output is True


def test_read_text_resume():
    """Resuming read_text() with string content returns it to Monty code."""
    code = """
from pathlib import Path
content = Path('/tmp/hello.txt').read_text()
'Content: ' + content
"""
    m = pydantic_monty.Monty(code)
    snapshot_result = m.start()

    assert isinstance(snapshot_result, pydantic_monty.MontySnapshot)
    result = snapshot_result.resume(return_value='Hello, World!')

    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot('Content: Hello, World!')


# =============================================================================
# stat() result round-trip (Python -> Monty -> Python)
# =============================================================================


def test_stat_resume_and_use_in_monty():
    """Resuming stat() with file_stat() allows Monty to access fields."""
    code = """
from pathlib import Path
info = Path('/tmp/file.txt').stat()
(info.st_mode, info.st_size, info[6])
"""
    m = pydantic_monty.Monty(code)
    snapshot_result = m.start()

    assert isinstance(snapshot_result, pydantic_monty.MontySnapshot)
    assert snapshot_result.function_name == snapshot('Path.stat')

    # Resume with a file_stat result - Monty accesses multiple fields
    result = snapshot_result.resume(return_value=StatResult.file_stat(1024, 0o100_644, 1234567890.0))

    assert isinstance(result, pydantic_monty.MontyComplete)
    # st_mode=0o100_644, st_size=1024, info[6]=st_size=1024
    assert result.output == snapshot((0o100_644, 1024, 1024))


def test_stat_result_returned_from_monty():
    """stat_result returned from Monty is accessible in Python."""
    code = """
from pathlib import Path
Path('/tmp/file.txt').stat()
"""
    m = pydantic_monty.Monty(code)
    snapshot_result = m.start()

    assert isinstance(snapshot_result, pydantic_monty.MontySnapshot)
    result = snapshot_result.resume(return_value=StatResult.file_stat(2048, 0o100_755, 1700000000.0))

    assert isinstance(result, pydantic_monty.MontyComplete)
    stat_result = result.output

    # Access attributes on the returned namedtuple
    assert stat_result.st_mode == snapshot(0o100_755)
    assert stat_result.st_size == snapshot(2048)
    assert stat_result.st_mtime == snapshot(1700000000.0)

    # Index access works too
    assert stat_result[0] == snapshot(0o100_755)  # st_mode
    assert stat_result[6] == snapshot(2048)  # st_size


def test_stat_result():
    """stat_result repr shows field names and values."""
    code = """
from pathlib import Path
Path('/tmp/file.txt').stat()
"""
    m = pydantic_monty.Monty(code)
    snapshot_result = m.start()

    assert isinstance(snapshot_result, pydantic_monty.MontySnapshot)
    result = snapshot_result.resume(return_value=StatResult.file_stat(512, 0o644, 0.0))

    assert isinstance(result, pydantic_monty.MontyComplete)
    assert repr(result.output) == snapshot(
        'StatResult(st_mode=33188, st_ino=0, st_dev=0, st_nlink=1, st_uid=0, st_gid=0, st_size=512, st_atime=0.0, st_mtime=0.0, st_ctime=0.0)'
    )
    # Should be a tuple subclass
    assert len(result.output) == 10
    assert isinstance(result.output, tuple)


# =============================================================================
# Multiple OS calls in sequence
# =============================================================================


def test_multiple_path_calls():
    """Multiple Path method calls yield multiple OS calls in sequence."""
    code = """
from pathlib import Path
p = Path('/tmp/test.txt')
exists = p.exists()
is_file = p.is_file()
(exists, is_file)
"""
    m = pydantic_monty.Monty(code)

    # First call: exists()
    result = m.start()
    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.function_name == snapshot('Path.exists')

    # Resume exists() with True
    result = result.resume(return_value=True)
    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.function_name == snapshot('Path.is_file')

    # Resume is_file() with True
    result = result.resume(return_value=True)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot((True, True))


def test_conditional_path_calls():
    """Path calls inside conditionals work correctly."""
    code = """
from pathlib import Path
p = Path('/tmp/test.txt')
if p.exists():
    content = p.read_text()
else:
    content = 'not found'
content
"""
    m = pydantic_monty.Monty(code)

    # First call: exists()
    result = m.start()
    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.function_name == snapshot('Path.exists')

    # Resume exists() with True - should trigger read_text()
    result = result.resume(return_value=True)
    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.function_name == snapshot('Path.read_text')

    # Resume read_text() with content
    result = result.resume(return_value='file contents')
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot('file contents')


# =============================================================================
# OS call vs external function distinction
# =============================================================================


def test_os_call_vs_external_function():
    """OS calls have is_os_function=True, external functions have is_os_function=False."""
    # OS call
    m1 = pydantic_monty.Monty('from pathlib import Path; Path("/tmp").exists()')
    result1 = m1.start()
    assert isinstance(result1, pydantic_monty.MontySnapshot)
    assert result1.is_os_function is True

    # External function
    m2 = pydantic_monty.Monty('my_func()', external_functions=['my_func'])
    result2 = m2.start()
    assert isinstance(result2, pydantic_monty.MontySnapshot)
    assert result2.is_os_function is False


# =============================================================================
# os in run() method
# =============================================================================


def test_os_basic():
    """os receives function name and args, return value is used."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...]) -> bool:
        calls.append((function_name, args))
        return True

    m = pydantic_monty.Monty('from pathlib import Path; Path("/tmp/test.txt").exists()')
    result = m.run(os=os_handler)

    assert result is True
    assert calls == snapshot([('Path.exists', (PurePosixPath('/tmp/test.txt'),))])


def test_os_stat():
    """os can return stat_result for Path.stat()."""

    def os_handler(function_name: str, args: tuple[Any, ...]) -> Any:
        if function_name == 'Path.stat':
            return StatResult.file_stat(1024, 0o644, 1700000000.0)
        return None

    code = """
from pathlib import Path
info = Path('/tmp/file.txt').stat()
(info.st_mode, info.st_size)
"""
    m = pydantic_monty.Monty(code)
    result = m.run(os=os_handler)

    assert result == snapshot((0o100_644, 1024))


def test_os_multiple_calls():
    """os is called for each OS operation."""
    calls: list[Any] = []

    def os_handler(function_name: str, args: tuple[Any, ...]) -> bool | str | None:
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
    m = pydantic_monty.Monty(code)
    result = m.run(os=os_handler)

    assert result == snapshot('file contents')
    assert calls == snapshot(['Path.exists', 'Path.read_text'])


def test_os_not_provided_error():
    """Error is raised when OS call is made without os."""
    import pytest

    m = pydantic_monty.Monty('from pathlib import Path; Path("/tmp").exists()')
    # When no external functions and no os, run() takes the fast path
    # and OS calls raise NotImplementedError inside Monty
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run()
    assert str(exc_info.value) == snapshot(
        "NotImplementedError: OS function 'Path.exists' not implemented with standard execution"
    )


def test_os_not_provided_error_ext_func():
    """Error is raised when OS call is made without os."""
    import pytest

    m = pydantic_monty.Monty('from pathlib import Path; Path("/tmp").exists()', external_functions=['x'])
    # When no external functions and no os, run() takes the fast path
    # and OS calls raise NotImplementedError inside Monty
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'x': int})
    assert str(exc_info.value) == snapshot("NotImplementedError: OS function 'Path.exists' not implemented")


def test_not_callable():
    """Raise NotImplementedError inside inside monty if so os"""
    m = pydantic_monty.Monty('from pathlib import Path; Path("/tmp/test.txt").exists()')

    with pytest.raises(TypeError, match="TypeError: 'int' object is not callable"):
        m.run(os=123)  # type: ignore


# =============================================================================
# os.getenv() tests
# =============================================================================


def test_os_getenv_yields_oscall():
    """os.getenv() yields an OS call with correct function and args."""
    m = pydantic_monty.Monty('import os; os.getenv("HOME")')
    result = m.start()

    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.is_os_function is True
    assert result.function_name == snapshot('os.getenv')
    assert result.args == snapshot(('HOME', None))


def test_os_getenv_with_default_yields_oscall():
    """os.getenv() with default yields an OS call with both args."""
    m = pydantic_monty.Monty('import os; os.getenv("MISSING", "fallback")')
    result = m.start()

    assert isinstance(result, pydantic_monty.MontySnapshot)
    assert result.is_os_function is True
    assert result.function_name == snapshot('os.getenv')
    assert result.args == snapshot(('MISSING', 'fallback'))


def test_os_getenv_callback():
    """os.getenv() with os works correctly."""

    def os_handler(function_name: str, args: tuple[Any, ...]) -> str | None:
        if function_name == 'os.getenv':
            key, default = args
            env = {'HOME': '/home/user', 'USER': 'testuser'}
            return env.get(key, default)
        return None

    m = pydantic_monty.Monty('import os; os.getenv("HOME")')
    result = m.run(os=os_handler)
    assert result == snapshot('/home/user')


def test_os_getenv_callback_missing():
    """os.getenv() returns None for missing env var when no default."""

    def os_handler(function_name: str, args: tuple[Any, ...]) -> str | None:
        if function_name == 'os.getenv':
            key, default = args
            env: dict[str, str] = {}
            return env.get(key, default)
        return None

    m = pydantic_monty.Monty('import os; os.getenv("NONEXISTENT")')
    result = m.run(os=os_handler)
    assert result is None


def test_os_getenv_callback_with_default():
    """os.getenv() uses default when env var is missing."""

    def os_handler(function_name: str, args: tuple[Any, ...]) -> str | None:
        if function_name == 'os.getenv':
            key, default = args
            env: dict[str, str] = {}
            return env.get(key, default)
        return None

    m = pydantic_monty.Monty('import os; os.getenv("NONEXISTENT", "default_value")')
    result = m.run(os=os_handler)
    assert result == snapshot('default_value')
