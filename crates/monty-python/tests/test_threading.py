"""True-parallelism tests: each thread checks out its own worker session, so
CPU-bound sandbox code runs concurrently across worker subprocesses."""

from __future__ import annotations

import os
import threading
import time
from typing import Any, Callable

import pytest
from inline_snapshot import snapshot

from pydantic_monty import Monty

# I don't see a way to run these tests reliably on CI since github actions only has one CPU
# perhaps we could use ubuntu-24.04-arm once the repo is open source (it's currently not supported for private repos)
# https://docs.github.com/en/actions/reference/runners/github-hosted-runners
pytestmark = pytest.mark.skipif('CI' in os.environ, reason='on CI')

THREADS = 4


def run_in_threads(run: Callable[[], Any]) -> tuple[list[Any], float]:
    """Run `run` in THREADS threads at once, returning the results and wall time."""
    results: list[Any] = []
    lock = threading.Lock()

    def worker() -> None:
        value = run()
        with lock:
            results.append(value)

    threads = [threading.Thread(target=worker) for _ in range(THREADS)]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return results, time.perf_counter() - start


def test_parallel_exec():
    """Run code in one session, run it in parallel sessions, check that parallel
    execution is not much slower (i.e. workers genuinely run concurrently)."""
    code = """
x = 0
for i in range(200_000):
    x += 1
x
"""
    # min_processes pre-spawns the workers so the parallel phase isn't
    # measuring process startup
    with Monty(min_processes=THREADS) as pool:

        def run() -> Any:
            with pool.checkout() as session:
                return session.feed_run(code)

        start = time.perf_counter()
        assert run() == 200_000
        diff = time.perf_counter() - start

        results, diff_parallel = run_in_threads(run)
        assert results == [200_000] * THREADS
        # check that running in parallel 4 times is less than 1.5x slower than running once
        time_multiple = diff_parallel / diff
        assert time_multiple < 1.5, 'Execution should not be slower in parallel'


def test_parallel_exec_print():
    """Parallel sessions each streaming print output back to their own callback."""
    code = """
x = 0
for i in range(200_000):
    x += 1
print(x)
"""
    with Monty(min_processes=THREADS) as pool:

        def run() -> list[str]:
            captured: list[str] = []

            def print_callback(file: str, content: str) -> None:
                captured.append(f'{file}: {content}')

            with pool.checkout() as session:
                assert session.feed_run(code, print_callback=print_callback) is None
            return captured

        start = time.perf_counter()
        assert run() == snapshot(['stdout: 200000\n'])
        diff = time.perf_counter() - start

        results, diff_parallel = run_in_threads(run)
        assert results == [['stdout: 200000\n']] * THREADS
        time_multiple = diff_parallel / diff
        assert time_multiple < 1.5, 'Execution should not be slower in parallel'


def double(a: int) -> int:
    return a * 2


def test_parallel_exec_ext_functions():
    """Parallel sessions each calling back into a host external function."""
    code = """
x = 0
for i in range(100_000):
    x += 1
x = double(x)
for i in range(100_000):
    x += 1
x
"""
    with Monty(min_processes=THREADS) as pool:

        def run() -> Any:
            with pool.checkout() as session:
                return session.feed_run(code, external_lookup={'double': double})

        start = time.perf_counter()
        assert run() == 300_000
        diff = time.perf_counter() - start

        results, diff_parallel = run_in_threads(run)
        assert results == [300_000] * THREADS
        time_multiple = diff_parallel / diff
        assert time_multiple < 1.5, 'Execution should not be slower in parallel'


def test_parallel_sessions_state_is_isolated():
    """Concurrent sessions never see each other's globals."""
    with Monty(min_processes=THREADS) as pool:

        def run() -> Any:
            with pool.checkout() as session:
                session.feed_run('values = []')
                for i in range(5):
                    session.feed_run('values.append(n)', inputs={'n': i})
                return session.feed_run('sum(values)')

        results, _ = run_in_threads(run)
        assert results == [10] * THREADS
