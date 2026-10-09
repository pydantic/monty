from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Callable

import pytest
from inline_snapshot import snapshot

from pydantic_monty import AsyncMonty, AsyncMontyWebsocket, Monty


def run_isolated(code: str, *, tokio_worker_threads: str | None = None) -> str:
    """Runs `code` in a fresh interpreter, since the Tokio runtime is process-wide."""
    env = {k: v for k, v in os.environ.items() if k != 'TOKIO_WORKER_THREADS'}
    if tokio_worker_threads is not None:
        env['TOKIO_WORKER_THREADS'] = tokio_worker_threads
    result = subprocess.run([sys.executable, '-c', code], env=env, capture_output=True, text=True, check=True)
    return result.stdout


POOL_FACTORIES: list[Callable[[int], object]] = [
    lambda threads: Monty(runtime_threads=threads),
    lambda threads: AsyncMonty(runtime_threads=threads),
    lambda threads: AsyncMontyWebsocket('ws://127.0.0.1:1', runtime_threads=threads),
]


@pytest.mark.parametrize('threads', [0, -1])
@pytest.mark.parametrize('make_pool', POOL_FACTORIES, ids=['Monty', 'AsyncMonty', 'AsyncMontyWebsocket'])
def test_runtime_threads_not_positive(make_pool: Callable[[int], object], threads: int):
    with pytest.raises(ValueError) as exc_info:
        make_pool(threads)
    assert exc_info.value.args[0] == snapshot('runtime_threads must be at least 1')


def test_runtime_threads_explicit_shared():
    stdout = run_isolated("""
import asyncio
from pydantic_monty import AsyncMonty, AsyncMontyWebsocket, Monty

with Monty(runtime_threads=2) as pool, pool.checkout() as session:
    print(session.feed_run('1 + 1'))

async def main():
    async with AsyncMonty(runtime_threads=2) as pool, pool.checkout() as session:
        print(await session.feed_run('2 + 2'))

asyncio.run(main())
AsyncMontyWebsocket('ws://127.0.0.1:1', runtime_threads=2)

with Monty() as pool, pool.checkout() as session:
    print(session.feed_run('3 + 3'))

for cls in (Monty, AsyncMonty):
    try:
        cls(runtime_threads=3)
    except RuntimeError as exc:
        print(exc)
""")
    assert stdout == snapshot("""\
2
4
6
cannot use runtime_threads=3: the shared Tokio runtime was already initialized with 2 worker threads
cannot use runtime_threads=3: the shared Tokio runtime was already initialized with 2 worker threads
""")


def test_runtime_threads_after_default_runtime():
    stdout = run_isolated(
        """
from pydantic_monty import Monty

with Monty() as pool, pool.checkout() as session:
    print(session.feed_run('1 + 1'))

with Monty(runtime_threads=3) as pool, pool.checkout() as session:
    print(session.feed_run('2 + 2'))

try:
    Monty(runtime_threads=2)
except RuntimeError as exc:
    print(exc)
""",
        tokio_worker_threads='3',
    )
    assert stdout == snapshot("""\
2
4
cannot use runtime_threads=2: the shared Tokio runtime was already initialized with 3 worker threads
""")


def test_runtime_threads_after_async_default_runtime():
    stdout = run_isolated(
        """
import asyncio
from pydantic_monty import AsyncMonty, AsyncMontyWebsocket

async def main():
    async with AsyncMonty() as pool, pool.checkout() as session:
        print(await session.feed_run('1 + 1'))
    for make_pool in (lambda: AsyncMonty(runtime_threads=2), lambda: AsyncMontyWebsocket('ws://127.0.0.1:1', runtime_threads=2)):
        try:
            make_pool()
        except RuntimeError as exc:
            print(exc)
    AsyncMonty(runtime_threads=3)

asyncio.run(main())
""",
        tokio_worker_threads='3',
    )
    assert stdout == snapshot("""\
2
cannot use runtime_threads=2: the shared Tokio runtime was already initialized with 3 worker threads
cannot use runtime_threads=2: the shared Tokio runtime was already initialized with 3 worker threads
""")
