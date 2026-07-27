"""print_callback tests: line-buffered chunk delivery, error propagation, collectors."""

from __future__ import annotations

from typing import Callable, Literal

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import CollectStreams, CollectString, Monty, MontyRuntimeError, MontySession

PrintCallback = Callable[[Literal['stdout', 'stderr'], str], None]


def make_print_collector() -> tuple[list[str], PrintCallback]:
    """Create a print callback that collects output into a list.

    The callback receives line-buffered chunks (one call per completed line,
    or 8KiB), so assertions join the chunks rather than checking fragments.
    """
    output: list[str] = []

    def callback(stream: Literal['stdout', 'stderr'], text: str) -> None:
        assert stream == 'stdout'
        output.append(text)

    return output, callback


def test_print_basic(monty_run: RunMonty) -> None:
    output, callback = make_print_collector()
    monty_run('print("hello")', print_callback=callback)
    assert ''.join(output) == snapshot('hello\n')


def test_print_multiple(monty_run: RunMonty) -> None:
    code = """
print("line 1")
print("line 2")
"""
    output, callback = make_print_collector()
    monty_run(code, print_callback=callback)
    assert ''.join(output) == snapshot('line 1\nline 2\n')


def test_print_with_values(monty_run: RunMonty) -> None:
    output, callback = make_print_collector()
    monty_run('print(1, 2, 3)', print_callback=callback)
    assert ''.join(output) == snapshot('1 2 3\n')


def test_print_with_sep(monty_run: RunMonty) -> None:
    output, callback = make_print_collector()
    monty_run('print(1, 2, 3, sep="-")', print_callback=callback)
    assert ''.join(output) == snapshot('1-2-3\n')


def test_print_with_end(monty_run: RunMonty) -> None:
    output, callback = make_print_collector()
    monty_run('print("hello", end="!")', print_callback=callback)
    assert ''.join(output) == snapshot('hello!')


def test_print_returns_none(monty_run: RunMonty) -> None:
    _, callback = make_print_collector()
    assert monty_run('print("test")', print_callback=callback) is None


def test_print_empty(monty_run: RunMonty) -> None:
    output, callback = make_print_collector()
    monty_run('print()', print_callback=callback)
    assert ''.join(output) == snapshot('\n')


def test_print_with_limits(monty_run: RunMonty) -> None:
    """Verify print_callback works together with resource limits."""
    output, callback = make_print_collector()
    monty_run('print("with limits")', print_callback=callback, limits={'max_duration_secs': 5.0})
    assert ''.join(output) == snapshot('with limits\n')


def test_print_with_inputs(monty_run: RunMonty) -> None:
    """Verify print_callback works together with inputs."""
    output, callback = make_print_collector()
    monty_run('print(x)', inputs={'x': 42}, print_callback=callback)
    assert ''.join(output) == snapshot('42\n')


def test_print_in_loop(monty_run: RunMonty) -> None:
    code = """
for i in range(3):
    print(i)
"""
    output, callback = make_print_collector()
    monty_run(code, print_callback=callback)
    assert ''.join(output) == snapshot('0\n1\n2\n')


def test_print_mixed_types(monty_run: RunMonty) -> None:
    output, callback = make_print_collector()
    monty_run('print(1, "hello", True, None)', print_callback=callback)
    assert ''.join(output) == snapshot('1 hello True None\n')


def make_error_callback(error: Exception) -> PrintCallback:
    """Create a print callback that raises an exception."""

    def callback(stream: Literal['stdout', 'stderr'], text: str) -> None:
        raise error

    return callback


def test_print_callback_raises_value_error(monty_run: RunMonty) -> None:
    """Test that ValueError raised in callback propagates correctly."""
    callback = make_error_callback(ValueError('callback error'))
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('print("hello")', print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('callback error')


def test_print_callback_raises_type_error(monty_run: RunMonty) -> None:
    """Test that TypeError raised in callback propagates correctly."""
    callback = make_error_callback(TypeError('wrong type'))
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('print("hello")', print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)
    assert inner.args[0] == snapshot('wrong type')


def test_print_callback_raises_in_function(monty_run: RunMonty) -> None:
    """Test exception from callback when print is called inside a function."""
    code = """
def greet(name):
    print(f"Hello, {name}!")

greet("World")
"""
    callback = make_error_callback(RuntimeError('io error'))
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert inner.args[0] == snapshot('io error')


def test_print_callback_raises_in_nested_function(monty_run: RunMonty) -> None:
    """Test exception from callback when print is called in nested functions."""
    code = """
def outer():
    def inner():
        print("from inner")
    inner()

outer()
"""
    callback = make_error_callback(ValueError('nested error'))
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('nested error')


def test_print_callback_failure_during_suspension_ends_session(session: MontySession) -> None:
    """A print-callback failure on a turn that then suspends (here on an
    external function call) must not wedge the session.

    The feed is aborted with the callback's error, and because the worker was
    left suspended — waiting for a resume the aborted feed will never send —
    the session is discarded so the next feed fails cleanly, rather than with
    a confusing "suspension awaiting an answer" protocol error.
    """
    code = """
print("before call")
fetch()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run(
            code,
            external_lookup={'fetch': lambda: 42},
            print_callback=make_error_callback(ValueError('callback boom')),
        )
    assert exc_info.value.exception().args[0] == snapshot('callback boom')

    # the session is cleanly ended, not wedged on the abandoned suspension
    with pytest.raises(RuntimeError) as exc_info2:
        session.feed_run('1 + 1')
    assert str(exc_info2.value) == snapshot('this checkout has already been finished')


def test_print_callback_raises_in_loop(monty_run: RunMonty) -> None:
    """Test exception from callback when print is called in a loop.

    Chunks are line-buffered, so each `print(i)` delivers exactly one chunk.
    """
    code = """
for i in range(5):
    print(i)
"""
    call_count = 0

    def callback(stream: Literal['stdout', 'stderr'], text: str) -> None:
        nonlocal call_count
        call_count += 1
        if call_count >= 3:
            raise ValueError('stopped at 3')

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('stopped at 3')
    assert call_count == snapshot(3)


def test_map_print(monty_run: RunMonty) -> None:
    """Test that print can be used inside map."""
    code = """
list(map(print, [1, 2, 3]))
"""
    output, callback = make_print_collector()
    monty_run(code, print_callback=callback)
    assert ''.join(output) == snapshot('1\n2\n3\n')


# === CollectStreams / CollectString ===


def test_collect_streams_run_returns_raw_output(monty_run: RunMonty) -> None:
    collector = CollectStreams()

    result = monty_run('print("a"); print("b", 1); 123', print_callback=collector)

    assert result == snapshot(123)
    assert collector.output == snapshot([('stdout', 'a\n'), ('stdout', 'b 1\n')])


def test_collect_streams_repr(monty_run: RunMonty) -> None:
    collector = CollectStreams()

    assert collector.output == snapshot([])
    assert repr(collector) == snapshot('CollectStreams(output=[])')

    monty_run('print("hello")', print_callback=collector)

    assert collector.output == snapshot([('stdout', 'hello\n')])
    assert repr(collector) == snapshot("CollectStreams(output=[('stdout', 'hello\\n')])")


def test_collect_string_run_returns_raw_output(monty_run: RunMonty) -> None:
    collector = CollectString()

    result = monty_run('print("a"); print("b", 1); 123', print_callback=collector)

    assert result == snapshot(123)
    assert collector.output == snapshot('a\nb 1\n')


def test_collect_string_repr(monty_run: RunMonty) -> None:
    collector = CollectString()

    assert collector.output == snapshot('')
    assert repr(collector) == snapshot("CollectString(output='')")

    monty_run('print("hello")', print_callback=collector)

    assert collector.output == snapshot('hello\n')
    assert repr(collector) == snapshot("CollectString(output='hello\\n')")


def test_collect_string_reuse_across_runs_accumulates(monty_run: RunMonty) -> None:
    collector = CollectString()

    assert monty_run('print("one")', print_callback=collector) is None
    assert monty_run('print("two")', print_callback=collector) is None

    assert collector.output == snapshot('one\ntwo\n')


def test_collect_streams_accumulates_across_external_call(monty_run: RunMonty) -> None:
    code = """
print("before")
x = fetch()
print("after", x)
"""
    collector = CollectStreams()
    result = monty_run(code, external_lookup={'fetch': lambda: 10}, print_callback=collector)

    assert result is None
    assert collector.output == snapshot([('stdout', 'before\n'), ('stdout', 'after 10\n')])


def test_collect_string_accumulates_across_external_call(monty_run: RunMonty) -> None:
    code = """
print("before")
x = fetch()
print("after", x)
"""
    collector = CollectString()
    result = monty_run(code, external_lookup={'fetch': lambda: 10}, print_callback=collector)

    assert result is None
    assert collector.output == snapshot('before\nafter 10\n')


def test_collect_streams_error_stays_on_collector(monty_run: RunMonty) -> None:
    collector = CollectStreams()

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('print("about to fail"); raise ValueError("boom")', print_callback=collector)

    assert collector.output == snapshot([('stdout', 'about to fail\n')])
    assert not hasattr(exc_info.value, 'print_output')


def test_collect_string_error_stays_on_collector(monty_run: RunMonty) -> None:
    collector = CollectString()

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('print("about to fail"); raise ValueError("boom")', print_callback=collector)

    assert collector.output == snapshot('about to fail\n')
    assert not hasattr(exc_info.value, 'print_output')


def test_collect_string_accumulates_across_feeds(pool: Monty) -> None:
    """One collector passed to multiple feeds of the same session accumulates."""
    collector = CollectString()
    with pool.checkout() as session:
        session.feed_run('print("first")', print_callback=collector)
        session.feed_run('print("second")', print_callback=collector)
    assert collector.output == snapshot('first\nsecond\n')


def test_collectors_are_valid_print_callback_values(monty_run: RunMonty) -> None:
    with pytest.raises(TypeError) as exc_info:
        monty_run('None', print_callback='collect-string')  # pyright: ignore[reportArgumentType]
    assert str(exc_info.value) == snapshot(
        'print_callback must be a callable, CollectStreams(), CollectString(), or None'
    )


def test_collect_string_max_bytes_raises(monty_run: RunMonty) -> None:
    """Host CollectString cap is independent of ResourceLimits.max_memory (issue #464)."""
    collector = CollectString(max_bytes=100)
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("print('x' * 200)", print_callback=collector)
    inner = exc_info.value.exception()
    assert isinstance(inner, MemoryError)
    assert str(inner) == snapshot('memory limit exceeded: 201 bytes > 100 bytes')
    assert collector.output == snapshot('')


def test_collect_streams_max_bytes_raises(monty_run: RunMonty) -> None:
    """Host CollectStreams cap charges payload + per-entry overhead (issue #464)."""
    collector = CollectStreams(max_bytes=100)
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("print('x' * 200)", print_callback=collector)
    inner = exc_info.value.exception()
    assert isinstance(inner, MemoryError)
    # 201 payload bytes + 64 entry overhead
    assert str(inner) == snapshot('memory limit exceeded: 265 bytes > 100 bytes')
    assert collector.output == snapshot([])
