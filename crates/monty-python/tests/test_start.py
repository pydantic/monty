import tempfile
from collections.abc import Generator
from pathlib import Path, PurePosixPath
from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import NOT_HANDLED, Monty, MountDir


def test_start_no_external_functions_returns_complete():
    m = pydantic_monty.Monty('1 + 2')
    result = m.start()
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(3)


def test_start_with_external_function_returns_progress():
    m = pydantic_monty.Monty('func()')
    result = m.start()
    assert isinstance(result, pydantic_monty.FunctionSnapshot)
    assert result.script_name == snapshot('main.py')
    assert result.function_name == snapshot('func')
    assert result.args == snapshot(())
    assert result.kwargs == snapshot({})


def test_start_custom_script_name():
    m = pydantic_monty.Monty('func()', script_name='custom.py')
    result = m.start()
    assert isinstance(result, pydantic_monty.FunctionSnapshot)
    assert result.script_name == snapshot('custom.py')


def test_start_progress_resume_returns_complete():
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot(())
    assert progress.kwargs == snapshot({})

    result = progress.resume({'return_value': 42})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(42)


def test_start_progress_with_args():
    m = pydantic_monty.Monty('func(1, 2, 3)')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot((1, 2, 3))
    assert progress.kwargs == snapshot({})


def test_start_progress_with_kwargs():
    m = pydantic_monty.Monty('func(a=1, b="two")')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot(())
    assert progress.kwargs == snapshot({'a': 1, 'b': 'two'})


def test_start_progress_with_mixed_args_kwargs():
    m = pydantic_monty.Monty('func(1, 2, x="hello", y=True)')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot((1, 2))
    assert progress.kwargs == snapshot({'x': 'hello', 'y': True})


def test_start_multiple_external_calls():
    m = pydantic_monty.Monty('a() + b()')

    # First call
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('a')

    # Resume with first return value
    progress = progress.resume({'return_value': 10})
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('b')

    # Resume with second return value
    result = progress.resume({'return_value': 5})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(15)


def test_start_chain_of_external_calls():
    m = pydantic_monty.Monty('c() + c() + c()')

    call_count = 0
    progress = m.start()

    while isinstance(progress, pydantic_monty.FunctionSnapshot | pydantic_monty.FutureSnapshot):
        assert isinstance(progress, pydantic_monty.FunctionSnapshot), 'Expected FunctionSnapshot'
        assert progress.function_name == snapshot('c')
        call_count += 1
        progress = progress.resume({'return_value': call_count})

    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(6)  # 1 + 2 + 3
    assert call_count == snapshot(3)


def test_start_with_inputs():
    m = pydantic_monty.Monty('process(x)', inputs=['x'])
    progress = m.start(inputs={'x': 100})
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('process')
    assert progress.args == snapshot((100,))


def test_start_with_limits():
    m = pydantic_monty.Monty('1 + 2')
    limits = pydantic_monty.ResourceLimits(max_allocations=1000)
    result = m.start(limits=limits)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(3)


def test_start_with_print_callback():
    output: list[tuple[str, str]] = []

    def callback(stream: str, text: str) -> None:
        output.append((stream, text))

    m = pydantic_monty.Monty('print("hello")')
    result = m.start(print_callback=callback)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert output == snapshot([('stdout', 'hello'), ('stdout', '\n')])


def test_start_resume_cannot_be_called_twice():
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    # First resume succeeds
    progress.resume({'return_value': 1})

    # Second resume should fail
    with pytest.raises(RuntimeError) as exc_info:
        progress.resume({'return_value': 2})
    assert exc_info.value.args[0] == snapshot('Progress already resumed')


def test_start_complex_return_value():
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    result = progress.resume({'return_value': {'a': [1, 2, 3], 'b': {'nested': True}}})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot({'a': [1, 2, 3], 'b': {'nested': True}})


def test_start_resume_with_none():
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    result = progress.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output is None


def test_progress_repr():
    m = pydantic_monty.Monty('func(1, x=2)')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert repr(progress) == snapshot(
        "FunctionSnapshot(script_name='main.py', function_name='func', args=(1,), kwargs={'x': 2})"
    )


def test_complete_repr():
    m = pydantic_monty.Monty('42')
    result = m.start()
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert repr(result) == snapshot('MontyComplete(output=42)')


def test_start_can_reuse_monty_instance():
    m = pydantic_monty.Monty('func(x)', inputs=['x'])

    # First run
    progress1 = m.start(inputs={'x': 1})
    assert isinstance(progress1, pydantic_monty.FunctionSnapshot)
    assert progress1.args == snapshot((1,))
    result1 = progress1.resume({'return_value': 10})
    assert isinstance(result1, pydantic_monty.MontyComplete)
    assert result1.output == snapshot(10)

    # Second run with different input
    progress2 = m.start(inputs={'x': 2})
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    assert progress2.args == snapshot((2,))
    result2 = progress2.resume({'return_value': 20})
    assert isinstance(result2, pydantic_monty.MontyComplete)
    assert result2.output == snapshot(20)


@pytest.mark.parametrize(
    'code,expected',
    [
        ('1', 1),
        ('"hello"', 'hello'),
        ('[1, 2, 3]', [1, 2, 3]),
        ('{"a": 1}', {'a': 1}),
        ('None', None),
        ('True', True),
    ],
)
def test_start_returns_complete_for_various_types(code: str, expected: Any):
    m = pydantic_monty.Monty(code)
    result = m.start()
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == expected


def test_start_progress_resume_with_exception_caught():
    """Test that resuming with an exception is caught by try/except."""
    code = """
try:
    result = external_func()
except ValueError:
    caught = True
caught
"""
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    # Resume with an exception using keyword argument
    result = progress.resume({'exception': ValueError('test error')})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(True)


def test_invalid_exception():
    code = 'foo()'
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    with pytest.raises(TypeError) as exc_info:
        progress.resume({'exception': 123})  # pyright: ignore[reportArgumentType]
    assert exc_info.value.args[0] == snapshot("'int' object is not an instance of 'BaseException'")


def test_start_progress_resume_exception_propagates_uncaught():
    """Test that uncaught exceptions from resume() propagate to caller."""
    code = 'external_func()'
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    # Resume with an exception that won't be caught - wrapped in MontyRuntimeError
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume({'exception': ValueError('uncaught error')})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('uncaught error')


def test_resume_none():
    code = 'external_func()'
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    result = progress.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(None)


def test_invalid_resume_args():
    """`resume()` validates the result dict shape without consuming the snapshot."""
    code = 'external_func()'
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    # No positional `result` argument — pyo3 surfaces the missing-arg TypeError.
    with pytest.raises(TypeError) as exc_info:
        progress.resume()  # pyright: ignore[reportCallIssue]
    assert exc_info.value.args[0] == snapshot(
        "FunctionSnapshot.resume() missing 1 required positional argument: 'result'"
    )

    # Empty result dict — must have exactly one key.
    with pytest.raises(TypeError) as exc_info:
        progress.resume({})  # pyright: ignore[reportArgumentType]
    assert exc_info.value.args[0] == snapshot(
        "ExternalResult must be a dict with exactly one of 'return_value', 'exception', or 'future'"
    )

    # Multiple keys — must have exactly one.
    with pytest.raises(TypeError) as exc_info:
        progress.resume({'return_value': 42, 'exception': ValueError('error')})  # pyright: ignore[reportArgumentType]
    assert exc_info.value.args[0] == snapshot(
        "ExternalResult must be a dict with exactly one of 'return_value', 'exception', or 'future'"
    )

    # Wrong key — must be one of the recognized ones.
    with pytest.raises(TypeError) as exc_info:
        progress.resume({'bogus': 1})  # pyright: ignore[reportArgumentType]
    assert exc_info.value.args[0] == snapshot(
        "ExternalResult must be a dict with exactly one of 'return_value', 'exception', or 'future'"
    )

    # Unexpected kwarg — pyo3 surfaces the unexpected-kwarg TypeError.
    with pytest.raises(TypeError) as exc_info:
        progress.resume({'return_value': 1}, invalid_kwarg=42)  # pyright: ignore[reportCallIssue]
    assert exc_info.value.args[0] == snapshot(
        "FunctionSnapshot.resume() got an unexpected keyword argument 'invalid_kwarg'"
    )


def test_start_progress_resume_exception_in_nested_try():
    """Test exception handling in nested try/except blocks."""
    code = """
outer_caught = False
finally_ran = False
try:
    try:
        external_func()
    except TypeError:
        pass  # Won't catch ValueError
    finally:
        finally_ran = True
except ValueError:
    outer_caught = True
(outer_caught, finally_ran)
"""
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    result = progress.resume({'exception': ValueError('propagates to outer')})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot((True, True))


def test_name_lookup():
    m = pydantic_monty.Monty('x = foo; x')
    p = m.start()
    assert isinstance(p, pydantic_monty.NameLookupSnapshot)
    p2 = p.resume(value=42)
    assert isinstance(p2, pydantic_monty.MontyComplete)
    assert p2.output == 42


def test_ext_function_alt_name():
    """Test that a NameLookup can resolve to a function whose __name__ differs
    from the variable it was assigned to.  The VM should yield a FunctionCall
    with the *function's* name (not the variable name)."""
    m = pydantic_monty.Monty('x = foobar; x()')
    p = m.start()
    assert isinstance(p, pydantic_monty.NameLookupSnapshot)

    def not_foobar():
        return 42

    p2 = p.resume(value=not_foobar)
    # The function is called via HeapData::ExtFunction, yielding a FunctionSnapshot
    assert isinstance(p2, pydantic_monty.FunctionSnapshot)
    assert p2.function_name == snapshot('not_foobar')
    assert p2.args == snapshot(())
    assert p2.kwargs == snapshot({})

    result = p2.resume({'return_value': 42})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(42)


# === Tests for Monty.start(mount=..., os=...) ===


@pytest.fixture
def test_dir() -> Generator[Path, None, None]:
    """Creates a temporary directory with test files for mount-based tests."""
    with tempfile.TemporaryDirectory() as tmpdir:
        p = Path(tmpdir)
        (p / 'hello.txt').write_text('hello world')
        (p / 'subdir').mkdir()
        (p / 'subdir' / 'nested.txt').write_text('nested content')
        yield p


def assert_mount_reusable(md: MountDir) -> None:
    """Assert that a previously used mount was returned to its shared slot."""
    m = Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()")
    assert m.run(mount=md) == snapshot('nested content')


def test_start_with_mount_read_returns_complete(test_dir: Path):
    """start() with mount auto-dispatches OS calls and returns MontyComplete directly."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    m = Monty("from pathlib import Path; Path('/data/hello.txt').read_text()")
    result = m.start(mount=md)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot('hello world')


def test_start_with_os_callback_returns_complete():
    """start() with os= callback auto-dispatches OS calls to the callback."""
    calls: list[tuple[str, tuple[Any, ...]]] = []

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        calls.append((func, args))
        return True

    m = Monty("from pathlib import Path; Path('/any/path.txt').exists()")
    result = m.start(os=os_cb)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output is True
    assert calls == snapshot([('Path.exists', (PurePosixPath('/any/path.txt'),))])


def test_start_with_os_callback_not_handled():
    """NOT_HANDLED sentinel falls back to Monty's default unhandled behavior (PermissionError)."""

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> object:
        return NOT_HANDLED

    m = Monty("from pathlib import Path; Path('/any/path.txt').exists()")
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.start(os=os_cb)
    inner = exc_info.value.exception()
    assert isinstance(inner, PermissionError)


def test_start_mount_then_external_function(test_dir: Path):
    """OS calls are auto-dispatched, then start() returns the external FunctionSnapshot."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
content = Path('/data/hello.txt').read_text()
process(content)
"""
    m = Monty(code)
    result = m.start(mount=md)
    assert isinstance(result, pydantic_monty.FunctionSnapshot)
    assert result.is_os_function is False
    assert result.function_name == snapshot('process')
    assert result.args == snapshot(('hello world',))
    final = result.resume({'return_value': 'processed'})
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert final.output == snapshot('processed')


def test_start_mount_then_name_lookup(test_dir: Path):
    """OS calls are auto-dispatched, then start() returns NameLookupSnapshot for a bare name."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
x = Path('/data/hello.txt').read_text()
y = unknown_name
"""
    m = Monty(code)
    result = m.start(mount=md)
    assert isinstance(result, pydantic_monty.NameLookupSnapshot)
    assert result.variable_name == snapshot('unknown_name')


def test_start_os_callback_exception_caught():
    """Exceptions raised by the os= callback are propagated into the sandbox as Python exceptions."""

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> object:
        raise ValueError('not allowed')

    code = """
from pathlib import Path
try:
    Path('/etc/passwd').read_text()
    caught = False
except ValueError as exc:
    caught = (True, str(exc))
caught
"""
    m = Monty(code)
    result = m.start(os=os_cb)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot((True, 'not allowed'))


def test_start_mount_released_after_completion(test_dir: Path):
    """After start() returns, the mount is put back and reusable by another run."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    m1 = Monty("from pathlib import Path; Path('/data/hello.txt').read_text()")
    first = m1.start(mount=md)
    assert isinstance(first, pydantic_monty.MontyComplete)

    # The mount was put back — a second Monty instance can use the same mount.
    m2 = Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()")
    second = m2.run(mount=md)
    assert second == snapshot('nested content')


def test_start_mount_released_after_runtime_error(test_dir: Path):
    """start(mount=...) puts mounts back when auto-dispatch ends in a runtime error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
1 / 0
"""
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        Monty(code).start(mount=md)
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)
    assert_mount_reusable(md)


def test_start_mount_released_after_resource_error(test_dir: Path):
    """start(mount=...) puts mounts back when a resource limit trips after an OS call."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        Monty(code).start(mount=md, limits=pydantic_monty.ResourceLimits(max_memory=100))
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


def test_start_no_mount_os_still_yields_os_snapshot():
    """Control test: without mount/os, OS calls still surface as is_os_function snapshots."""
    m = Monty("from pathlib import Path; Path('/tmp/x.txt').exists()")
    result = m.start()
    assert isinstance(result, pydantic_monty.FunctionSnapshot)
    assert result.is_os_function is True
    assert result.function_name == snapshot('Path.exists')


def test_start_os_handler_invalid_rejected():
    """Non-callable os= is rejected before any VM work."""
    m = Monty("from pathlib import Path; Path('/tmp/x.txt').exists()")
    with pytest.raises(TypeError) as exc_info:
        m.start(os=123)  # pyright: ignore[reportArgumentType]
    assert str(exc_info.value) == snapshot("os must be callable, got 'int'")


def test_start_resume_after_auto_dispatch_yields_os_snapshot(test_dir: Path):
    """When resume() is called without mount/os, auto-dispatch stops.

    OS calls produced by a later resume() surface as a FunctionSnapshot with
    is_os_function=True — the mount context must be re-provided via resume().
    """
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
first = Path('/data/hello.txt').read_text()
mid = process(first)
second = Path('/data/subdir/nested.txt').read_text()
(first, mid, second)
"""
    m = Monty(code)
    # First OS call is auto-dispatched, returns at external `process`.
    p1 = m.start(mount=md)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('process')

    # Resume without mount=: second OS call surfaces as an OS snapshot.
    p2 = p1.resume({'return_value': 'processed'})
    assert isinstance(p2, pydantic_monty.FunctionSnapshot)
    assert p2.is_os_function is True
    assert p2.function_name == snapshot('Path.read_text')


def test_start_mount_ignored_when_progress_never_reaches_os_call(test_dir: Path):
    """start(mount=...) does not take the mount unless an OS call is actually reached."""
    md = MountDir('/data', str(test_dir), mode='read-only')

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        inner = Monty('1 + 1').start(mount=md)
        assert isinstance(inner, pydantic_monty.MontyComplete)
        assert inner.output == snapshot(2)
        return False

    outer = Monty("from pathlib import Path; Path('/outside').exists()")
    result = outer.start(mount=md, os=os_cb)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output is False


# === Tests for snapshot.resume(mount=..., os=...) ===


def test_function_snapshot_resume_with_mount_drives_os(test_dir: Path):
    """resume(return_value=..., mount=...) auto-dispatches OS calls and runs to completion."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
x = fetch()
content = Path('/data/hello.txt').read_text()
(x, content)
"""
    m = Monty(code)
    p1 = m.start(mount=md)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('fetch')

    # Pass mount= on resume — the OS call after the external function is now auto-dispatched.
    result = p1.resume({'return_value': 'fetched'}, mount=md)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(('fetched', 'hello world'))


def test_function_snapshot_resume_with_os_callback_drives_os():
    """resume(return_value=..., os=...) auto-dispatches OS calls via the callback."""

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        return True

    code = """
x = fetch()
from pathlib import Path
exists = Path('/tmp/some.txt').exists()
(x, exists)
"""
    m = Monty(code)
    p1 = m.start()  # no mount/os at start — fetch yields first
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('fetch')

    result = p1.resume({'return_value': 'fetched'}, os=os_cb)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(('fetched', True))


def test_function_snapshot_resume_with_mount_yields_next_external(test_dir: Path):
    """resume(..., mount=...) auto-dispatches OS calls then yields the next external function."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
a = first()
c = Path('/data/hello.txt').read_text()
b = second(c)
"""
    m = Monty(code)
    p1 = m.start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('first')

    # Pass mount=; OS call between first() and second() is auto-dispatched.
    p2 = p1.resume({'return_value': 1}, mount=md)
    assert isinstance(p2, pydantic_monty.FunctionSnapshot)
    assert p2.is_os_function is False
    assert p2.function_name == snapshot('second')
    assert p2.args == snapshot(('hello world',))


def test_function_snapshot_resume_exception_with_mount(test_dir: Path):
    """resume(exception=..., mount=...) works when the exception is caught and execution continues with OS calls."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
try:
    result = fetch()
except ValueError:
    result = Path('/data/hello.txt').read_text()
result
"""
    m = Monty(code)
    p1 = m.start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('fetch')

    final = p1.resume({'exception': ValueError('boom')}, mount=md)
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert final.output == snapshot('hello world')


def test_name_lookup_snapshot_resume_with_mount(test_dir: Path):
    """NameLookupSnapshot.resume(value=..., mount=...) auto-dispatches subsequent OS calls."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
v = my_name
content = Path('/data/hello.txt').read_text()
(v, content)
"""
    m = Monty(code)
    p1 = m.start()
    assert isinstance(p1, pydantic_monty.NameLookupSnapshot)
    assert p1.variable_name == snapshot('my_name')

    result = p1.resume(value=42, mount=md)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot((42, 'hello world'))


def test_name_lookup_resume_mount_released_after_runtime_error(test_dir: Path):
    """NameLookupSnapshot.resume(..., mount=...) puts mounts back on runtime error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
value = missing_name
Path('/data/hello.txt').read_text()
1 / 0
"""
    progress = Monty(code).start()
    assert isinstance(progress, pydantic_monty.NameLookupSnapshot)

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume(value=42, mount=md)
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)
    assert_mount_reusable(md)


def test_resume_not_handled_with_mount(test_dir: Path):
    """resume_not_handled(mount=...) auto-dispatches subsequent OS calls.

    The initial OS call is marked unhandled (raises PermissionError), but the
    sandbox catches it and subsequent OS calls (handled via mount) auto-dispatch.
    """
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
try:
    Path('/outside').exists()
    raised = False
except PermissionError:
    raised = True
content = Path('/data/hello.txt').read_text()
(raised, content)
"""
    m = Monty(code)
    p1 = m.start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.is_os_function is True
    assert p1.function_name == snapshot('Path.exists')

    result = p1.resume_not_handled(mount=md)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot((True, 'hello world'))


def test_resume_mount_then_next_os_snapshot_without_mount(test_dir: Path):
    """Auto-dispatch only covers the single resume() call — next resume without mount yields OS snapshot."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
x = first()
a = Path('/data/hello.txt').read_text()
y = middle()
b = Path('/data/subdir/nested.txt').read_text()
(x, a, y, b)
"""
    m = Monty(code)
    p1 = m.start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('first')

    # Resume with mount: first OS call auto-dispatched, yields at middle().
    p2 = p1.resume({'return_value': 1}, mount=md)
    assert isinstance(p2, pydantic_monty.FunctionSnapshot)
    assert p2.function_name == snapshot('middle')

    # Resume without mount: second OS call surfaces as an OS snapshot.
    p3 = p2.resume({'return_value': 2})
    assert isinstance(p3, pydantic_monty.FunctionSnapshot)
    assert p3.is_os_function is True
    assert p3.function_name == snapshot('Path.read_text')


def test_resume_mount_put_back_after_auto_dispatch(test_dir: Path):
    """After resume() with mount/os completes, the mount is put back."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
x = fetch()
from pathlib import Path
content = Path('/data/hello.txt').read_text()
(x, content)
"""
    m = Monty(code)
    p1 = m.start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    final = p1.resume({'return_value': 'x'}, mount=md)
    assert isinstance(final, pydantic_monty.MontyComplete)

    # Reusing the mount should still work after resume finishes.
    m2 = Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()")
    assert m2.run(mount=md) == snapshot('nested content')


def test_resume_mount_released_after_runtime_error(test_dir: Path):
    """FunctionSnapshot.resume(..., mount=...) puts mounts back on runtime error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
fetch()
Path('/data/hello.txt').read_text()
1 / 0
"""
    progress = Monty(code).start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume({'return_value': 'fetched'}, mount=md)
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)
    assert_mount_reusable(md)


def test_resume_mount_released_after_resource_error(test_dir: Path):
    """FunctionSnapshot.resume(..., mount=...) puts mounts back on resource error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
fetch()
Path('/data/hello.txt').read_text()
result = []
for i in range(10000):
    result.append([i])
len(result)
"""
    progress = Monty(code).start(limits=pydantic_monty.ResourceLimits(max_allocations=100))
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume({'return_value': 'fetched'}, mount=md)
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


def test_future_snapshot_resume_with_mount(test_dir: Path):
    """FutureSnapshot.resume(..., mount=...) auto-dispatches OS calls and completes."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
value = await fetch()
from pathlib import Path
content = Path('/data/hello.txt').read_text()
(value, content)
"""
    progress = Monty(code).start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    call_id = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    final = progress.resume({call_id: {'return_value': 'fetched'}}, mount=md)
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert final.output == snapshot(('fetched', 'hello world'))


def test_future_snapshot_resume_mount_released_after_resource_error(test_dir: Path):
    """FutureSnapshot.resume(..., mount=...) puts mounts back on resource error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
value = await fetch()
from pathlib import Path
Path('/data/hello.txt').read_text()
result = []
for i in range(10000):
    result.append([i])
len(result)
"""
    progress = Monty(code).start(limits=pydantic_monty.ResourceLimits(max_allocations=100))
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    call_id = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume({call_id: {'return_value': 'fetched'}}, mount=md)
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


def test_resume_mount_ignored_when_progress_never_reaches_os_call(test_dir: Path):
    """resume(mount=...) does not take the mount unless the resumed code hits an OS call."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    pending = Monty('fetch()').start()
    assert isinstance(pending, pydantic_monty.FunctionSnapshot)

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        inner = pending.resume({'return_value': 1}, mount=md)
        assert isinstance(inner, pydantic_monty.MontyComplete)
        assert inner.output == snapshot(1)
        return False

    outer = Monty("from pathlib import Path; Path('/outside').exists()")
    result = outer.start(mount=md, os=os_cb)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output is False


def test_resume_invalid_os_rejected():
    """Non-callable `os=` on resume is rejected with TypeError."""
    m = Monty('fetch()')
    p1 = m.start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    with pytest.raises(TypeError) as exc_info:
        p1.resume({'return_value': 1}, os=123)  # pyright: ignore[reportArgumentType]
    assert str(exc_info.value) == snapshot("os must be callable, got 'int'")


# === Tests that snapshot is preserved on resume() validation errors ===


def test_function_snapshot_preserved_on_invalid_result():
    """`resume()` with a bad result dict leaves the snapshot intact for retry.

    A typo in the result key, an empty dict, or multiple keys used to
    consume the snapshot via `mem::replace(..., Done)` before validating, so
    the user lost their snapshot to a one-character typo.
    """
    p1 = Monty('fetch()').start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)

    # Wrong key — TypeError, snapshot must NOT be consumed.
    with pytest.raises(TypeError):
        p1.resume({'retrun_value': 1})  # pyright: ignore[reportArgumentType]
    # Multiple keys — TypeError, snapshot must NOT be consumed.
    with pytest.raises(TypeError):
        p1.resume({'return_value': 1, 'exception': ValueError('x')})  # pyright: ignore[reportArgumentType]
    # Missing positional — TypeError, snapshot must NOT be consumed.
    with pytest.raises(TypeError):
        p1.resume()  # pyright: ignore[reportCallIssue]

    # Snapshot is still usable.
    final = p1.resume({'return_value': 42})
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert final.output == snapshot(42)


def test_function_snapshot_preserved_on_invalid_mount():
    """Validation of `mount`/`os` happens before the snapshot is consumed."""
    p1 = Monty('fetch()').start()
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    with pytest.raises(TypeError):
        p1.resume({'return_value': 1}, os=123)  # pyright: ignore[reportArgumentType]
    final = p1.resume({'return_value': 42})
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert final.output == snapshot(42)


def test_name_lookup_snapshot_preserved_on_invalid_value():
    """`NameLookupSnapshot.resume()` with an unconvertible `value` preserves the snapshot."""

    class Custom:
        pass

    p1 = Monty('x = my_name; x').start()
    assert isinstance(p1, pydantic_monty.NameLookupSnapshot)

    # Custom non-convertible object → TypeError from py_to_monty, snapshot intact.
    with pytest.raises(TypeError):
        p1.resume(value=Custom())

    final = p1.resume(value=42)
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert final.output == snapshot(42)


def test_future_snapshot_preserved_on_invalid_results():
    """`FutureSnapshot.resume()` with a malformed `results` dict preserves the snapshot."""
    progress = Monty('x = await fetch(); x').start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    call_id = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)

    # Bad call_id type (string instead of int) → TypeError, snapshot intact.
    with pytest.raises(TypeError):
        progress.resume({'not-an-int': {'return_value': 1}})  # pyright: ignore[reportArgumentType]
    # Inner dict has both keys → TypeError, snapshot intact.
    with pytest.raises(TypeError):
        progress.resume({call_id: {'return_value': 1, 'exception': ValueError('x')}})  # pyright: ignore[reportArgumentType]

    final = progress.resume({call_id: {'return_value': 42}})
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert final.output == snapshot(42)
