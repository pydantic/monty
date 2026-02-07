import asyncio

import pytest
from dirty_equals import IsList
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import run_monty_async


def test_async():
    code = 'await foobar(1, 2)'
    m = pydantic_monty.Monty(code, external_functions=['foobar'])
    progress = m.start()
    assert isinstance(progress, pydantic_monty.MontySnapshot)
    assert progress.function_name == snapshot('foobar')
    assert progress.args == snapshot((1, 2))
    call_id = progress.call_id
    progress = progress.resume(future=...)
    assert isinstance(progress, pydantic_monty.MontyFutureSnapshot)
    assert progress.pending_call_ids == snapshot([call_id])
    progress = progress.resume({call_id: {'return_value': 3}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(3)


def test_asyncio_gather():
    code = """
import asyncio

await asyncio.gather(foo(1), bar(2))
"""
    m = pydantic_monty.Monty(code, external_functions=['foo', 'bar'])
    progress = m.start()
    assert isinstance(progress, pydantic_monty.MontySnapshot)
    assert progress.function_name == snapshot('foo')
    assert progress.args == snapshot((1,))
    foo_call_ids = progress.call_id

    progress = progress.resume(future=...)
    assert isinstance(progress, pydantic_monty.MontySnapshot)
    assert progress.function_name == snapshot('bar')
    assert progress.args == snapshot((2,))
    bar_call_ids = progress.call_id
    progress = progress.resume(future=...)

    assert isinstance(progress, pydantic_monty.MontyFutureSnapshot)
    dump_progress = progress.dump()

    assert progress.pending_call_ids == IsList(foo_call_ids, bar_call_ids, check_order=False)
    progress = progress.resume({foo_call_ids: {'return_value': 3}, bar_call_ids: {'return_value': 4}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot([3, 4])

    progress2 = pydantic_monty.MontyFutureSnapshot.load(dump_progress)
    assert progress2.pending_call_ids == IsList(foo_call_ids, bar_call_ids, check_order=False)
    progress = progress2.resume({bar_call_ids: {'return_value': 14}, foo_call_ids: {'return_value': 13}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot([13, 14])

    progress3 = pydantic_monty.MontyFutureSnapshot.load(dump_progress)
    progress = progress3.resume({bar_call_ids: {'return_value': 14}, foo_call_ids: {'future': ...}})
    assert isinstance(progress, pydantic_monty.MontyFutureSnapshot)

    assert progress.pending_call_ids == [foo_call_ids]
    progress = progress.resume({foo_call_ids: {'return_value': 144}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot([144, 14])


# === Tests for run_monty_async ===


async def test_run_monty_async_sync_function():
    """Test run_monty_async with a basic sync external function."""
    m = pydantic_monty.Monty('get_value()', external_functions=['get_value'])

    def get_value():
        return 42

    result = await run_monty_async(m, external_functions={'get_value': get_value})
    assert result == snapshot(42)


async def test_run_monty_async_async_function():
    """Test run_monty_async with a basic async external function."""
    m = pydantic_monty.Monty('await fetch_data()', external_functions=['fetch_data'])

    async def fetch_data():
        await asyncio.sleep(0.001)
        return 'async result'

    result = await run_monty_async(m, external_functions={'fetch_data': fetch_data})
    assert result == snapshot('async result')


async def test_run_monty_async_function_not_found():
    """Test that missing external function raises wrapped error."""
    m = pydantic_monty.Monty('missing_func()', external_functions=['missing_func'])

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await run_monty_async(m, external_functions={})
    inner = exc_info.value.exception()
    assert isinstance(inner, KeyError)
    assert inner.args[0] == snapshot("'Function missing_func not found'")


async def test_run_monty_async_sync_exception():
    """Test that sync function exceptions propagate correctly."""
    m = pydantic_monty.Monty('fail()', external_functions=['fail'])

    def fail():
        raise ValueError('sync error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await run_monty_async(m, external_functions={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('sync error')


async def test_run_monty_async_async_exception():
    """Test that async function exceptions propagate correctly."""
    m = pydantic_monty.Monty('await async_fail()', external_functions=['async_fail'])

    async def async_fail():
        await asyncio.sleep(0.001)
        raise RuntimeError('async error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await run_monty_async(m, external_functions={'async_fail': async_fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert inner.args[0] == snapshot('async error')


async def test_run_monty_async_exception_caught():
    """Test that exceptions caught in try/except don't propagate."""
    code = """
try:
    fail()
except ValueError:
    caught = True
caught
"""
    m = pydantic_monty.Monty(code, external_functions=['fail'])

    def fail():
        raise ValueError('caught error')

    result = await run_monty_async(m, external_functions={'fail': fail})
    assert result == snapshot(True)


async def test_run_monty_async_multiple_async_functions():
    """Test asyncio.gather with multiple async functions."""
    code = """
import asyncio
await asyncio.gather(fetch_a(), fetch_b())
"""
    m = pydantic_monty.Monty(code, external_functions=['fetch_a', 'fetch_b'])

    async def fetch_a():
        await asyncio.sleep(0.01)
        return 'a'

    async def fetch_b():
        await asyncio.sleep(0.005)
        return 'b'

    result = await run_monty_async(m, external_functions={'fetch_a': fetch_a, 'fetch_b': fetch_b})
    assert result == snapshot(['a', 'b'])


async def test_run_monty_async_mixed_sync_async():
    """Test mix of sync and async external functions."""
    code = """
sync_val = sync_func()
async_val = await async_func()
sync_val + async_val
"""
    m = pydantic_monty.Monty(code, external_functions=['sync_func', 'async_func'])

    def sync_func():
        return 10

    async def async_func():
        await asyncio.sleep(0.001)
        return 5

    result = await run_monty_async(m, external_functions={'sync_func': sync_func, 'async_func': async_func})
    assert result == snapshot(15)


async def test_run_monty_async_with_inputs():
    """Test run_monty_async with inputs parameter."""
    m = pydantic_monty.Monty('process(x, y)', inputs=['x', 'y'], external_functions=['process'])

    def process(a: int, b: int) -> int:
        return a * b

    result = await run_monty_async(m, inputs={'x': 6, 'y': 7}, external_functions={'process': process})
    assert result == snapshot(42)


async def test_run_monty_async_with_print_callback():
    """Test run_monty_async with print_callback parameter."""
    output: list[tuple[str, str]] = []

    def callback(stream: str, text: str) -> None:
        output.append((stream, text))

    m = pydantic_monty.Monty('print("hello from async")')
    result = await run_monty_async(m, print_callback=callback)
    assert result is None
    assert output == snapshot([('stdout', 'hello from async'), ('stdout', '\n')])


async def test_run_monty_async_function_returning_none():
    """Test async function that returns None."""
    m = pydantic_monty.Monty('do_nothing()', external_functions=['do_nothing'])

    def do_nothing():
        return None

    result = await run_monty_async(m, external_functions={'do_nothing': do_nothing})
    assert result is None


async def test_run_monty_async_no_external_calls():
    """Test run_monty_async when code has no external calls."""
    m = pydantic_monty.Monty('1 + 2 + 3')
    result = await run_monty_async(m)
    assert result == snapshot(6)


# === Tests for run_monty_async with os parameter ===


async def test_run_monty_async_with_os():
    """run_monty_async can use OSAccess for file operations."""
    from pydantic_monty import MemoryFile, OSAccess

    fs = OSAccess([MemoryFile('/test.txt', content='hello world')])

    m = pydantic_monty.Monty(
        """
from pathlib import Path
Path('/test.txt').read_text()
        """,
        external_functions=[],
    )

    result = await run_monty_async(m, os=fs)
    assert result == snapshot('hello world')


async def test_run_monty_async_os_with_external_functions():
    """run_monty_async can combine OSAccess with external functions."""
    from pydantic_monty import MemoryFile, OSAccess

    fs = OSAccess([MemoryFile('/data.txt', content='test data')])

    async def process(text: str) -> str:
        return text.upper()

    m = pydantic_monty.Monty(
        """
from pathlib import Path
content = Path('/data.txt').read_text()
await process(content)
        """,
        external_functions=['process'],
    )

    result = await run_monty_async(
        m,
        external_functions={'process': process},
        os=fs,
    )
    assert result == snapshot('TEST DATA')


async def test_run_monty_async_os_file_not_found():
    """run_monty_async propagates OS errors correctly."""
    from pydantic_monty import OSAccess

    fs = OSAccess()

    m = pydantic_monty.Monty(
        """
from pathlib import Path
Path('/missing.txt').read_text()
        """,
    )

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await run_monty_async(m, os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing.txt'")


async def test_run_monty_async_os_not_provided():
    """run_monty_async raises error when OS function called without os handler."""
    m = pydantic_monty.Monty(
        """
from pathlib import Path
Path('/test.txt').exists()
        """,
    )

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await run_monty_async(m)
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert 'OS function' in inner.args[0]
    assert 'no os handler provided' in inner.args[0]


async def test_run_monty_async_os_write_and_read():
    """run_monty_async supports both reading and writing files."""
    from pydantic_monty import MemoryFile, OSAccess

    fs = OSAccess([MemoryFile('/file.txt', content='original')])

    m = pydantic_monty.Monty(
        """
from pathlib import Path
p = Path('/file.txt')
p.write_text('updated')
p.read_text()
        """,
    )

    result = await run_monty_async(m, os=fs)
    assert result == snapshot('updated')
