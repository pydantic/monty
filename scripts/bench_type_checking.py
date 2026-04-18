"""
Benchmark: time successive calls to `Monty.type_check()` on different snippets.

Runs six distinct snippets in a fixed order so you can see the one-time pooled-db
cold-start cost (call 1) vs. the steady-state cost (calls 2-6) once a scrubbed
pooled database is available for reuse, without re-checking the exact same source
text.

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


def time_one_call(code: str) -> float:
    """Create a fresh Monty and time a single type_check invocation.

    A new Monty per call mirrors typical usage (each snippet gets its own instance)
    and avoids any per-instance caching hiding the effect we want to measure.
    """
    m = pydantic_monty.Monty(code)
    start = time.perf_counter()
    result = m.type_check()
    elapsed = time.perf_counter() - start
    assert result is None, f'unexpected type errors: {result}'
    return elapsed


def main() -> None:
    print('type_check() latency, six successive calls on distinct snippets')
    print('-' * 70, flush=True)

    times: list[float] = []
    for i, (name, code) in enumerate(SNIPPETS, start=1):
        print(f'  call {i} ({name}): running...', end='', flush=True)
        t = time_one_call(code)
        times.append(t)
        speedup = f'  {times[0] / t:.1f}x faster than call 1' if i > 1 and t > 0 else ''
        print(f'\r  call {i} {name:>20}: {format_ms(t):>10}{speedup}          ', flush=True)

    print('-' * 70)


if __name__ == '__main__':
    main()
