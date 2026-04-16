import asyncio
from typing import Callable, Literal

import pytest
from inline_snapshot import snapshot

import pydantic_monty

PrintCallback = Callable[[Literal['stdout'], str], None]


def make_print_collector() -> tuple[list[str], PrintCallback]:
    """Create a print callback that collects output into a list."""
    output: list[str] = []

    def callback(stream: Literal['stdout'], text: str) -> None:
        assert stream == 'stdout'
        output.append(text)

    return output, callback


def test_print_basic() -> None:
    m = pydantic_monty.Monty('print("hello")')
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('hello\n')


def test_print_multiple() -> None:
    code = """
print("line 1")
print("line 2")
"""
    m = pydantic_monty.Monty(code)
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('line 1\nline 2\n')


def test_print_with_values() -> None:
    m = pydantic_monty.Monty('print(1, 2, 3)')
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('1 2 3\n')


def test_print_with_sep() -> None:
    m = pydantic_monty.Monty('print(1, 2, 3, sep="-")')
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('1-2-3\n')


def test_print_with_end() -> None:
    m = pydantic_monty.Monty('print("hello", end="!")')
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('hello!')


def test_print_returns_none() -> None:
    m = pydantic_monty.Monty('print("test")')
    _, callback = make_print_collector()
    result = m.run(print_callback=callback)
    assert result is None


def test_print_empty() -> None:
    m = pydantic_monty.Monty('print()')
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('\n')


def test_print_with_limits() -> None:
    """Verify print_callback works together with resource limits."""
    m = pydantic_monty.Monty('print("with limits")')
    output, callback = make_print_collector()
    limits = pydantic_monty.ResourceLimits(max_duration_secs=5.0)
    m.run(print_callback=callback, limits=limits)
    assert ''.join(output) == snapshot('with limits\n')


def test_print_with_inputs() -> None:
    """Verify print_callback works together with inputs."""
    m = pydantic_monty.Monty('print(x)', inputs=['x'])
    output, callback = make_print_collector()
    m.run(inputs={'x': 42}, print_callback=callback)
    assert ''.join(output) == snapshot('42\n')


def test_print_in_loop() -> None:
    code = """
for i in range(3):
    print(i)
"""
    m = pydantic_monty.Monty(code)
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('0\n1\n2\n')


def test_print_mixed_types() -> None:
    m = pydantic_monty.Monty('print(1, "hello", True, None)')
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('1 hello True None\n')


def make_error_callback(error: Exception) -> PrintCallback:
    """Create a print callback that raises an exception."""

    def callback(stream: Literal['stdout'], text: str) -> None:
        raise error

    return callback


def test_print_callback_raises_value_error() -> None:
    """Test that ValueError raised in callback propagates correctly."""
    m = pydantic_monty.Monty('print("hello")')
    callback = make_error_callback(ValueError('callback error'))
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('callback error')


def test_print_callback_raises_type_error() -> None:
    """Test that TypeError raised in callback propagates correctly."""
    m = pydantic_monty.Monty('print("hello")')
    callback = make_error_callback(TypeError('wrong type'))
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)
    assert inner.args[0] == snapshot('wrong type')


def test_print_callback_raises_in_function() -> None:
    """Test exception from callback when print is called inside a function."""
    code = """
def greet(name):
    print(f"Hello, {name}!")

greet("World")
"""
    m = pydantic_monty.Monty(code)
    callback = make_error_callback(RuntimeError('io error'))
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert inner.args[0] == snapshot('io error')


def test_print_callback_raises_in_nested_function() -> None:
    """Test exception from callback when print is called in nested functions."""
    code = """
def outer():
    def inner():
        print("from inner")
    inner()

outer()
"""
    m = pydantic_monty.Monty(code)
    callback = make_error_callback(ValueError('nested error'))
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('nested error')


def test_print_callback_raises_in_loop() -> None:
    """Test exception from callback when print is called in a loop."""
    code = """
for i in range(5):
    print(i)
"""
    m = pydantic_monty.Monty(code)
    call_count = 0

    def callback(stream: Literal['stdout'], text: str) -> None:
        nonlocal call_count
        call_count += 1
        if call_count >= 3:
            raise ValueError('stopped at 3')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback=callback)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('stopped at 3')
    assert call_count == snapshot(3)


def test_map_print() -> None:
    """Test that print can be used inside map."""
    code = """
list(map(print, [1, 2, 3]))
"""
    m = pydantic_monty.Monty(code)
    output, callback = make_print_collector()
    m.run(print_callback=callback)
    assert ''.join(output) == snapshot('1\n2\n3\n')


def test_collect_streams_run_returns_raw_output() -> None:
    m = pydantic_monty.Monty('print("a"); print("b", 1); 123')
    collector = pydantic_monty.CollectStreams()

    result = m.run(print_callback=collector)

    assert result == snapshot(123)
    assert collector.output == snapshot([('stdout', 'a\nb 1\n')])


def test_collect_streams_repr() -> None:
    collector = pydantic_monty.CollectStreams()

    assert collector.output == snapshot([])
    assert repr(collector) == snapshot('CollectStreams(output=[])')

    pydantic_monty.Monty('print("hello")').run(print_callback=collector)

    assert collector.output == snapshot([('stdout', 'hello\n')])
    assert repr(collector) == snapshot("CollectStreams(output=[('stdout', 'hello\\n')])")


def test_collect_string_run_returns_raw_output() -> None:
    m = pydantic_monty.Monty('print("a"); print("b", 1); 123')
    collector = pydantic_monty.CollectString()

    result = m.run(print_callback=collector)

    assert result == snapshot(123)
    assert collector.output == snapshot('a\nb 1\n')


def test_collect_string_repr() -> None:
    collector = pydantic_monty.CollectString()

    assert collector.output == snapshot('')
    assert repr(collector) == snapshot("CollectString(output='')")

    pydantic_monty.Monty('print("hello")').run(print_callback=collector)

    assert collector.output == snapshot('hello\n')
    assert repr(collector) == snapshot("CollectString(output='hello\\n')")


def test_collect_string_reuse_across_runs_accumulates() -> None:
    collector = pydantic_monty.CollectString()

    assert pydantic_monty.Monty('print("one")').run(print_callback=collector) is None
    assert pydantic_monty.Monty('print("two")').run(print_callback=collector) is None

    assert collector.output == snapshot('one\ntwo\n')


def test_collect_streams_start_resume_uses_collector_only() -> None:
    code = """
print("before")
x = fetch()
print("after", x)
"""
    m = pydantic_monty.Monty(code)
    collector = pydantic_monty.CollectStreams()

    progress = m.start(print_callback=collector)

    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert not hasattr(progress, 'print_output')
    assert collector.output == snapshot([('stdout', 'before\n')])

    complete = progress.resume({'return_value': 5})

    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert not hasattr(complete, 'print_output')
    assert complete.output is None
    assert collector.output == snapshot([('stdout', 'before\nafter 5\n')])


def test_collect_streams_error_stays_on_collector() -> None:
    m = pydantic_monty.Monty('print("about to fail"); raise ValueError("boom")')
    collector = pydantic_monty.CollectStreams()

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback=collector)

    assert collector.output == snapshot([('stdout', 'about to fail\n')])
    assert not hasattr(exc_info.value, 'print_output')


def test_collect_string_error_stays_on_collector() -> None:
    m = pydantic_monty.Monty('print("about to fail"); raise ValueError("boom")')
    collector = pydantic_monty.CollectString()

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback=collector)

    assert collector.output == snapshot('about to fail\n')
    assert not hasattr(exc_info.value, 'print_output')


def test_collect_streams_run_async_accumulates_across_external_call() -> None:
    code = """
print("before")
x = await fetch()
print("after", x)
"""
    m = pydantic_monty.Monty(code)
    collector = pydantic_monty.CollectStreams()

    async def fetch() -> int:
        return 10

    async def go() -> object:
        return await m.run_async(
            external_functions={'fetch': fetch},
            print_callback=collector,
        )

    result = asyncio.run(go())

    assert result is None
    assert collector.output == snapshot([('stdout', 'before\nafter 10\n')])


def test_collect_string_run_async_accumulates_across_external_call() -> None:
    code = """
print("before")
x = await fetch()
print("after", x)
"""
    m = pydantic_monty.Monty(code)
    collector = pydantic_monty.CollectString()

    async def fetch() -> int:
        return 10

    async def go() -> object:
        return await m.run_async(
            external_functions={'fetch': fetch},
            print_callback=collector,
        )

    result = asyncio.run(go())

    assert result is None
    assert collector.output == snapshot('before\nafter 10\n')


def test_load_snapshot_uses_fresh_collect_string() -> None:
    code = """
print("before")
x = fetch()
print("after", x)
"""
    m = pydantic_monty.Monty(code)
    first_collector = pydantic_monty.CollectString()
    progress = m.start(print_callback=first_collector)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert first_collector.output == snapshot('before\n')

    data = progress.dump()
    loaded_collector = pydantic_monty.CollectString()
    loaded = pydantic_monty.load_snapshot(data, print_callback=loaded_collector)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    complete = loaded.resume({'return_value': 10})

    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output is None
    assert first_collector.output == snapshot('before\n')
    assert loaded_collector.output == snapshot('after 10\n')


def test_load_snapshot_uses_fresh_collect_streams() -> None:
    code = """
print("before")
x = fetch()
print("after", x)
"""
    m = pydantic_monty.Monty(code)
    first_collector = pydantic_monty.CollectStreams()
    progress = m.start(print_callback=first_collector)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert first_collector.output == snapshot([('stdout', 'before\n')])

    data = progress.dump()
    loaded_collector = pydantic_monty.CollectStreams()
    loaded = pydantic_monty.load_snapshot(data, print_callback=loaded_collector)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    complete = loaded.resume({'return_value': 10})

    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output is None
    assert first_collector.output == snapshot([('stdout', 'before\n')])
    assert loaded_collector.output == snapshot([('stdout', 'after 10\n')])


def test_collectors_are_valid_print_callback_values() -> None:
    m = pydantic_monty.Monty('None')
    with pytest.raises(TypeError) as exc_info:
        m.run(print_callback='collect-string')  # type: ignore[arg-type]
    assert str(exc_info.value) == snapshot(
        'print_callback must be a callable, CollectStreams(), CollectString(), or None'
    )
