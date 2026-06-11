"""Tests for `MontyPool` — crash-isolated execution in subprocess workers."""

import asyncio
import os
import signal
import sys
import threading
from pathlib import Path
from typing import Any

import pytest
from inline_snapshot import snapshot

from pydantic_monty import (
    CollectStreams,
    MontyCrashedError,
    MontyPool,
    MontyRuntimeError,
    MontyTypingError,
    MountDir,
)


async def test_basic_execution():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            assert await session.feed_run_async('1 + 2') == snapshot(3)


async def test_session_state_persists_across_feeds():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            await session.feed_run_async('x = 40')
            assert await session.feed_run_async('x + 2') == snapshot(42)


async def test_sessions_are_isolated():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            await session.feed_run_async('leaky = 1')
        # a new session reuses the worker process but never its state
        async with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run_async('leaky')
            assert exc_info.value.display(format='msg') == snapshot("name 'leaky' is not defined")


async def test_inputs():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            assert await session.feed_run_async('a * b', inputs={'a': 6, 'b': 7}) == snapshot(42)


async def test_external_functions_sync():
    calls: list[tuple[int, int]] = []

    def add(a: int, b: int) -> int:
        calls.append((a, b))
        return a + b

    async with MontyPool() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run_async('add(1, 2) + add(3, 4)', external_functions={'add': add})
    assert result == snapshot(10)
    assert calls == snapshot([(1, 2), (3, 4)])


async def test_external_functions_async():
    async def fetch(url: str) -> str:
        await asyncio.sleep(0.001)
        return url.upper()

    async with MontyPool() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run_async("await fetch('hello')", external_functions={'fetch': fetch})
    assert result == snapshot('HELLO')


async def test_external_function_exception_round_trip():
    def boom() -> None:
        raise ValueError('intentional')

    async with MontyPool() as pool:
        async with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run_async('boom()', external_functions={'boom': boom})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('intentional')


async def test_print_callback_streams_output():
    collector = CollectStreams()
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            await session.feed_run_async("print('one')\nprint('two')", print_callback=collector)
    # one tuple per streamed protocol frame: output is line-buffered in the worker
    assert collector.output == snapshot([('stdout', 'one\n'), ('stdout', 'two\n')])


async def test_runtime_error_preserves_session():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            await session.feed_run_async('kept = 41')
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run_async('1 / 0')
            assert exc_info.value.display(format='msg') == snapshot('division by zero')
            # the session (and its globals) survives the error
            assert await session.feed_run_async('kept + 1') == snapshot(42)


async def test_runtime_error_parity_with_in_process_repl():
    code = 'def boom():\n    raise ValueError("oops")\n\nboom()'
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError) as pool_exc:
                await session.feed_run_async(code)

    import pydantic_monty

    repl = pydantic_monty.MontyRepl()
    with pytest.raises(MontyRuntimeError) as local_exc:
        repl.feed_run(code)
    # the full rendered traceback must match the in-process REPL byte for byte
    assert pool_exc.value.display() == local_exc.value.display()


async def test_typing_error():
    async with MontyPool() as pool:
        async with pool.checkout(type_check=True) as session:
            with pytest.raises(MontyTypingError) as exc_info:
                await session.feed_run_async("x: int = 'nope'")
            assert 'invalid-assignment' in exc_info.value.display()
            # the session survives a typing rejection
            assert await session.feed_run_async('1 + 1') == snapshot(2)


async def test_limits_enforced_in_worker():
    async with MontyPool() as pool:
        async with pool.checkout(limits={'max_duration_secs': 0.1}) as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run_async('while True:\n    pass')
            assert exc_info.value.display(format='type-msg').startswith('TimeoutError')


async def test_mounts_are_worker_local(tmp_path: Path):
    (tmp_path / 'data.txt').write_text('mounted!')
    mount = MountDir('/mnt', tmp_path, mode='read-only')
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run_async(
                "from pathlib import Path\nPath('/mnt/data.txt').read_text()", mount=mount
            )
    assert result == snapshot('mounted!')


async def test_os_callback_fallback():
    def os_callback(name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> str:
        assert name == snapshot('os.getenv')
        return 'value-from-host'

    async with MontyPool() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run_async(
                "import os\nos.getenv('KEY')",
                os=os_callback,
            )
    assert result == snapshot('value-from-host')


async def test_unhandled_os_call_raises_inside_sandbox():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run_async("from pathlib import Path\nPath('/nope.txt').read_text()")
            assert exc_info.value.display(format='type-msg') == snapshot(
                "PermissionError: Permission denied: '/nope.txt'"
            )


async def test_worker_crash_raises_crashed_error_and_pool_recovers():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            pid = session.worker_pid
            assert pid is not None

            async def kill_soon() -> None:
                await asyncio.sleep(0.2)
                os.kill(pid, signal.SIGKILL if sys.platform != 'win32' else signal.SIGTERM)

            kill_task = asyncio.create_task(kill_soon())
            with pytest.raises(MontyCrashedError) as exc_info:
                await session.feed_run_async('while True:\n    pass')
            await kill_task
            assert exc_info.value.timed_out is False

        # the pool replaces the dead worker transparently
        async with pool.checkout() as session:
            assert await session.feed_run_async('1 + 1') == snapshot(2)


async def test_request_timeout_kills_hung_worker():
    async with MontyPool(request_timeout=0.3) as pool:
        async with pool.checkout() as session:
            with pytest.raises(MontyCrashedError) as exc_info:
                await session.feed_run_async('while True:\n    pass')
            assert exc_info.value.timed_out is True
        async with pool.checkout() as session:
            assert await session.feed_run_async('2 + 2') == snapshot(4)


async def test_concurrent_sessions():
    async with MontyPool(min_processes=2) as pool:

        async def run(value: int) -> object:
            async with pool.checkout() as session:
                return await session.feed_run_async('v * 2', inputs={'v': value})

        results = await asyncio.gather(run(1), run(2), run(3))
    assert results == snapshot([2, 4, 6])


async def test_dump_returns_bytes():
    async with MontyPool() as pool:
        async with pool.checkout() as session:
            await session.feed_run_async('x = 1')
            state = session.dump()
            assert isinstance(state, bytes)
            assert len(state) > 0


async def test_pool_not_entered():
    pool = MontyPool()
    session = pool.checkout()
    with pytest.raises(RuntimeError) as exc_info:
        await session.__aenter__()
    assert exc_info.value.args[0] == snapshot('MontyPool is not active — use `async with MontyPool(...)`')


def test_feed_run_sync_from_thread():
    """The sync `feed_run` works without an event loop (e.g. worker threads)."""

    async def main() -> None:
        async with MontyPool() as pool:
            async with pool.checkout() as session:
                result: list[object] = []

                def worker() -> None:
                    result.append(session.feed_run('21 * 2'))

                thread = threading.Thread(target=worker)
                thread.start()
                await asyncio.to_thread(thread.join)
                assert result == [42]

    asyncio.run(main())
