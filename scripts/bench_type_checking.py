"""
Benchmark: time successive type-checked feeds on different snippets.

Runs six distinct snippets in a fixed order so you can see the one-time pooled-db
cold-start cost (call 1) vs. the steady-state cost (calls 2-6) once a scrubbed
pooled database is available for reuse inside the worker, without re-checking the
exact same source text.

Each call checks out a fresh type-checking session from a shared worker pool, so
the measured time includes the protocol round trips and the (tiny) execution
cost on top of the type check itself.

Usage:
    python scripts/bench_type_checking.py
"""

import time

import pydantic_monty

SNIPPETS: list[tuple[str, str]] = [
    (
        'union_return',
        """\
def pick_value(flag: bool, text: str) -> str | None:
    if flag:
        return text
    return None

pick_value(True, 'hello')
""",
    ),
    (
        'list_comprehension',
        """\
def scale(values: list[int]) -> list[int]:
    return [value * 2 for value in values]

scale([1, 2, 3])
""",
    ),
    (
        'dict_lookup',
        """\
def total(data: dict[str, int]) -> int:
    return data['left'] + data['right']

total({'left': 1, 'right': 2})
""",
    ),
    (
        'tuple_unpack',
        """\
def make_pair(name: str, count: int) -> tuple[str, int]:
    return name, count

label, amount = make_pair('item', 3)
""",
    ),
    (
        'optional_branch',
        """\
def normalize(value: int | None) -> int:
    if value is None:
        return 0
    return value

normalize(5)
""",
    ),
    (
        'nested_function',
        """\
def outer(scale: int) -> int:
    def inner(value: int) -> int:
        return value * scale

    return inner(4)

outer(3)
""",
    ),
]


def format_ms(seconds: float) -> str:
    """Format seconds as ms or us depending on magnitude."""
    if seconds >= 1e-3:
        return f'{seconds * 1000:.2f} ms'
    return f'{seconds * 1_000_000:.1f} us'


def time_one_call(pool: pydantic_monty.Monty, code: str) -> float:
    """Time a single type-checked feed in a fresh session.

    A fresh session per call mirrors typical usage (each snippet gets its own
    session) and avoids the session's accumulated type-check context hiding
    the effect we want to measure.
    """
    with pool.checkout(type_check=True) as session:
        start = time.perf_counter()
        session.feed_run(code)
        return time.perf_counter() - start


def main() -> None:
    print('type_check() latency, six successive calls on distinct snippets')
    print('-' * 70, flush=True)

    times: list[float] = []
    with pydantic_monty.Monty() as pool:
        for i, (name, code) in enumerate(SNIPPETS, start=1):
            print(f'  call {i} ({name}): running...', end='', flush=True)
            t = time_one_call(pool, code)
            times.append(t)
            speedup = f'  {times[0] / t:.1f}x faster than call 1' if i > 1 and t > 0 else ''
            print(f'\r  call {i} {name:>20}: {format_ms(t):>10}{speedup}          ', flush=True)

    print('-' * 70)


if __name__ == '__main__':
    main()
