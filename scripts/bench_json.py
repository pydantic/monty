"""
Benchmark: CPython vs Monty JSON parse (loads) and serialize (dumps).

Downloads JSON fixtures from the jiter benchmarks and times
json.loads / json.dumps in both CPython and Monty.

The loop runs *inside* each runtime so that Monty's startup overhead
is paid once, not per-iteration.

Usage:
    python playground/bench_json.py [--duration N]
"""

import json
import sys
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import pydantic_monty

BASE_URL = 'https://raw.githubusercontent.com/pydantic/jiter/main/crates/jiter/benches'

BENCH_FILES = [
    'short_numbers.json',
    'bigints_array.json',
    'floats_array.json',
    'string_array.json',
    'true_array.json',
    'true_object.json',
    'sentence.json',
    'unicode.json',
    'medium_response.json',
    'pass1.json',
    'pass2.json',
]

target_duration = 2.0  # seconds per benchmark

MONTY_LOADS_CODE = """\
import json

for _ in range(iterations):
    json.loads(data)
"""

MONTY_DUMPS_CODE = """\
import json

obj = json.loads(data)
for _ in range(iterations):
    json.dumps(obj)
"""


@dataclass
class BenchResult:
    name: str
    size_bytes: int
    cpython_loads_us: float
    monty_loads_us: float
    cpython_dumps_us: float
    monty_dumps_us: float


def download_fixtures() -> dict[str, bytes]:
    """Download all benchmark JSON files and return {name: content}.

    Files are cached in a local `json_files` directory next to this script
    so they only need to be downloaded once.
    """
    cache_dir = Path(__file__).parent / 'json_files'
    cache_dir.mkdir(exist_ok=True)

    fixtures: dict[str, bytes] = {}
    for filename in BENCH_FILES:
        cached = cache_dir / filename
        if cached.exists():
            fixtures[filename] = cached.read_bytes()
        else:
            url = f'{BASE_URL}/{filename}'
            try:
                with urllib.request.urlopen(url) as resp:
                    content = resp.read()
                cached.write_bytes(content)
                fixtures[filename] = content
            except Exception as e:
                print(f'  SKIP {filename}: {e}')
    return fixtures


def calibrate_iterations(func: Callable[[int], None], pilot_n: int = 100) -> int:
    """Run func(pilot_n), then return iterations needed for ~target_duration seconds."""
    start = time.perf_counter()
    func(pilot_n)
    pilot_elapsed = time.perf_counter() - start
    if pilot_elapsed <= 0:
        pilot_elapsed = 1e-9
    per_iter = pilot_elapsed / pilot_n
    return max(pilot_n, int(target_duration / per_iter))


def bench_cpython_loads(data: bytes) -> tuple[float, int]:
    """Return (average us, iterations) for json.loads."""

    def run(n: int) -> None:
        for _ in range(n):
            json.loads(data)

    iterations = calibrate_iterations(run)
    start = time.perf_counter()
    run(iterations)
    elapsed = time.perf_counter() - start
    return (elapsed / iterations) * 1_000_000, iterations


def bench_cpython_dumps(data: bytes) -> tuple[float, int]:
    """Return (average us, iterations) for json.dumps."""
    obj = json.loads(data)

    def run(n: int) -> None:
        for _ in range(n):
            json.dumps(obj)

    iterations = calibrate_iterations(run)
    start = time.perf_counter()
    run(iterations)
    elapsed = time.perf_counter() - start
    return (elapsed / iterations) * 1_000_000, iterations


def bench_monty_loads(data: bytes) -> tuple[float, int]:
    """Return (average us, iterations) for Monty json.loads (loop inside Monty)."""
    m = pydantic_monty.Monty(MONTY_LOADS_CODE, inputs=['data', 'iterations'])

    def run(n: int) -> None:
        m.run(inputs={'data': data, 'iterations': n})

    iterations = calibrate_iterations(run)
    start = time.perf_counter()
    run(iterations)
    elapsed = time.perf_counter() - start
    return (elapsed / iterations) * 1_000_000, iterations


def bench_monty_dumps(data: bytes) -> tuple[float, int]:
    """Return (average us, iterations) for Monty json.dumps (loop inside Monty)."""
    m = pydantic_monty.Monty(MONTY_DUMPS_CODE, inputs=['data', 'iterations'])

    def run(n: int) -> None:
        m.run(inputs={'data': data, 'iterations': n})

    iterations = calibrate_iterations(run)
    start = time.perf_counter()
    run(iterations)
    elapsed = time.perf_counter() - start
    return (elapsed / iterations) * 1_000_000, iterations


def format_us(us: float) -> str:
    """Format microseconds nicely."""
    if us >= 1000:
        return f'{us / 1000:.2f}ms'
    return f'{us:.1f}us'


def format_ratio(cpython_us: float, monty_us: float) -> str:
    """Format the speed ratio."""
    if monty_us < cpython_us:
        return f'{cpython_us / monty_us:.2f}x faster'
    return f'{monty_us / cpython_us:.2f}x slower'


def run_benchmarks() -> list[BenchResult]:
    print(f'Downloading {len(BENCH_FILES)} JSON fixtures from jiter benchmarks...')
    fixtures = download_fixtures()
    print(f'Downloaded {len(fixtures)} fixtures')
    print(f'Target duration per benchmark: {target_duration}s\n')

    results: list[BenchResult] = []
    for name, data in fixtures.items():
        size = len(data)
        print(f'--- {name} ({size:,} bytes) ---')

        # loads
        print('  json.loads:', end=' ', flush=True)
        cp_loads, cp_n = bench_cpython_loads(data)
        print(f'CPython={format_us(cp_loads)} ({cp_n:,} iters)', end='  ', flush=True)
        mt_loads, mt_n = bench_monty_loads(data)
        print(f'Monty={format_us(mt_loads)} ({mt_n:,} iters)', end='  ')
        print(f'[{format_ratio(cp_loads, mt_loads)}]')

        # dumps
        print('  json.dumps:', end=' ', flush=True)
        cp_dumps, cp_n = bench_cpython_dumps(data)
        print(f'CPython={format_us(cp_dumps)} ({cp_n:,} iters)', end='  ', flush=True)
        mt_dumps, mt_n = bench_monty_dumps(data)
        print(f'Monty={format_us(mt_dumps)} ({mt_n:,} iters)', end='  ')
        print(f'[{format_ratio(cp_dumps, mt_dumps)}]')

        results.append(
            BenchResult(
                name=name,
                size_bytes=size,
                cpython_loads_us=cp_loads,
                monty_loads_us=mt_loads,
                cpython_dumps_us=cp_dumps,
                monty_dumps_us=mt_dumps,
            )
        )
        print()

    return results


def print_summary(results: list[BenchResult]) -> None:
    """Print a summary table."""
    header = f'{"Fixture":<25} {"Size":>8} {"loads CPy":>10} {"loads Mty":>10} {"loads ratio":>14} {"dumps CPy":>10} {"dumps Mty":>10} {"dumps ratio":>14}'
    print('=' * len(header))
    print('SUMMARY')
    print('=' * len(header))
    print(header)
    print('-' * len(header))

    for r in results:
        print(
            f'{r.name:<25} {r.size_bytes:>7,}B'
            f' {format_us(r.cpython_loads_us):>10} {format_us(r.monty_loads_us):>10} {format_ratio(r.cpython_loads_us, r.monty_loads_us):>14}'
            f' {format_us(r.cpython_dumps_us):>10} {format_us(r.monty_dumps_us):>10} {format_ratio(r.cpython_dumps_us, r.monty_dumps_us):>14}'
        )

    # overall geometric mean of ratios
    loads_ratios = [r.monty_loads_us / r.cpython_loads_us for r in results]
    dumps_ratios = [r.monty_dumps_us / r.cpython_dumps_us for r in results]

    geo_mean_loads = _geo_mean(loads_ratios)
    geo_mean_dumps = _geo_mean(dumps_ratios)

    print('-' * len(header))
    print(f'Geometric mean ratio (Monty/CPython):  loads={geo_mean_loads:.2f}x  dumps={geo_mean_dumps:.2f}x')
    if geo_mean_loads < 1:
        print(f'  -> Monty loads is {1 / geo_mean_loads:.2f}x faster on average')
    else:
        print(f'  -> Monty loads is {geo_mean_loads:.2f}x slower on average')
    if geo_mean_dumps < 1:
        print(f'  -> Monty dumps is {1 / geo_mean_dumps:.2f}x faster on average')
    else:
        print(f'  -> Monty dumps is {geo_mean_dumps:.2f}x slower on average')


def _geo_mean(values: list[float]) -> float:
    product = 1.0
    for v in values:
        product *= v
    return product ** (1.0 / len(values))


def main() -> None:
    global target_duration
    if '--duration' in sys.argv:
        idx = sys.argv.index('--duration')
        target_duration = float(sys.argv[idx + 1])

    print('JSON Benchmark: CPython vs Monty\n')

    results = run_benchmarks()
    print()
    print_summary(results)


if __name__ == '__main__':
    main()
