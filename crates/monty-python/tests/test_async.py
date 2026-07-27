"""Async tests: sandbox asyncio code via `AsyncMonty` sessions with `feed_run`.

External functions may be coroutines; sandbox `asyncio` code (gather etc.) runs
unchanged inside the worker.
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator

import pytest
from inline_snapshot import snapshot

from pydantic_monty import (
    AsyncMonty,
    AsyncMontySession,
    MemoryFile,
    MontyRuntimeError,
    MontySyntaxError,
    OSAccess,
)


@pytest.fixture
async def apool() -> AsyncIterator[AsyncMonty]:
    """A fresh async worker pool for one test."""
    async with AsyncMonty() as pool:
        yield pool


@pytest.fixture
async def asession(apool: AsyncMonty) -> AsyncIterator[AsyncMontySession]:
    """A fresh checked-out async session for one test."""
    async with apool.checkout() as session:
        yield session


# === Basic async external function dispatch ===


async def test_await_external_function(asession: AsyncMontySession):
    async def foobar(a: int, b: int) -> int:
        return a + b

    result = await asession.feed_run('await foobar(1, 2)', external_lookup={'foobar': foobar})
    assert result == snapshot(3)


async def test_external_lookup_value(asession: AsyncMontySession):
    """A non-callable external_lookup entry resolves the bare name to its value,
    alongside a callable entry resolved as a function proxy."""

    def double(x: int) -> int:
        return x * 2

    result = await asession.feed_run('double(n)', external_lookup={'double': double, 'n': 21})
    assert result == snapshot(42)


async def test_asyncio_gather(asession: AsyncMontySession):
    code = """
import asyncio

await asyncio.gather(foo(1), bar(2))
"""

    async def foo(x: int) -> int:
        return x + 2

    async def bar(x: int) -> int:
        return x + 2

    result = await asession.feed_run(code, external_lookup={'foo': foo, 'bar': bar})
    assert result == snapshot([3, 4])


async def test_sync_function(asession: AsyncMontySession):
    """feed_run with a basic sync external function."""

    def get_value():
        return 42

    result = await asession.feed_run('get_value()', external_lookup={'get_value': get_value})
    assert result == snapshot(42)


async def test_async_function(asession: AsyncMontySession):
    """feed_run with a basic async external function."""

    async def fetch_data():
        await asyncio.sleep(0.001)
        return 'async result'

    result = await asession.feed_run('await fetch_data()', external_lookup={'fetch_data': fetch_data})
    assert result == snapshot('async result')


async def test_function_not_found(asession: AsyncMontySession):
    """Missing external function raises wrapped NameError."""
    with pytest.raises(MontyRuntimeError) as exc_info:
        await asession.feed_run('missing_func()', external_lookup={})
    inner = exc_info.value.exception()
    assert isinstance(inner, NameError)
    assert inner.args[0] == snapshot("name 'missing_func' is not defined")


async def test_sync_exception(asession: AsyncMontySession):
    """Sync function exceptions propagate correctly."""

    def fail():
        raise ValueError('sync error')

    with pytest.raises(MontyRuntimeError) as exc_info:
        await asession.feed_run('fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('sync error')


async def test_async_exception(asession: AsyncMontySession):
    """Async function exceptions propagate correctly."""

    async def async_fail():
        await asyncio.sleep(0.001)
        raise RuntimeError('async error')

    with pytest.raises(MontyRuntimeError) as exc_info:
        await asession.feed_run('await async_fail()', external_lookup={'async_fail': async_fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert inner.args[0] == snapshot('async error')


async def test_exception_caught(asession: AsyncMontySession):
    """Exceptions caught in try/except don't propagate."""
    code = """
try:
    fail()
except ValueError:
    caught = True
caught
"""

    def fail():
        raise ValueError('caught error')

    result = await asession.feed_run(code, external_lookup={'fail': fail})
    assert result == snapshot(True)


async def test_multiple_async_functions(asession: AsyncMontySession):
    """asyncio.gather with multiple async functions."""
    code = """
import asyncio
await asyncio.gather(fetch_a(), fetch_b())
"""

    async def fetch_a():
        await asyncio.sleep(0.01)
        return 'a'

    async def fetch_b():
        await asyncio.sleep(0.005)
        return 'b'

    result = await asession.feed_run(code, external_lookup={'fetch_a': fetch_a, 'fetch_b': fetch_b})
    assert result == snapshot(['a', 'b'])


async def test_mixed_sync_async(asession: AsyncMontySession):
    """Mix of sync and async external functions."""
    code = """
sync_val = sync_func()
async_val = await async_func()
sync_val + async_val
"""

    def sync_func():
        return 10

    async def async_func():
        await asyncio.sleep(0.001)
        return 5

    result = await asession.feed_run(code, external_lookup={'sync_func': sync_func, 'async_func': async_func})
    assert result == snapshot(15)


async def test_with_inputs(asession: AsyncMontySession):
    """feed_run with inputs parameter."""

    def process(a: int, b: int) -> int:
        return a * b

    result = await asession.feed_run('process(x, y)', inputs={'x': 6, 'y': 7}, external_lookup={'process': process})
    assert result == snapshot(42)


async def test_with_print_callback(asession: AsyncMontySession):
    """feed_run with print_callback parameter."""
    output: list[tuple[str, str]] = []

    def callback(stream: str, text: str) -> None:
        output.append((stream, text))

    result = await asession.feed_run('print("hello from async")', print_callback=callback)
    assert result is None
    assert output == snapshot([('stdout', 'hello from async\n')])


async def test_function_returning_none(asession: AsyncMontySession):
    def do_nothing():
        return None

    result = await asession.feed_run('do_nothing()', external_lookup={'do_nothing': do_nothing})
    assert result is None


async def test_no_external_calls(asession: AsyncMontySession):
    result = await asession.feed_run('1 + 2 + 3')
    assert result == snapshot(6)


# === os parameter ===


async def test_with_os(asession: AsyncMontySession):
    """feed_run can use OSAccess for file operations."""
    fs = OSAccess([MemoryFile('/test.txt', content='hello world')])
    code = """
from pathlib import Path
Path('/test.txt').read_text()
"""
    result = await asession.feed_run(code, os=fs)
    assert result == snapshot('hello world')


async def test_os_with_external_lookup(asession: AsyncMontySession):
    """feed_run can combine OSAccess with external functions."""
    fs = OSAccess([MemoryFile('/data.txt', content='test data')])

    async def process(text: str) -> str:
        return text.upper()

    code = """
from pathlib import Path
content = Path('/data.txt').read_text()
await process(content)
"""
    result = await asession.feed_run(code, external_lookup={'process': process}, os=fs)
    assert result == snapshot('TEST DATA')


async def test_os_file_not_found(asession: AsyncMontySession):
    """feed_run propagates OS errors correctly."""
    code = """
from pathlib import Path
Path('/missing.txt').read_text()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        await asession.feed_run(code, os=OSAccess())
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing.txt'")


async def test_os_not_provided(asession: AsyncMontySession):
    """OS calls without an os handler hit the default-unhandled PermissionError."""
    code = """
from pathlib import Path
Path('/test.txt').exists()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        await asession.feed_run(code)
    inner = exc_info.value.exception()
    assert isinstance(inner, PermissionError)
    assert str(inner) == snapshot("Permission denied: '/test.txt'")


async def test_os_write_and_read(asession: AsyncMontySession):
    """feed_run supports both reading and writing files."""
    fs = OSAccess([MemoryFile('/file.txt', content='original')])
    code = """
from pathlib import Path
p = Path('/file.txt')
p.write_text('updated')
p.read_text()
"""
    result = await asession.feed_run(code, os=fs)
    assert result == snapshot('updated')


async def test_nested_gather_with_external_lookup(asession: AsyncMontySession):
    """Nested asyncio.gather with spawned tasks and external async functions.

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

    result = await asession.feed_run(
        code,
        external_lookup={
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


# === State persistence across async feeds ===


async def test_state_persists(asession: AsyncMontySession):
    """Session state persists across multiple feed_run calls."""

    def double(x: int) -> int:
        return x * 2

    ext = {'double': double}
    await asession.feed_run('x = 10', external_lookup=ext)
    await asession.feed_run('y = double(x)', external_lookup=ext)
    result = await asession.feed_run('y', external_lookup=ext)
    assert result == snapshot(20)


async def test_async_state_persists(asession: AsyncMontySession):
    """Session state persists across async calls with await."""

    async def fetch(key: str) -> str:
        return f'value_{key}'

    ext = {'fetch': fetch}
    await asession.feed_run("a = await fetch('one')", external_lookup=ext)
    await asession.feed_run("b = await fetch('two')", external_lookup=ext)
    result = await asession.feed_run('a + b', external_lookup=ext)
    assert result == snapshot('value_onevalue_two')


async def test_error_preserves_state(asession: AsyncMontySession):
    """Session state is preserved after an error in feed_run."""
    await asession.feed_run('x = 42')

    def fail():
        raise ValueError('oops')

    with pytest.raises(MontyRuntimeError):
        await asession.feed_run('fail()', external_lookup={'fail': fail})

    result = await asession.feed_run('x')
    assert result == snapshot(42)


async def test_async_error_preserves_state(asession: AsyncMontySession):
    """Session state is preserved when an async coroutine raises an exception."""
    await asession.feed_run('x = 100')

    async def failing_async():
        await asyncio.sleep(0.001)
        raise RuntimeError('async kaboom')

    with pytest.raises(MontyRuntimeError):
        await asession.feed_run('await failing_async()', external_lookup={'failing_async': failing_async})

    result = await asession.feed_run('x')
    assert result == snapshot(100)


# === Resource limits in async sessions ===


async def test_limits_exceeded(apool: AsyncMonty):
    """Resource limit errors surface as MontyRuntimeError from feed_run."""
    code = """\
result = []
for i in range(10000):
    result.append([i])
len(result)
"""
    async with apool.checkout(limits={'max_memory': 500}) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            await session.feed_run(code)
        assert isinstance(exc_info.value.exception(), MemoryError)


# === Lone surrogate strings crossing the boundary ===


async def test_sync_external_return_lone_surrogate(asession: AsyncMontySession):
    """A sync callback returning a lone-surrogate string surfaces inside Monty
    as a catchable ValueError."""
    code = """
try:
    get_str()
    result = 'no error'
except ValueError:
    result = 'caught'
result
"""
    result = await asession.feed_run(code, external_lookup={'get_str': lambda: '\ud83d'})
    assert result == snapshot('caught')


async def test_async_external_return_lone_surrogate(asession: AsyncMontySession):
    """An async callback returning a lone-surrogate string surfaces as
    `MontyRuntimeError(ValueError)` rather than a raw `UnicodeEncodeError`."""

    async def get_str() -> str:
        return '\ud83d'

    with pytest.raises(MontyRuntimeError) as exc_info:
        await asession.feed_run('await get_str()', external_lookup={'get_str': get_str})
    assert isinstance(exc_info.value.exception(), ValueError)


# === LLM agent patterns: realistic multi-feed scenarios ===


async def test_llm_iterative_data_collection(asession: AsyncMontySession):
    """LLM collects data in batches, accumulating results across snippets."""
    responses: dict[int, list[dict[str, object]]] = {
        0: [{'id': 1, 'name': 'Alice'}, {'id': 2, 'name': 'Bob'}],
        2: [{'id': 3, 'name': 'Charlie'}],
        3: [],
    }

    async def fetch_users(offset: int, limit: int) -> list[dict[str, object]]:
        return responses.get(offset, [])

    ext = {'fetch_users': fetch_users}

    # Snippet 1: LLM sets up accumulator
    await asession.feed_run('all_users = []', external_lookup=ext)

    # Snippets 2-4: LLM fetches batches until one comes back empty
    batch_code = """\
batch = await fetch_users(len(all_users), 2)
all_users = all_users + batch
len(batch)
"""
    assert await asession.feed_run(batch_code, external_lookup=ext) == 2
    assert await asession.feed_run(batch_code, external_lookup=ext) == 1
    assert await asession.feed_run(batch_code, external_lookup=ext) == 0

    # Snippet 5: LLM extracts final result
    result = await asession.feed_run('[u["name"] for u in all_users]', external_lookup=ext)
    assert result == snapshot(['Alice', 'Bob', 'Charlie'])


async def test_llm_error_recovery_retry(asession: AsyncMontySession):
    """LLM catches an error, adjusts approach, retries successfully."""
    call_count = 0

    async def flaky_api(query: str) -> str:
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            raise ConnectionError('server unavailable')
        return f'result for {query}'

    ext = {'flaky_api': flaky_api}

    # Snippet 1: LLM tries, gets error
    with pytest.raises(MontyRuntimeError):
        await asession.feed_run("data = await flaky_api('test')", external_lookup=ext)

    # Snippet 2: LLM wraps in try/except and retries
    result = await asession.feed_run(
        """\
try:
    data = await flaky_api('test')
except Exception as e:
    data = 'fallback'
data
""",
        external_lookup=ext,
    )
    assert result == snapshot('result for test')


async def test_llm_redefine_helper_function(asession: AsyncMontySession):
    """LLM defines a function, uses it, then redefines it with improvements."""

    async def fetch(url: str) -> str:
        return f'<html>{url}</html>'

    ext = {'fetch': fetch}

    # Snippet 1: LLM defines initial parser
    await asession.feed_run(
        """\
def parse_title(html):
    return html
""",
        external_lookup=ext,
    )

    # Snippet 2: LLM uses it, gets raw html back
    result = await asession.feed_run(
        """\
html = await fetch('example.com')
parse_title(html)
""",
        external_lookup=ext,
    )
    assert result == snapshot('<html>example.com</html>')

    # Snippet 3: LLM redefines parser with better logic
    await asession.feed_run(
        """\
def parse_title(html):
    start = html.find('>') + 1
    end = html.rfind('<')
    return html[start:end]
""",
        external_lookup=ext,
    )

    # Snippet 4: uses improved parser on previously fetched data
    result = await asession.feed_run('parse_title(html)', external_lookup=ext)
    assert result == snapshot('example.com')


async def test_llm_sequential_async_pipeline(asession: AsyncMontySession):
    """LLM builds a data pipeline: fetch -> transform -> store, each step depends on previous."""

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
    result = await asession.feed_run(code, external_lookup=ext)
    assert result == snapshot(['summary(python async_result_1)', 'summary(python async_result_2)'])
    assert records == snapshot(['summary(python async_result_1)', 'summary(python async_result_2)'])


async def test_llm_gather_fan_out(asession: AsyncMontySession):
    """LLM uses asyncio.gather to fan out many concurrent requests."""

    async def fetch_price(item: str) -> float:
        prices = {'apple': 1.5, 'banana': 0.75, 'cherry': 3.0, 'date': 5.0, 'elderberry': 8.0}
        return prices[item]

    code = """\
import asyncio

items = ['apple', 'banana', 'cherry', 'date', 'elderberry']
prices = await asyncio.gather(*(fetch_price(item) for item in items))
dict(zip(items, prices))
"""
    result = await asession.feed_run(code, external_lookup={'fetch_price': fetch_price})
    assert result == snapshot({'apple': 1.5, 'banana': 0.75, 'cherry': 3.0, 'date': 5.0, 'elderberry': 8.0})


async def test_llm_try_except_around_external(asession: AsyncMontySession):
    """LLM wraps individual external calls in try/except for graceful degradation."""

    def fetch_data(key: str) -> str:
        if key == 'bad':
            raise KeyError(f'no data for {key}')
        return f'data_{key}'

    code = """\
results = {}
for key in ['good', 'bad', 'also_good']:
    try:
        results[key] = fetch_data(key)
    except KeyError:
        results[key] = 'missing'
results
"""
    result = await asession.feed_run(code, external_lookup={'fetch_data': fetch_data})
    assert result == snapshot({'good': 'data_good', 'bad': 'missing', 'also_good': 'data_also_good'})


async def test_llm_conditional_external_call(asession: AsyncMontySession):
    """LLM only calls external function when a condition is met."""
    call_count = 0

    async def expensive_lookup(key: str) -> str:
        nonlocal call_count
        call_count += 1
        return f'looked up {key}'

    ext = {'expensive_lookup': expensive_lookup}

    # Snippet 1: set up a cache
    await asession.feed_run("cache = {'x': 'cached_x'}", external_lookup=ext)

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
    result = await asession.feed_run(code, external_lookup=ext)
    assert result == snapshot(['cached_x', 'looked up y', 'cached_x'])
    assert call_count == 1  # only 'y' triggered a call


async def test_llm_side_effect_recording(asession: AsyncMontySession):
    """LLM uses a side-effect-only external function to record structured data."""
    recorded: list[dict[str, object]] = []

    def record_model(name: str, params: str, price: float) -> None:
        recorded.append({'name': name, 'params': params, 'price': price})

    async def get_models() -> list[dict[str, str]]:
        return [
            {'name': 'gpt-4', 'params': '1.7T'},
            {'name': 'claude-3', 'params': '???'},
        ]

    code = """\
models = await get_models()
for m in models:
    record_model(m['name'], m['params'], 0.01)
len(models)
"""
    result = await asession.feed_run(code, external_lookup={'record_model': record_model, 'get_models': get_models})
    assert result == snapshot(2)
    assert recorded == snapshot(
        [{'name': 'gpt-4', 'params': '1.7T', 'price': 0.01}, {'name': 'claude-3', 'params': '???', 'price': 0.01}]
    )


async def test_llm_helper_wrapping_externals_with_retry(asession: AsyncMontySession):
    """LLM defines a helper function that wraps external calls with retry logic."""
    attempt_counts: dict[str, int] = {}

    def unreliable_fetch(url: str) -> str:
        attempt_counts.setdefault(url, 0)
        attempt_counts[url] += 1
        if attempt_counts[url] < 2:
            raise ValueError('temporary failure')
        return f'content of {url}'

    ext = {'unreliable_fetch': unreliable_fetch}

    # Snippet 1: LLM defines retry helper
    await asession.feed_run(
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
        external_lookup=ext,
    )

    # Snippet 2: LLM uses the retry helper
    result = await asession.feed_run("fetch_with_retry('example.com')", external_lookup=ext)
    assert result == snapshot('content of example.com')
    assert attempt_counts == snapshot({'example.com': 2})


async def test_llm_nested_gather_with_sequential_deps(asession: AsyncMontySession):
    """LLM does gather of tasks where each task has sequential async steps internally."""

    async def get_user(user_id: int) -> dict[str, object]:
        return {'id': user_id, 'name': f'user_{user_id}'}

    async def get_posts(user_id: int) -> list[str]:
        return [f'post_{user_id}_1', f'post_{user_id}_2']

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
    result = await asession.feed_run(code, external_lookup={'get_user': get_user, 'get_posts': get_posts})
    assert result == snapshot(
        [
            {'id': 1, 'name': 'user_1', 'posts': ['post_1_1', 'post_1_2']},
            {'id': 2, 'name': 'user_2', 'posts': ['post_2_1', 'post_2_2']},
            {'id': 3, 'name': 'user_3', 'posts': ['post_3_1', 'post_3_2']},
        ]
    )


async def test_llm_external_returns_complex_nested_structure(asession: AsyncMontySession):
    """LLM processes deeply nested API response from external function."""

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
    await asession.feed_run('response = await get_api_response()', external_lookup=ext)

    # Snippet 2: LLM navigates nested structure
    result = await asession.feed_run(
        """\
users = response['data']['users']
averages = {}
for u in users:
    avg = sum(u['scores']) / len(u['scores'])
    averages[u['name']] = round(avg, 1)
averages
""",
        external_lookup=ext,
    )
    assert result == snapshot({'Alice': 91.3, 'Bob': 84.3})


async def test_llm_external_with_kwargs(asession: AsyncMontySession):
    """LLM calls external functions using keyword arguments."""

    async def search(query: str, limit: int = 10, offset: int = 0) -> dict[str, object]:
        return {'query': query, 'limit': limit, 'offset': offset, 'results': [f'{query}_{i}' for i in range(limit)]}

    code = """\
page1 = await search('test', limit=2, offset=0)
page2 = await search('test', limit=2, offset=2)
page1['results'] + page2['results']
"""
    result = await asession.feed_run(code, external_lookup={'search': search})
    assert result == snapshot(['test_0', 'test_1', 'test_0', 'test_1'])


async def test_llm_os_read_then_process_with_external(asession: AsyncMontySession):
    """LLM reads a file via OS, then processes content with an async external function."""
    fs = OSAccess([MemoryFile('/data.csv', content='alice,95\nbob,87\ncharlie,92')])

    async def analyze(text: str) -> dict[str, int]:
        rows = text.strip().split('\n')
        return {name: int(score) for name, score in (r.split(',') for r in rows)}

    ext = {'analyze': analyze}

    # Snippet 1: read file
    await asession.feed_run(
        """\
from pathlib import Path
raw = Path('/data.csv').read_text()
""",
        external_lookup=ext,
        os=fs,
    )

    # Snippet 2: process with external
    result = await asession.feed_run('await analyze(raw)', external_lookup=ext, os=fs)
    assert result == snapshot({'alice': 95, 'bob': 87, 'charlie': 92})


async def test_llm_long_multi_step_session(asession: AsyncMontySession):
    """Simulates a multi-step LLM agent session: setup, explore, process, summarize."""
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
    result = await asession.feed_run('await query_db("products")', external_lookup=ext)
    assert len(result) == 4

    # Step 2: LLM filters by category
    await asession.feed_run(
        "tools = await query_db('products', filters={'category': 'tools'})",
        external_lookup=ext,
    )

    # Step 3: LLM computes stats
    result = await asession.feed_run(
        """\
total = sum(p['price'] for p in tools)
avg = total / len(tools)
{'count': len(tools), 'total': round(total, 2), 'average': round(avg, 2)}
""",
        external_lookup=ext,
    )
    assert result == snapshot({'count': 2, 'total': 14.98, 'average': 7.49})

    # Step 4: LLM also checks electronics
    await asession.feed_run(
        "electronics = await query_db('products', filters={'category': 'electronics'})",
        external_lookup=ext,
    )

    # Step 5: LLM builds final summary from accumulated state
    result = await asession.feed_run(
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
        external_lookup=ext,
    )
    assert result == snapshot(
        {
            'tools': {'count': 2, 'total': 14.98, 'items': ['Widget', 'Doohickey']},
            'electronics': {'count': 2, 'total': 74.98, 'items': ['Gadget', 'Thingamajig']},
        }
    )


async def test_llm_string_manipulation_of_external_result(asession: AsyncMontySession):
    """LLM fetches HTML-like content and does string processing across snippets."""

    async def fetch_page(url: str) -> str:
        return '<title>Test Page</title><body><p>Hello</p><p>World</p></body>'

    ext = {'fetch_page': fetch_page}

    await asession.feed_run("html = await fetch_page('example.com')", external_lookup=ext)

    # LLM extracts title
    result = await asession.feed_run(
        """\
start = html.find('<title>') + len('<title>')
end = html.find('</title>')
title = html[start:end]
title
""",
        external_lookup=ext,
    )
    assert result == snapshot('Test Page')

    # LLM extracts paragraphs
    result = await asession.feed_run(
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
        external_lookup=ext,
    )
    assert result == snapshot(['Hello', 'World'])


async def test_llm_syntax_error_then_fix(asession: AsyncMontySession):
    """LLM writes code with a syntax error, then fixes it in the next snippet."""

    def add(a: int, b: int) -> int:
        return a + b

    ext = {'add': add}

    # Snippet 1: set up state
    await asession.feed_run('x = 10', external_lookup=ext)

    # Snippet 2: syntax error
    with pytest.raises(MontySyntaxError):
        await asession.feed_run('y = add(x,', external_lookup=ext)

    # Snippet 3: state preserved, LLM fixes the code
    result = await asession.feed_run('y = add(x, 5)\ny', external_lookup=ext)
    assert result == snapshot(15)
