import threading
import time
from functools import partial

import monty


def test_parallel_exec():
    """Run code directly, run it in parallel, check that parallel execution not much slower."""
    code = """
x = 0
for i in range(200_000):
    x += 1
x
"""
    m = monty.Monty(code)
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
    slowdown = diff_parallel / diff
    assert slowdown < 1.5, 'Execution should not be slower in parallel'


def double(a: int) -> int:
    return a * 2


def test_parallel_exec_ext_functions():
    """Run code directly, run it in parallel, check that parallel execution not much slower."""
    code = """
x = 0
for i in range(200_000):
    x += 1
double(x)
"""
    m = monty.Monty(code, external_functions=['double'])
    start = time.perf_counter()
    result = m.run(external_functions={'double': double})
    diff = time.perf_counter() - start
    assert result == 400_000

    threads = [threading.Thread(target=partial(m.run, external_functions={'double': double})) for _ in range(4)]
    start = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    diff_parallel = time.perf_counter() - start
    # check that running the function in parallel 4 times is less than 1.5x slower than running it once
    slowdown = diff_parallel / diff
    assert slowdown < 1.5, 'Execution should not be slower in parallel'
