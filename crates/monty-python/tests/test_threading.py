import os
import threading
import time
from functools import partial
from typing import cast

import pytest
from inline_snapshot import snapshot

import pydantic_monty

# I don't see a way to run these tests reliably on CI since github actions only has one CPU
# perhaps we could use ubuntu-24.04-arm once the repo is open source (it's currently not supported for private repos)
# https://docs.github.com/en/actions/reference/runners/github-hosted-runners
pytestmark = pytest.mark.skipif('CI' in os.environ, reason='on CI')


def test_parallel_exec():
    """Run code directly, run it in parallel, check that parallel execution not much slower."""
    code = """
x = 0
for i in range(200_000):
    x += 1
x
"""
    m = pydantic_monty.Monty(code)
    start = time.perf_counter()
    result = m.run()
    diff = time.perf_counter() - start
    assert result == 200_000

    threads = [threading.Thread(target=m.run) for _ in range(4)]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    diff_parallel = time.perf_counter() - start
    # check that running the function in parallel 4 times is less than 1.5x slower than running it once
    time_multiple = diff_parallel / diff
    assert time_multiple < 1.5, 'Execution should not be slower in parallel'


def test_parallel_exec_print():
    """Run code directly, run it in parallel, check that parallel execution not much slower."""
    code = """
x = 0
for i in range(200_000):
    x += 1
print(x)
"""
    captured: list[str] = []

    def print_callback(file: str, content: str):
        captured.append(f'{file}: {content}')

    m = pydantic_monty.Monty(code)
    start = time.perf_counter()
    result = m.run(print_callback=print_callback)
    diff = time.perf_counter() - start
    assert result is None
    assert captured == snapshot(['stdout: 200000', 'stdout: \n'])

    threads = [threading.Thread(target=partial(m.run, print_callback=print_callback)) for _ in range(4)]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    diff_parallel = time.perf_counter() - start
    # check that running the function in parallel 4 times is less than 1.5x slower than running it once
    time_multiple = diff_parallel / diff
    assert time_multiple < 1.5, 'Execution should not be slower in parallel'


def double(a: int) -> int:
    return a * 2


def test_parallel_exec_ext_functions():
    """Run code directly, run it in parallel, check that parallel execution not much slower."""
    code = """
x = 0
for i in range(100_000):
    x += 1
x = double(x)
for i in range(100_000):
    x += 1
x
"""
    m = pydantic_monty.Monty(code, external_functions=['double'])
    start = time.perf_counter()
    result = m.run(external_functions={'double': double})
    diff = time.perf_counter() - start
    assert result == 300_000

    threads = [threading.Thread(target=partial(m.run, external_functions={'double': double})) for _ in range(4)]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    diff_parallel = time.perf_counter() - start
    # check that running the function in parallel 4 times is less than 1.5x slower than running it once
    time_multiple = diff_parallel / diff
    assert time_multiple < 1.5, 'Execution should not be slower in parallel'


def test_parallel_exec_start():
    """Run code directly, run it in parallel, check that parallel execution not much slower."""
    code = """
x = 0
for i in range(200_000):
    x += 1
double(x)
"""
    m = pydantic_monty.Monty(code, external_functions=['double'])
    start = time.perf_counter()
    progress = m.start()
    diff = time.perf_counter() - start
    assert isinstance(progress, pydantic_monty.MontySnapshot)

    threads = [threading.Thread(target=m.start) for _ in range(4)]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    diff_parallel = time.perf_counter() - start
    # check that running the function in parallel 4 times is less than 1.5x slower than running it once
    time_multiple = diff_parallel / diff
    assert time_multiple < 1.5, 'Execution should not be slower in parallel'


def test_parallel_exec_start_resume():
    """Run code directly, run it in parallel, check that parallel execution not much slower."""
    code = """
x = double(1)
for i in range(200_000):
    x += 1
x
"""
    m = pydantic_monty.Monty(code, external_functions=['double'])
    progress = m.start()
    assert isinstance(progress, pydantic_monty.MontySnapshot)
    start = time.perf_counter()
    result = progress.resume(return_value=2)
    diff = time.perf_counter() - start
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == 200_002

    progresses = cast(list[pydantic_monty.MontySnapshot], [m.start() for _ in range(4)])

    threads = [threading.Thread(target=partial(p.resume, return_value=2)) for p in progresses]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    diff_parallel = time.perf_counter() - start
    # check that running the function in parallel 4 times is less than 1.5x slower than running it once
    time_multiple = diff_parallel / diff
    assert time_multiple < 1.5, 'Execution should not be slower in parallel'
