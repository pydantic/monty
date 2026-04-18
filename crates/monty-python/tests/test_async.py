import asyncio

import pytest
from dirty_equals import IsList
from inline_snapshot import snapshot

import pydantic_monty


def test_async():
    code = 'await foobar(1, 2)'
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('foobar')
    assert progress.args == snapshot((1, 2))
    call_id = progress.call_id
    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    assert progress.pending_call_ids == snapshot([call_id])
    progress = progress.resume({call_id: {'return_value': 3}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(3)


def test_asyncio_gather():
    code = """
import asyncio

await asyncio.gather(foo(1), bar(2))
"""
    m = pydantic_monty.Monty(code)
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('foo')
    assert progress.args == snapshot((1,))
    foo_call_ids = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('bar')
    assert progress.args == snapshot((2,))
    bar_call_ids = progress.call_id
    progress = progress.resume({'future': ...})

    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    dump_progress = progress.dump()

    assert progress.pending_call_ids == IsList(foo_call_ids, bar_call_ids, check_order=False)
    progress = progress.resume({foo_call_ids: {'return_value': 3}, bar_call_ids: {'return_value': 4}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot([3, 4])

    progress2 = pydantic_monty.load_snapshot(dump_progress)
    assert isinstance(progress2, pydantic_monty.FutureSnapshot)
    assert progress2.pending_call_ids == IsList(foo_call_ids, bar_call_ids, check_order=False)
    progress = progress2.resume({bar_call_ids: {'return_value': 14}, foo_call_ids: {'return_value': 13}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot([13, 14])

    progress3 = pydantic_monty.load_snapshot(dump_progress)
    assert isinstance(progress3, pydantic_monty.FutureSnapshot)
    progress = progress3.resume({bar_call_ids: {'return_value': 14}, foo_call_ids: {'future': ...}})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)

    assert progress.pending_call_ids == [foo_call_ids]
    progress = progress.resume({foo_call_ids: {'return_value': 144}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot([144, 14])


# === Tests for run_monty_async ===


async def test_run_monty_async_sync_function():
    """Test run_monty_async with a basic sync external function."""
    m = pydantic_monty.Monty('get_value()')

    def get_value():
        return 42

    result = await m.run_async(external_functions={'get_value': get_value})
    assert result == snapshot(42)


async def test_run_monty_async_async_function():
    """Test run_monty_async with a basic async external function."""
    m = pydantic_monty.Monty('await fetch_data()')

    async def fetch_data():
        await asyncio.sleep(0.001)
        return 'async result'

    result = await m.run_async(external_functions={'fetch_data': fetch_data})
    assert result == snapshot('async result')


async def test_run_monty_async_function_not_found():
    """Test that missing external function raises wrapped error."""
    m = pydantic_monty.Monty('missing_func()')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await m.run_async(external_functions={})
    inner = exc_info.value.exception()
    assert isinstance(inner, NameError)
    assert inner.args[0] == snapshot("name 'missing_func' is not defined")


async def test_run_monty_async_sync_exception():
    """Test that sync function exceptions propagate correctly."""
    m = pydantic_monty.Monty('fail()')

    def fail():
        raise ValueError('sync error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await m.run_async(external_functions={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('sync error')


async def test_run_monty_async_async_exception():
    """Test that async function exceptions propagate correctly."""
    m = pydantic_monty.Monty('await async_fail()')

    async def async_fail():
        await asyncio.sleep(0.001)
        raise RuntimeError('async error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await m.run_async(external_functions={'async_fail': async_fail})
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
    m = pydantic_monty.Monty(code)

    def fail():
        raise ValueError('caught error')

    result = await m.run_async(external_functions={'fail': fail})
    assert result == snapshot(True)


async def test_run_monty_async_multiple_async_functions():
    """Test asyncio.gather with multiple async functions."""
    code = """
import asyncio
await asyncio.gather(fetch_a(), fetch_b())
"""
    m = pydantic_monty.Monty(code)

    async def fetch_a():
        await asyncio.sleep(0.01)
        return 'a'

    async def fetch_b():
        await asyncio.sleep(0.005)
        return 'b'

    result = await m.run_async(external_functions={'fetch_a': fetch_a, 'fetch_b': fetch_b})
    assert result == snapshot(['a', 'b'])


async def test_run_monty_async_mixed_sync_async():
    """Test mix of sync and async external functions."""
    code = """
sync_val = sync_func()
async_val = await async_func()
sync_val + async_val
"""
    m = pydantic_monty.Monty(code)

    def sync_func():
        return 10

    async def async_func():
        await asyncio.sleep(0.001)
        return 5

    result = await m.run_async(external_functions={'sync_func': sync_func, 'async_func': async_func})
    assert result == snapshot(15)


async def test_run_monty_async_with_inputs():
    """Test run_monty_async with inputs parameter."""
    m = pydantic_monty.Monty('process(x, y)', inputs=['x', 'y'])

    def process(a: int, b: int) -> int:
        return a * b

    result = await m.run_async(inputs={'x': 6, 'y': 7}, external_functions={'process': process})
    assert result == snapshot(42)


async def test_run_monty_async_with_print_callback():
    """Test run_monty_async with print_callback parameter."""
    output: list[tuple[str, str]] = []

    def callback(stream: str, text: str) -> None:
        output.append((stream, text))

    m = pydantic_monty.Monty('print("hello from async")')
    result = await m.run_async(print_callback=callback)
    assert result is None
    assert output == snapshot([('stdout', 'hello from async'), ('stdout', '\n')])


async def test_run_monty_async_function_returning_none():
    """Test async function that returns None."""
    m = pydantic_monty.Monty('do_nothing()')

    def do_nothing():
        return None

    result = await m.run_async(external_functions={'do_nothing': do_nothing})
    assert result is None


async def test_run_monty_async_no_external_calls():
    """Test run_monty_async when code has no external calls."""
    m = pydantic_monty.Monty('1 + 2 + 3')
    result = await m.run_async()
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
    )

    result = await m.run_async(os=fs)
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
    )

    result = await m.run_async(
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
        await m.run_async(os=fs)
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
        await m.run_async()
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert 'OS function' in inner.args[0]
    assert 'not implemented' in inner.args[0]


async def test_run_monty_async_os_callback_not_handled():
    """`NOT_HANDLED` from the async os callback falls through to default unhandled behavior.

    Mirrors the sync `Monty.run(os=...)` semantics — without this, the sentinel
    object would be passed through `py_to_monty` and surface as a TypeError.
    """

    def os_cb(func: object, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        return pydantic_monty.NOT_HANDLED

    m = pydantic_monty.Monty("from pathlib import Path; Path('/foo.txt').exists()")
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await m.run_async(os=os_cb)  # pyright: ignore[reportArgumentType]
    inner = exc_info.value.exception()
    assert isinstance(inner, PermissionError)


async def test_repl_feed_run_async_os_callback_not_handled():
    """Same as above but for `MontyRepl.feed_run_async`."""

    def os_cb(func: object, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        return pydantic_monty.NOT_HANDLED

    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await repl.feed_run_async(
            "from pathlib import Path; Path('/foo.txt').exists()",
            os=os_cb,  # pyright: ignore[reportArgumentType]
        )
    inner = exc_info.value.exception()
    assert isinstance(inner, PermissionError)


async def test_run_monty_async_nested_gather_with_external_functions():
    """Test nested asyncio.gather with spawned tasks and external async functions.

    https://github.com/pydantic/monty/pull/174

    Reproduces the pattern from stack_overflow.py: outer gather spawns 3 coroutine tasks,
    each doing a sequential await then an inner gather with 2 external futures.
    """
    code = """\
import asyncio

async def get_city_weather(city_name: str):
    coords = await get_lat_lng(location_description=city_name)
    lat, lng = coords['lat'], coords['lng']
    temp_task = get_temp(lat=lat, lng=lng)
    desc_task = get_weather_description(lat=lat, lng=lng)
    temp, desc = await asyncio.gather(temp_task, desc_task)
    return {
        'city': city_name,
        'temp': temp,
        'description': desc
    }

async def main():
    cities = ['London', 'Paris', 'Tokyo']
    results = await asyncio.gather(*(get_city_weather(city) for city in cities))
    return results

await main()
"""
    m = pydantic_monty.Monty(code)

    city_coords = {
        'London': {'lat': 51.5, 'lng': -0.1},
        'Paris': {'lat': 48.9, 'lng': 2.3},
        'Tokyo': {'lat': 35.7, 'lng': 139.7},
    }
    city_temps = {
        (51.5, -0.1): 15.0,
        (48.9, 2.3): 18.0,
        (35.7, 139.7): 22.0,
    }
    city_descs = {
        (51.5, -0.1): 'Cloudy',
        (48.9, 2.3): 'Sunny',
        (35.7, 139.7): 'Humid',
    }

    async def get_lat_lng(location_description: str):
        return city_coords[location_description]

    async def get_temp(lat: float, lng: float):
        return city_temps[(lat, lng)]

    async def get_weather_description(lat: float, lng: float):
        return city_descs[(lat, lng)]

    result = await m.run_async(
        external_functions={
            'get_lat_lng': get_lat_lng,
            'get_temp': get_temp,
            'get_weather_description': get_weather_description,
        },
    )
    assert result == snapshot(
        [
            {'city': 'London', 'temp': 15.0, 'description': 'Cloudy'},
            {'city': 'Paris', 'temp': 18.0, 'description': 'Sunny'},
            {'city': 'Tokyo', 'temp': 22.0, 'description': 'Humid'},
        ]
    )


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

    result = await m.run_async(os=fs)
    assert result == snapshot('updated')


# === Tests for MontyRepl.feed_start() with async patterns ===


def test_repl_feed_start_async_gather():
    """MontyRepl.feed_start supports asyncio.gather with multiple futures."""
    code = """
import asyncio

await asyncio.gather(foo(1), bar(2))
"""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start(code)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('foo')
    foo_call_id = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('bar')
    bar_call_id = progress.call_id
    progress = progress.resume({'future': ...})

    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    from dirty_equals import IsList

    assert progress.pending_call_ids == IsList(foo_call_id, bar_call_id, check_order=False)
    progress = progress.resume({foo_call_id: {'return_value': 3}, bar_call_id: {'return_value': 4}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot([3, 4])

    # REPL should still be usable after async completion
    assert repl.feed_run('1 + 1') == snapshot(2)


def test_repl_feed_start_async_state_persistence():
    """MontyRepl.feed_start async: REPL state persists across async snippets."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 10')

    progress = repl.feed_start('result = await fetch(x)')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('fetch')
    assert progress.args == snapshot((10,))
    call_id = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    progress = progress.resume({call_id: {'return_value': 'fetched'}})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output is None  # assignment, not expression

    assert repl.feed_run('result') == snapshot('fetched')
    assert repl.feed_run('x') == snapshot(10)


# === Tests for run_repl_async ===


async def test_run_repl_async_sync_function():
    """run_repl_async with a basic sync external function."""
    repl = pydantic_monty.MontyRepl()

    def get_value():
        return 42

    result = await repl.feed_run_async('get_value()', external_functions={'get_value': get_value})
    assert result == snapshot(42)


async def test_run_repl_async_async_function():
    """run_repl_async with a basic async external function."""
    repl = pydantic_monty.MontyRepl()

    async def fetch_data():
        await asyncio.sleep(0.001)
        return 'async result'

    result = await repl.feed_run_async('await fetch_data()', external_functions={'fetch_data': fetch_data})
    assert result == snapshot('async result')


async def test_run_repl_async_state_persists():
    """REPL state persists across multiple run_repl_async calls."""
    repl = pydantic_monty.MontyRepl()

    def double(x: int) -> int:
        return x * 2

    ext = {'double': double}
    await repl.feed_run_async('x = 10', external_functions=ext)
    await repl.feed_run_async('y = double(x)', external_functions=ext)
    result = await repl.feed_run_async('y', external_functions=ext)
    assert result == snapshot(20)


async def test_run_repl_async_async_state_persists():
    """REPL state persists across async calls with await."""
    repl = pydantic_monty.MontyRepl()

    async def fetch(key: str) -> str:
        return f'value_{key}'

    ext = {'fetch': fetch}
    await repl.feed_run_async("a = await fetch('one')", external_functions=ext)
    await repl.feed_run_async("b = await fetch('two')", external_functions=ext)
    result = await repl.feed_run_async('a + b', external_functions=ext)
    assert result == snapshot('value_onevalue_two')


async def test_run_repl_async_gather():
    """run_repl_async handles asyncio.gather with multiple futures."""
    repl = pydantic_monty.MontyRepl()

    async def fetch_a():
        await asyncio.sleep(0.01)
        return 'a'

    async def fetch_b():
        await asyncio.sleep(0.005)
        return 'b'

    code = """\
import asyncio
await asyncio.gather(fetch_a(), fetch_b())
"""
    result = await repl.feed_run_async(code, external_functions={'fetch_a': fetch_a, 'fetch_b': fetch_b})
    assert result == snapshot(['a', 'b'])


async def test_run_repl_async_function_not_found():
    """run_repl_async raises error for missing external function."""
    repl = pydantic_monty.MontyRepl()

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await repl.feed_run_async('missing_func()', external_functions={})
    inner = exc_info.value.exception()
    assert isinstance(inner, NameError)
    assert inner.args[0] == snapshot("name 'missing_func' is not defined")


async def test_run_repl_async_error_preserves_state():
    """REPL state is preserved after an error in run_repl_async."""
    repl = pydantic_monty.MontyRepl()
    await repl.feed_run_async('x = 42')

    def fail():
        raise ValueError('oops')

    with pytest.raises(pydantic_monty.MontyRuntimeError):
        await repl.feed_run_async('fail()', external_functions={'fail': fail})

    result = await repl.feed_run_async('x')
    assert result == snapshot(42)


async def test_run_repl_async_with_inputs():
    """run_repl_async supports inputs parameter."""
    repl = pydantic_monty.MontyRepl()

    def add(a: int, b: int) -> int:
        return a + b

    result = await repl.feed_run_async('add(x, y)', inputs={'x': 3, 'y': 4}, external_functions={'add': add})
    assert result == snapshot(7)


async def test_run_repl_async_with_print_callback():
    """run_repl_async supports print_callback parameter."""
    repl = pydantic_monty.MontyRepl()
    output: list[str] = []

    def callback(stream: str, text: str) -> None:
        output.append(text)

    await repl.feed_run_async('print("hello from repl")', print_callback=callback)
    assert output == snapshot(['hello from repl', '\n'])


async def test_run_repl_async_with_os():
    """run_repl_async supports OS access."""
    from pydantic_monty import MemoryFile, OSAccess

    repl = pydantic_monty.MontyRepl()
    fs = OSAccess([MemoryFile('/test.txt', content='repl content')])

    code = """\
from pathlib import Path
Path('/test.txt').read_text()
"""
    result = await repl.feed_run_async(code, os=fs)
    assert result == snapshot('repl content')


async def test_run_repl_async_mixed_sync_async():
    """run_repl_async handles mix of sync and async functions."""
    repl = pydantic_monty.MontyRepl()

    def sync_func():
        return 10

    async def async_func():
        await asyncio.sleep(0.001)
        return 5

    code = """\
sync_val = sync_func()
async_val = await async_func()
sync_val + async_val
"""
    result = await repl.feed_run_async(code, external_functions={'sync_func': sync_func, 'async_func': async_func})
    assert result == snapshot(15)


async def test_run_repl_async_no_external_calls():
    """run_repl_async works when code has no external calls."""
    repl = pydantic_monty.MontyRepl()
    result = await repl.feed_run_async('1 + 2 + 3')
    assert result == snapshot(6)


# === LLM agent patterns: realistic run_repl_async scenarios ===


async def test_repl_llm_iterative_data_collection():
    """LLM defines a helper, collects data in batches, accumulates results across snippets."""
    repl = pydantic_monty.MontyRepl()

    responses: dict[int, list[dict[str, object]]] = {
        0: [{'id': 1, 'name': 'Alice'}, {'id': 2, 'name': 'Bob'}],
        2: [{'id': 3, 'name': 'Charlie'}],
        3: [],
    }

    async def fetch_users(offset: int, limit: int) -> list[dict[str, object]]:
        return responses.get(offset, [])

    ext = {'fetch_users': fetch_users}

    # Snippet 1: LLM sets up accumulator
    await repl.feed_run_async('all_users = []', external_functions=ext)

    # Snippet 2: LLM fetches first batch
    await repl.feed_run_async(
        """\
batch = await fetch_users(0, 2)
all_users = all_users + batch
len(batch)
""",
        external_functions=ext,
    )

    # Snippet 3: LLM fetches next batch using state
    await repl.feed_run_async(
        """\
batch = await fetch_users(len(all_users), 2)
all_users = all_users + batch
len(batch)
""",
        external_functions=ext,
    )

    # Snippet 4: LLM fetches again, gets empty — realizes done
    await repl.feed_run_async(
        """\
batch = await fetch_users(len(all_users), 2)
all_users = all_users + batch
len(batch)
""",
        external_functions=ext,
    )

    # Snippet 5: LLM extracts final result
    result = await repl.feed_run_async('[u["name"] for u in all_users]', external_functions=ext)
    assert result == snapshot(['Alice', 'Bob', 'Charlie'])


async def test_repl_llm_error_recovery_retry():
    """LLM catches an error, adjusts approach, retries successfully."""
    repl = pydantic_monty.MontyRepl()
    call_count = 0

    async def flaky_api(query: str) -> str:
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            raise ConnectionError('server unavailable')
        return f'result for {query}'

    ext = {'flaky_api': flaky_api}

    # Snippet 1: LLM tries, gets error
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        await repl.feed_run_async("data = await flaky_api('test')", external_functions=ext)

    # Snippet 2: LLM wraps in try/except and retries
    result = await repl.feed_run_async(
        """\
try:
    data = await flaky_api('test')
except Exception as e:
    data = 'fallback'
data
""",
        external_functions=ext,
    )
    assert result == snapshot('result for test')


async def test_repl_llm_redefine_helper_function():
    """LLM defines a function, uses it, then redefines it with improvements."""
    repl = pydantic_monty.MontyRepl()

    async def fetch(url: str) -> str:
        return f'<html>{url}</html>'

    ext = {'fetch': fetch}

    # Snippet 1: LLM defines initial parser
    await repl.feed_run_async(
        """\
def parse_title(html):
    return html
""",
        external_functions=ext,
    )

    # Snippet 2: LLM uses it, gets raw html back
    result = await repl.feed_run_async(
        """\
html = await fetch('example.com')
parse_title(html)
""",
        external_functions=ext,
    )
    assert result == snapshot('<html>example.com</html>')

    # Snippet 3: LLM redefines parser with better logic
    await repl.feed_run_async(
        """\
def parse_title(html):
    start = html.find('>') + 1
    end = html.rfind('<')
    return html[start:end]
""",
        external_functions=ext,
    )

    # Snippet 4: uses improved parser on previously fetched data
    result = await repl.feed_run_async('parse_title(html)', external_functions=ext)
    assert result == snapshot('example.com')


async def test_repl_llm_sequential_async_pipeline():
    """LLM builds a data pipeline: fetch -> transform -> store, each step depends on previous."""
    repl = pydantic_monty.MontyRepl()

    async def search(query: str) -> list[str]:
        return [f'{query}_result_1', f'{query}_result_2']

    async def summarize(text: str) -> str:
        return f'summary({text})'

    records: list[str] = []

    def record(item: str) -> None:
        records.append(item)

    ext = {'search': search, 'summarize': summarize, 'record': record}

    code = """\
results = await search('python async')
summaries = []
for r in results:
    s = await summarize(r)
    summaries.append(s)
    record(s)
summaries
"""
    result = await repl.feed_run_async(code, external_functions=ext)
    assert result == snapshot(['summary(python async_result_1)', 'summary(python async_result_2)'])
    assert records == snapshot(['summary(python async_result_1)', 'summary(python async_result_2)'])


async def test_repl_llm_gather_fan_out():
    """LLM uses asyncio.gather to fan out many concurrent requests."""
    repl = pydantic_monty.MontyRepl()

    async def fetch_price(item: str) -> float:
        prices = {'apple': 1.5, 'banana': 0.75, 'cherry': 3.0, 'date': 5.0, 'elderberry': 8.0}
        return prices[item]

    ext = {'fetch_price': fetch_price}

    code = """\
import asyncio

items = ['apple', 'banana', 'cherry', 'date', 'elderberry']
prices = await asyncio.gather(*(fetch_price(item) for item in items))
dict(zip(items, prices))
"""
    result = await repl.feed_run_async(code, external_functions=ext)
    assert result == snapshot({'apple': 1.5, 'banana': 0.75, 'cherry': 3.0, 'date': 5.0, 'elderberry': 8.0})


async def test_repl_llm_try_except_around_external():
    """LLM wraps individual external calls in try/except for graceful degradation."""
    repl = pydantic_monty.MontyRepl()

    def fetch_data(key: str) -> str:
        if key == 'bad':
            raise KeyError(f'no data for {key}')
        return f'data_{key}'

    ext = {'fetch_data': fetch_data}

    code = """\
results = {}
for key in ['good', 'bad', 'also_good']:
    try:
        results[key] = fetch_data(key)
    except KeyError:
        results[key] = 'missing'
results
"""
    result = await repl.feed_run_async(code, external_functions=ext)
    assert result == snapshot({'good': 'data_good', 'bad': 'missing', 'also_good': 'data_also_good'})


async def test_repl_llm_conditional_external_call():
    """LLM only calls external function when a condition is met."""
    repl = pydantic_monty.MontyRepl()
    call_count = 0

    async def expensive_lookup(key: str) -> str:
        nonlocal call_count
        call_count += 1
        return f'looked up {key}'

    ext = {'expensive_lookup': expensive_lookup}

    # Snippet 1: set up a cache
    await repl.feed_run_async("cache = {'x': 'cached_x'}", external_functions=ext)

    # Snippet 2: LLM checks cache before calling
    code = """\
results = []
for key in ['x', 'y', 'x']:
    if key in cache:
        results.append(cache[key])
    else:
        val = await expensive_lookup(key)
        cache[key] = val
        results.append(val)
results
"""
    result = await repl.feed_run_async(code, external_functions=ext)
    assert result == snapshot(['cached_x', 'looked up y', 'cached_x'])
    assert call_count == 1  # only 'y' triggered a call


async def test_repl_llm_side_effect_recording():
    """LLM uses a side-effect-only external function to record structured data."""
    repl = pydantic_monty.MontyRepl()
    recorded: list[dict[str, object]] = []

    def record_model(name: str, params: str, price: float) -> None:
        recorded.append({'name': name, 'params': params, 'price': price})

    async def get_models() -> list[dict[str, str]]:
        return [
            {'name': 'gpt-4', 'params': '1.7T'},
            {'name': 'claude-3', 'params': '???'},
        ]

    ext = {'record_model': record_model, 'get_models': get_models}

    code = """\
models = await get_models()
for m in models:
    record_model(m['name'], m['params'], 0.01)
len(models)
"""
    result = await repl.feed_run_async(code, external_functions=ext)
    assert result == snapshot(2)
    assert recorded == snapshot(
        [{'name': 'gpt-4', 'params': '1.7T', 'price': 0.01}, {'name': 'claude-3', 'params': '???', 'price': 0.01}]
    )


async def test_repl_llm_helper_wrapping_externals_with_retry():
    """LLM defines a helper function that wraps external calls with retry logic."""
    repl = pydantic_monty.MontyRepl()
    attempt_counts: dict[str, int] = {}

    def unreliable_fetch(url: str) -> str:
        attempt_counts.setdefault(url, 0)
        attempt_counts[url] += 1
        if attempt_counts[url] < 2:
            raise ValueError('temporary failure')
        return f'content of {url}'

    ext = {'unreliable_fetch': unreliable_fetch}

    # Snippet 1: LLM defines retry helper
    await repl.feed_run_async(
        """\
def fetch_with_retry(url, max_retries=3):
    for i in range(max_retries):
        try:
            return unreliable_fetch(url)
        except ValueError:
            if i == max_retries - 1:
                raise
    raise ValueError('should not reach here')
""",
        external_functions=ext,
    )

    # Snippet 2: LLM uses the retry helper
    result = await repl.feed_run_async("fetch_with_retry('example.com')", external_functions=ext)
    assert result == snapshot('content of example.com')
    assert attempt_counts == snapshot({'example.com': 2})


async def test_repl_llm_nested_gather_with_sequential_deps():
    """LLM does gather of tasks where each task has sequential async steps internally."""
    repl = pydantic_monty.MontyRepl()

    async def get_user(user_id: int) -> dict[str, object]:
        return {'id': user_id, 'name': f'user_{user_id}'}

    async def get_posts(user_id: int) -> list[str]:
        return [f'post_{user_id}_1', f'post_{user_id}_2']

    ext = {'get_user': get_user, 'get_posts': get_posts}

    code = """\
import asyncio

async def get_user_with_posts(uid):
    user = await get_user(uid)
    posts = await get_posts(uid)
    user['posts'] = posts
    return user

results = await asyncio.gather(
    get_user_with_posts(1),
    get_user_with_posts(2),
    get_user_with_posts(3),
)
results
"""
    result = await repl.feed_run_async(code, external_functions=ext)
    assert result == snapshot(
        [
            {'id': 1, 'name': 'user_1', 'posts': ['post_1_1', 'post_1_2']},
            {'id': 2, 'name': 'user_2', 'posts': ['post_2_1', 'post_2_2']},
            {'id': 3, 'name': 'user_3', 'posts': ['post_3_1', 'post_3_2']},
        ]
    )


async def test_repl_llm_external_returns_complex_nested_structure():
    """LLM processes deeply nested API response from external function."""
    repl = pydantic_monty.MontyRepl()

    async def get_api_response() -> dict[str, object]:
        return {
            'status': 'ok',
            'data': {
                'users': [
                    {'name': 'Alice', 'scores': [95, 87, 92]},
                    {'name': 'Bob', 'scores': [78, 85, 90]},
                ],
                'metadata': {'page': 1, 'total': 2},
            },
        }

    ext = {'get_api_response': get_api_response}

    # Snippet 1: fetch and store
    await repl.feed_run_async('response = await get_api_response()', external_functions=ext)

    # Snippet 2: LLM navigates nested structure
    result = await repl.feed_run_async(
        """\
users = response['data']['users']
averages = {}
for u in users:
    avg = sum(u['scores']) / len(u['scores'])
    averages[u['name']] = round(avg, 1)
averages
""",
        external_functions=ext,
    )
    assert result == snapshot({'Alice': 91.3, 'Bob': 84.3})


async def test_repl_llm_external_with_kwargs():
    """LLM calls external functions using keyword arguments."""
    repl = pydantic_monty.MontyRepl()

    async def search(query: str, limit: int = 10, offset: int = 0) -> dict[str, object]:
        return {'query': query, 'limit': limit, 'offset': offset, 'results': [f'{query}_{i}' for i in range(limit)]}

    ext = {'search': search}

    code = """\
page1 = await search('test', limit=2, offset=0)
page2 = await search('test', limit=2, offset=2)
page1['results'] + page2['results']
"""
    result = await repl.feed_run_async(code, external_functions=ext)
    assert result == snapshot(['test_0', 'test_1', 'test_0', 'test_1'])


async def test_repl_llm_os_read_then_process_with_external():
    """LLM reads a file via OS, then processes content with an async external function."""
    from pydantic_monty import MemoryFile, OSAccess

    repl = pydantic_monty.MontyRepl()
    fs = OSAccess([MemoryFile('/data.csv', content='alice,95\nbob,87\ncharlie,92')])

    async def analyze(text: str) -> dict[str, int]:
        rows = text.strip().split('\n')
        return {name: int(score) for name, score in (r.split(',') for r in rows)}

    ext = {'analyze': analyze}

    # Snippet 1: read file
    await repl.feed_run_async(
        """\
from pathlib import Path
raw = Path('/data.csv').read_text()
""",
        external_functions=ext,
        os=fs,
    )

    # Snippet 2: process with external
    result = await repl.feed_run_async('await analyze(raw)', external_functions=ext, os=fs)
    assert result == snapshot({'alice': 95, 'bob': 87, 'charlie': 92})


async def test_repl_llm_long_multi_step_session():
    """Simulates a multi-step LLM agent session: setup, explore, process, summarize."""
    repl = pydantic_monty.MontyRepl()

    db: dict[str, list[dict[str, object]]] = {
        'products': [
            {'name': 'Widget', 'price': 9.99, 'category': 'tools'},
            {'name': 'Gadget', 'price': 24.99, 'category': 'electronics'},
            {'name': 'Doohickey', 'price': 4.99, 'category': 'tools'},
            {'name': 'Thingamajig', 'price': 49.99, 'category': 'electronics'},
        ],
    }

    async def query_db(table: str, filters: dict[str, str] | None = None) -> list[dict[str, object]]:
        rows = db.get(table, [])
        if filters:
            for k, v in filters.items():
                rows = [r for r in rows if r.get(k) == v]
        return rows

    ext = {'query_db': query_db}

    # Step 1: LLM explores what's available
    result = await repl.feed_run_async('await query_db("products")', external_functions=ext)
    assert len(result) == 4

    # Step 2: LLM filters by category
    await repl.feed_run_async(
        "tools = await query_db('products', filters={'category': 'tools'})",
        external_functions=ext,
    )

    # Step 3: LLM computes stats
    result = await repl.feed_run_async(
        """\
total = sum(p['price'] for p in tools)
avg = total / len(tools)
{'count': len(tools), 'total': round(total, 2), 'average': round(avg, 2)}
""",
        external_functions=ext,
    )
    assert result == snapshot({'count': 2, 'total': 14.98, 'average': 7.49})

    # Step 4: LLM also checks electronics
    await repl.feed_run_async(
        "electronics = await query_db('products', filters={'category': 'electronics'})",
        external_functions=ext,
    )

    # Step 5: LLM builds final summary from accumulated state
    result = await repl.feed_run_async(
        """\
summary = {}
for cat, items in [('tools', tools), ('electronics', electronics)]:
    summary[cat] = {
        'count': len(items),
        'total': round(sum(i['price'] for i in items), 2),
        'items': [i['name'] for i in items],
    }
summary
""",
        external_functions=ext,
    )
    assert result == snapshot(
        {
            'tools': {'count': 2, 'total': 14.98, 'items': ['Widget', 'Doohickey']},
            'electronics': {'count': 2, 'total': 74.98, 'items': ['Gadget', 'Thingamajig']},
        }
    )


async def test_repl_llm_string_manipulation_of_external_result():
    """LLM fetches HTML-like content and does string processing across snippets."""
    repl = pydantic_monty.MontyRepl()

    async def fetch_page(url: str) -> str:
        return '<title>Test Page</title><body><p>Hello</p><p>World</p></body>'

    ext = {'fetch_page': fetch_page}

    await repl.feed_run_async("html = await fetch_page('example.com')", external_functions=ext)

    # LLM extracts title
    result = await repl.feed_run_async(
        """\
start = html.find('<title>') + len('<title>')
end = html.find('</title>')
title = html[start:end]
title
""",
        external_functions=ext,
    )
    assert result == snapshot('Test Page')

    # LLM extracts paragraphs
    result = await repl.feed_run_async(
        """\
paragraphs = []
remaining = html
while '<p>' in remaining:
    s = remaining.find('<p>') + 3
    e = remaining.find('</p>')
    paragraphs.append(remaining[s:e])
    remaining = remaining[e + 4:]
paragraphs
""",
        external_functions=ext,
    )
    assert result == snapshot(['Hello', 'World'])


async def test_repl_llm_syntax_error_then_fix():
    """LLM writes code with a syntax error, then fixes it in the next snippet."""
    repl = pydantic_monty.MontyRepl()

    def add(a: int, b: int) -> int:
        return a + b

    ext = {'add': add}

    # Snippet 1: set up state
    await repl.feed_run_async('x = 10', external_functions=ext)

    # Snippet 2: syntax error
    with pytest.raises(pydantic_monty.MontySyntaxError):
        await repl.feed_run_async('y = add(x,', external_functions=ext)

    # Snippet 3: state preserved, LLM fixes the code
    result = await repl.feed_run_async('y = add(x, 5)\ny', external_functions=ext)
    assert result == snapshot(15)


# === Tests for run_async with resource limits ===


async def test_run_monty_async_with_limits():
    """run_async works with resource limits."""
    m = pydantic_monty.Monty('x + 1', inputs=['x'])
    result = await m.run_async(inputs={'x': 41}, limits={'max_duration_secs': 5.0})
    assert result == snapshot(42)


async def test_run_monty_async_limits_exceeded():
    """run_async propagates resource limit errors through spawn_blocking."""
    code = """\
result = []
for i in range(10000):
    result.append([i])
len(result)
"""
    m = pydantic_monty.Monty(code)

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        await m.run_async(limits={'max_allocations': 5})
    assert isinstance(exc_info.value.exception(), MemoryError)


async def test_run_monty_async_cancel_stops_vm_execution():
    """Cancelling run_async stops active Monty execution rather than waiting for a timeout."""
    code = """\
while True:
    pass
"""
    m = pydantic_monty.Monty(code)

    async def run_code():
        await m.run_async(limits={'max_duration_secs': 0.5})

    task = asyncio.create_task(run_code())
    await asyncio.sleep(0.01)
    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, 1.0)


# === Tests for concurrent REPL access ===


async def test_run_repl_async_concurrent_raises():
    """Two concurrent feed_run_async on the same REPL raises an error."""
    repl = pydantic_monty.MontyRepl()

    async def slow_func():
        await asyncio.sleep(0.1)
        return 42

    ext = {'slow_func': slow_func}

    # Wrap in a coroutine so asyncio.create_task works with the pyo3 Future
    async def run_first():
        return await repl.feed_run_async('await slow_func()', external_functions=ext)

    # Start first call (don't await yet)
    task1 = asyncio.create_task(run_first())

    # Give task1 a moment to start and take the REPL
    await asyncio.sleep(0.01)

    # Second call should fail because REPL is taken
    with pytest.raises(RuntimeError, match='currently executing'):
        await repl.feed_run_async('1 + 1')

    # First call should complete successfully
    result = await task1
    assert result == snapshot(42)


async def test_run_repl_async_discarded_awaitable_does_not_take_repl():
    """A returned awaitable does not steal REPL ownership until it is actually awaited."""
    repl = pydantic_monty.MontyRepl()

    async def slow_func():
        await asyncio.sleep(0.1)
        return 42

    pending = repl.feed_run_async('await slow_func()', external_functions={'slow_func': slow_func})

    result = await repl.feed_run_async('1 + 1')
    assert result == snapshot(2)
    del pending


async def test_run_repl_async_cancel_restores_repl():
    """Cancelling an in-flight async REPL call restores the REPL state."""
    repl = pydantic_monty.MontyRepl(limits={'max_duration_secs': 0.5})
    await repl.feed_run_async('x = 100')

    started = asyncio.Event()
    release = asyncio.Event()

    async def slow_func():
        started.set()
        await release.wait()
        return 42

    task = asyncio.ensure_future(repl.feed_run_async('await slow_func()', external_functions={'slow_func': slow_func}))
    await started.wait()
    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await task

    result = await repl.feed_run_async('x')
    assert result == snapshot(100)


async def test_run_repl_async_cancel_stops_vm_execution():
    """Cancelling a CPU-bound REPL snippet stops execution and restores the REPL."""
    repl = pydantic_monty.MontyRepl(limits={'max_duration_secs': 0.5})
    await repl.feed_run_async('x = 100')

    async def run_code():
        await repl.feed_run_async(
            """\
while True:
    pass
"""
        )

    task = asyncio.create_task(run_code())
    await asyncio.sleep(0.01)
    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, 1.0)

    result = await repl.feed_run_async('x')
    assert result == snapshot(100)


# === Tests for async error + REPL restoration ===


async def test_run_repl_async_error_restores_repl_on_async_failure():
    """REPL state is preserved when an async coroutine raises an exception."""
    repl = pydantic_monty.MontyRepl()
    await repl.feed_run_async('x = 100')

    async def failing_async():
        await asyncio.sleep(0.001)
        raise RuntimeError('async kaboom')

    with pytest.raises(pydantic_monty.MontyRuntimeError):
        await repl.feed_run_async('await failing_async()', external_functions={'failing_async': failing_async})

    # REPL should still be usable and state should be preserved
    result = await repl.feed_run_async('x')
    assert result == snapshot(100)


# === Tests for os callable validation ===


async def test_run_monty_async_os_not_callable():
    """run_async raises TypeError when os is not callable."""
    m = pydantic_monty.Monty('1 + 1')

    with pytest.raises(TypeError, match='not callable'):
        await m.run_async(os='not a callable')  # pyright: ignore[reportArgumentType]


async def test_run_repl_async_os_not_callable():
    """feed_run_async raises TypeError when os is not callable."""
    repl = pydantic_monty.MontyRepl()

    with pytest.raises(TypeError, match='not callable'):
        await repl.feed_run_async('1 + 1', os=42)  # pyright: ignore[reportArgumentType]
