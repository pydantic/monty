from typing import Callable, Literal

import pytest
from inline_snapshot import snapshot

import pydantic_monty

PrintCallback = Callable[[Literal['stdout', 'stderr'], str], None]


def make_print_collector() -> tuple[list[str], PrintCallback]:
    """Create a print callback that collects output into a list."""
    output: list[str] = []

    def callback(stream: Literal['stdout', 'stderr'], text: str) -> None:
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
    assert result.output is None
    assert result.print_output is None  # callback mode doesn't populate this


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

    def callback(stream: Literal['stdout', 'stderr'], text: str) -> None:
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

    def callback(stream: Literal['stdout', 'stderr'], text: str) -> None:
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


# ---------------------------------------------------------------------------
# `print_callback='collect'` mode
# ---------------------------------------------------------------------------


def test_collect_basic() -> None:
    """Contiguous same-stream output merges into a single tuple.

    Stream changes are where new tuples get pushed — today all output goes to
    stdout so the list usually contains exactly one entry per run.
    """
    m = pydantic_monty.Monty('print("a"); print("b", 1)')
    result = m.run(print_callback='collect')
    assert result.output is None
    assert result.print_output == snapshot([('stdout', 'a\nb 1\n')])


def test_collect_empty_when_no_prints() -> None:
    m = pydantic_monty.Monty('1 + 1')
    result = m.run(print_callback='collect')
    assert result.output == snapshot(2)
    assert result.print_output == snapshot([])


def test_collect_none_when_not_enabled() -> None:
    """Without `print_callback='collect'`, `print_output` is `None`."""
    m = pydantic_monty.Monty('1 + 1')
    result = m.run()
    assert result.output == 2
    assert result.print_output is None


def test_collect_invalid_string_raises() -> None:
    m = pydantic_monty.Monty('1')
    with pytest.raises(TypeError) as exc_info:
        m.run(print_callback='bogus')  # type: ignore[arg-type]
    assert exc_info.value.args[0] == snapshot('print_callback string must be \'collect\', got "bogus"')


def test_collect_across_start_resume() -> None:
    """Collect buffer accumulates across `start` / `resume` snapshots."""
    code = """
print("before")
x = magic
print("after", x)
x + 1
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect')
    # `magic` is undefined so we get a NameLookupSnapshot.
    assert isinstance(progress, pydantic_monty.NameLookupSnapshot)
    # Live view on the snapshot shows the pre-lookup prints.
    assert progress.print_output == snapshot([('stdout', 'before\n')])
    complete = progress.resume(value=10)
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output == 11
    assert complete.print_output == snapshot([('stdout', 'before\nafter 10\n')])


def test_collect_on_runtime_error() -> None:
    """`MontyRuntimeError.print_output` carries what was printed before the error."""
    code = """
print("about to fail")
1 / 0
"""
    m = pydantic_monty.Monty(code)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback='collect')
    assert exc_info.value.print_output == snapshot([('stdout', 'about to fail\n')])


def test_collect_run_async() -> None:
    import asyncio

    async def go() -> pydantic_monty.MontyComplete:
        m = pydantic_monty.Monty('print("async"); 7')
        return await m.run_async(print_callback='collect')

    result = asyncio.run(go())
    assert result.output == 7
    assert result.print_output == snapshot([('stdout', 'async\n')])


def test_collect_repl_feed_run() -> None:
    repl = pydantic_monty.MontyRepl()
    r1 = repl.feed_run('print("one"); x = 1', print_callback='collect')
    assert r1.output is None
    assert r1.print_output == snapshot([('stdout', 'one\n')])
    # Each feed_run gets its own buffer.
    r2 = repl.feed_run('print("two"); x + 1', print_callback='collect')
    assert r2.output == 2
    assert r2.print_output == snapshot([('stdout', 'two\n')])
