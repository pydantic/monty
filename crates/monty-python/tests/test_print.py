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
# `print_callback='collect-streams'` mode
# ---------------------------------------------------------------------------


def test_collect_streams_basic() -> None:
    """Contiguous same-stream output merges into a single tuple.

    Stream changes are where new tuples get pushed — today all output goes to
    stdout so the list usually contains exactly one entry per run.
    """
    m = pydantic_monty.Monty('print("a"); print("b", 1)')
    result = m.run(print_callback='collect-streams')
    assert result.output is None
    assert result.print_output == snapshot([('stdout', 'a\nb 1\n')])


def test_collect_streams_empty_when_no_prints() -> None:
    m = pydantic_monty.Monty('1 + 1')
    result = m.run(print_callback='collect-streams')
    assert result.output == snapshot(2)
    assert result.print_output == snapshot([])


def test_collect_none_when_not_enabled() -> None:
    """Without a collect mode set, `print_output` is `None`."""
    m = pydantic_monty.Monty('1 + 1')
    result = m.run()
    assert result.output == 2
    assert result.print_output is None


def test_collect_invalid_string_raises() -> None:
    m = pydantic_monty.Monty('1')
    with pytest.raises(TypeError) as exc_info:
        m.run(print_callback='bogus')  # type: ignore[arg-type]
    assert exc_info.value.args[0] == snapshot(
        "print_callback string must be 'collect-streams' or 'collect-string', got \"bogus\""
    )


def test_collect_streams_across_start_resume() -> None:
    """Collect buffer accumulates across `start` / `resume` snapshots."""
    code = """
print("before")
x = magic
print("after", x)
x + 1
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-streams')
    # `magic` is undefined so we get a NameLookupSnapshot.
    assert isinstance(progress, pydantic_monty.NameLookupSnapshot)
    # Live view on the snapshot shows the pre-lookup prints.
    assert progress.print_output == snapshot([('stdout', 'before\n')])
    complete = progress.resume(value=10)
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output == 11
    assert complete.print_output == snapshot([('stdout', 'before\nafter 10\n')])


def test_collect_streams_on_runtime_error() -> None:
    """`MontyRuntimeError.print_output` carries what was printed before the error."""
    code = """
print("about to fail")
1 / 0
"""
    m = pydantic_monty.Monty(code)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback='collect-streams')
    assert exc_info.value.print_output == snapshot([('stdout', 'about to fail\n')])


def test_collect_streams_run_async() -> None:
    import asyncio

    async def go() -> pydantic_monty.MontyComplete:
        m = pydantic_monty.Monty('print("async"); 7')
        return await m.run_async(print_callback='collect-streams')

    result = asyncio.run(go())
    assert result.output == 7
    assert result.print_output == snapshot([('stdout', 'async\n')])


def test_collect_streams_repl_feed_run() -> None:
    repl = pydantic_monty.MontyRepl()
    r1 = repl.feed_run('print("one"); x = 1', print_callback='collect-streams')
    assert r1.output is None
    assert r1.print_output == snapshot([('stdout', 'one\n')])
    # Each feed_run gets its own buffer.
    r2 = repl.feed_run('print("two"); x + 1', print_callback='collect-streams')
    assert r2.output == 2
    assert r2.print_output == snapshot([('stdout', 'two\n')])


# ---------------------------------------------------------------------------
# `print_callback='collect-string'` mode
# ---------------------------------------------------------------------------


def test_collect_string_basic() -> None:
    """All prints concatenate into a single `str`, in emit order, no stream labels."""
    m = pydantic_monty.Monty('print("a"); print("b", 1)')
    result = m.run(print_callback='collect-string')
    assert result.output is None
    assert result.print_output == snapshot('a\nb 1\n')


def test_collect_string_empty_when_no_prints() -> None:
    m = pydantic_monty.Monty('1 + 1')
    result = m.run(print_callback='collect-string')
    assert result.output == snapshot(2)
    assert result.print_output == snapshot('')


def test_collect_string_across_start_resume() -> None:
    """`collect-string` buffer accumulates the same way across `start`/`resume`."""
    code = """
print("before")
x = magic
print("after", x)
x + 1
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-string')
    assert isinstance(progress, pydantic_monty.NameLookupSnapshot)
    assert progress.print_output == snapshot('before\n')
    complete = progress.resume(value=10)
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output == 11
    assert complete.print_output == snapshot('before\nafter 10\n')


def test_collect_string_on_runtime_error() -> None:
    code = """
print("about to fail")
1 / 0
"""
    m = pydantic_monty.Monty(code)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(print_callback='collect-string')
    assert exc_info.value.print_output == snapshot('about to fail\n')


def test_collect_string_run_async() -> None:
    import asyncio

    async def go() -> pydantic_monty.MontyComplete:
        m = pydantic_monty.Monty('print("async"); 7')
        return await m.run_async(print_callback='collect-string')

    result = asyncio.run(go())
    assert result.output == 7
    assert result.print_output == snapshot('async\n')


def test_collect_string_repl_feed_run() -> None:
    repl = pydantic_monty.MontyRepl()
    r1 = repl.feed_run('print("one"); x = 1', print_callback='collect-string')
    assert r1.output is None
    assert r1.print_output == snapshot('one\n')
    r2 = repl.feed_run('print("two"); x + 1', print_callback='collect-string')
    assert r2.output == 2
    assert r2.print_output == snapshot('two\n')


# ---------------------------------------------------------------------------
# Type and rejection-regression guards
# ---------------------------------------------------------------------------


def test_collect_streams_returns_list_type() -> None:
    """`'collect-streams'` populates `print_output` with a `list`, not a `str`."""
    m = pydantic_monty.Monty('print("x")')
    result = m.run(print_callback='collect-streams')
    assert isinstance(result.print_output, list)


def test_collect_string_returns_str_type() -> None:
    """`'collect-string'` populates `print_output` with a `str`, not a `list`."""
    m = pydantic_monty.Monty('print("x")')
    result = m.run(print_callback='collect-string')
    assert isinstance(result.print_output, str)


def test_legacy_collect_literal_rejected() -> None:
    """The old `'collect'` literal must no longer be accepted (regression guard)."""
    m = pydantic_monty.Monty('1')
    with pytest.raises(TypeError) as exc_info:
        m.run(print_callback='collect')  # type: ignore[arg-type]
    assert exc_info.value.args[0] == snapshot(
        "print_callback string must be 'collect-streams' or 'collect-string', got \"collect\""
    )


# ---------------------------------------------------------------------------
# FunctionSnapshot / FutureSnapshot live peek
# ---------------------------------------------------------------------------


def test_collect_streams_function_snapshot_live_peek() -> None:
    """`FunctionSnapshot.print_output` reflects prints emitted before the call."""
    code = """
print("pre-call")
x = func(1)
print("post-call", x)
x * 2
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-streams')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('func')
    # Live peek before resume sees the pre-call print only.
    assert progress.print_output == snapshot([('stdout', 'pre-call\n')])
    complete = progress.resume(return_value=5)
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output == 10
    # Final buffer includes both prints.
    assert complete.print_output == snapshot([('stdout', 'pre-call\npost-call 5\n')])


def test_collect_string_function_snapshot_live_peek() -> None:
    """`FunctionSnapshot.print_output` in string mode accumulates raw text."""
    code = """
print("pre-call")
x = func(1)
print("post-call", x)
x * 2
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-string')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.print_output == snapshot('pre-call\n')
    complete = progress.resume(return_value=5)
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.print_output == snapshot('pre-call\npost-call 5\n')


def test_collect_streams_future_snapshot_live_peek() -> None:
    """`FutureSnapshot.print_output` sees prints emitted before the await point."""
    code = """
print("before")
result = await foobar(1)
print("after", result)
result + 1
"""
    m = pydantic_monty.Monty(code)
    fn_snap = m.start(print_callback='collect-streams')
    assert isinstance(fn_snap, pydantic_monty.FunctionSnapshot)
    assert fn_snap.print_output == snapshot([('stdout', 'before\n')])
    # Resume with a future → yields FutureSnapshot.
    fut_snap = fn_snap.resume(future=...)
    assert isinstance(fut_snap, pydantic_monty.FutureSnapshot)
    # Live peek on FutureSnapshot still sees the pre-await print.
    assert fut_snap.print_output == snapshot([('stdout', 'before\n')])
    complete = fut_snap.resume({fn_snap.call_id: {'return_value': 10}})
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output == 11
    assert complete.print_output == snapshot([('stdout', 'before\nafter 10\n')])


def test_collect_string_future_snapshot_live_peek() -> None:
    """Same as above for `'collect-string'` — `FutureSnapshot.print_output` is a `str`."""
    code = """
print("before")
result = await foobar(1)
print("after", result)
result + 1
"""
    m = pydantic_monty.Monty(code)
    fn_snap = m.start(print_callback='collect-string')
    assert isinstance(fn_snap, pydantic_monty.FunctionSnapshot)
    fut_snap = fn_snap.resume(future=...)
    assert isinstance(fut_snap, pydantic_monty.FutureSnapshot)
    assert fut_snap.print_output == snapshot('before\n')
    complete = fut_snap.resume({fn_snap.call_id: {'return_value': 10}})
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.print_output == snapshot('before\nafter 10\n')


# ---------------------------------------------------------------------------
# Post-error snapshot drain behavior
# ---------------------------------------------------------------------------


def test_collect_streams_snapshot_emptied_after_error_resume() -> None:
    """After an error, the snapshot's live-peek buffer is emptied.

    `MontyRuntimeError.print_output` carries the buffer; the snapshot's
    underlying shared `Arc` is left with an empty `list` so subsequent
    `.print_output` access on the (now-consumed) snapshot returns `[]`.
    """
    code = """
print("before")
x = func(1)
x + 1
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-streams')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.print_output == snapshot([('stdout', 'before\n')])
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume(exception=ValueError('boom'))
    assert exc_info.value.print_output == snapshot([('stdout', 'before\n')])
    # Snapshot's buffer is now empty — the error drained it.
    assert progress.print_output == snapshot([])


def test_collect_string_snapshot_emptied_after_error_resume() -> None:
    """Same as above, but for `'collect-string'`: snapshot sees `''` after error."""
    code = """
print("before")
x = func(1)
x + 1
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-string')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.print_output == snapshot('before\n')
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume(exception=ValueError('boom'))
    assert exc_info.value.print_output == snapshot('before\n')
    assert progress.print_output == snapshot('')


# ---------------------------------------------------------------------------
# Async external-function buffer sharing
# ---------------------------------------------------------------------------


def test_collect_streams_run_async_with_async_external() -> None:
    """`run_async` buffer survives coroutine-returning external functions.

    The collect buffer is shared across `spawn_blocking` VM transitions and
    the event-loop round-trip that awaits the coroutine. Prints emitted by
    Monty before, between, and after the await must all land in the final
    buffer.
    """
    import asyncio

    code = """
print("before call")
x = await fetch(10)
print("after call", x)
x + 1
"""

    async def fetch(n: int) -> int:
        # Await a real I/O point so the event loop actually yields.
        await asyncio.sleep(0)
        return n * 2

    async def go() -> pydantic_monty.MontyComplete:
        m = pydantic_monty.Monty(code)
        return await m.run_async(
            external_functions={'fetch': fetch},
            print_callback='collect-streams',
        )

    result = asyncio.run(go())
    assert result.output == 21
    assert result.print_output == snapshot([('stdout', 'before call\nafter call 20\n')])


def test_collect_string_run_async_with_async_external() -> None:
    """Same as above for `'collect-string'` — single concatenated buffer."""
    import asyncio

    code = """
print("before call")
x = await fetch(10)
print("after call", x)
x + 1
"""

    async def fetch(n: int) -> int:
        await asyncio.sleep(0)
        return n * 2

    async def go() -> pydantic_monty.MontyComplete:
        m = pydantic_monty.Monty(code)
        return await m.run_async(
            external_functions={'fetch': fetch},
            print_callback='collect-string',
        )

    result = asyncio.run(go())
    assert result.output == 21
    assert result.print_output == snapshot('before call\nafter call 20\n')


# ---------------------------------------------------------------------------
# Snapshot serialization: collect mode can be re-attached after `load_snapshot`
# ---------------------------------------------------------------------------


def test_load_snapshot_with_collect_streams() -> None:
    """`load_snapshot(..., print_callback='collect-streams')` attaches a fresh buffer.

    Serialization does not persist the collect buffer (it lives only in the
    Python-side `PrintTarget`), so the loaded snapshot starts with an empty
    buffer and prints emitted after the resume are collected into it.
    """
    code = """
print("pre-call")
x = func(1)
print("post-call", x)
x + 1
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-streams')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    data = progress.dump()

    loaded = pydantic_monty.load_snapshot(data, print_callback='collect-streams')
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    # Loaded snapshot starts with an empty buffer — the pre-call print was
    # not persisted across serialization.
    assert loaded.print_output == snapshot([])
    complete = loaded.resume(return_value=10)
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output == 11
    # Only the post-resume print is visible.
    assert complete.print_output == snapshot([('stdout', 'post-call 10\n')])


def test_load_snapshot_with_collect_string() -> None:
    """Same as above for `'collect-string'` — fresh empty buffer after load."""
    code = """
x = func(1)
print("after", x)
x + 1
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(print_callback='collect-string')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    data = progress.dump()

    loaded = pydantic_monty.load_snapshot(data, print_callback='collect-string')
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    assert loaded.print_output == snapshot('')
    complete = loaded.resume(return_value=10)
    assert isinstance(complete, pydantic_monty.MontyComplete)
    assert complete.output == 11
    assert complete.print_output == snapshot('after 10\n')


def test_load_snapshot_rejects_legacy_collect_literal() -> None:
    """The old `'collect'` literal is rejected on `load_snapshot` too."""
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    data = progress.dump()
    with pytest.raises(TypeError) as exc_info:
        pydantic_monty.load_snapshot(data, print_callback='collect')  # type: ignore[arg-type]
    assert exc_info.value.args[0] == snapshot(
        "print_callback string must be 'collect-streams' or 'collect-string', got \"collect\""
    )
