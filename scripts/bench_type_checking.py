"""
Benchmark: time successive calls to `Monty.type_check()` on the same small snippet.

Measures the first, second, third, and fourth call independently so you can see
the one-time pooled-db warmup cost (first call) vs. the steady-state cost (calls 2+)
once a scrubbed warm database is available for reuse.

Usage:
    python scripts/bench_type_checking.py [--runs N]
"""

import sys
import time

import pydantic_monty

CODE = """\
def foo(x: int, y: str | bytes) -> list[int | str | bytes]:
    return [x, y]

foo(1, '2')
"""


def format_ms(seconds: float) -> str:
    """Format seconds as ms or us depending on magnitude."""
    if seconds >= 1e-3:
        return f'{seconds * 1000:.2f} ms'
    return f'{seconds * 1_000_000:.1f} us'


def time_one_call() -> float:
    """Create a fresh Monty and time a single type_check invocation.

    A new Monty per call mirrors typical usage (each snippet gets its own instance)
    and avoids any per-instance caching hiding the effect we want to measure.
    """
    m = pydantic_monty.Monty(CODE)
    start = time.perf_counter()
    result = m.type_check()
    elapsed = time.perf_counter() - start
    assert result is None, f'unexpected type errors: {result}'
    return elapsed


def main() -> None:
    runs = 4
    if '--runs' in sys.argv:
        runs = int(sys.argv[sys.argv.index('--runs') + 1])

    print('type_check() latency, successive calls')
    print('-' * 50, flush=True)

    times: list[float] = []
    for i in range(1, runs + 1):
        print(f'  call {i}: running...', end='', flush=True)
        t = time_one_call()
        times.append(t)
        speedup = f'  {times[0] / t:.1f}x faster than call 1' if i > 1 and t > 0 else ''
        print(f'\r  call {i}: {format_ms(t):>10}{speedup}          ', flush=True)

    print('-' * 50)


if __name__ == '__main__':
    main()
