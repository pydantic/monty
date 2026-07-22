"""Run JSONL code snippets concurrently via `AsyncMonty`.

Each non-empty input line must be a JSON object with a string `code` field.
The runner records status counts and optionally logs snippets that crash a
worker.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
from collections import Counter
from pathlib import Path
from typing import IO

import jiter

from pydantic_monty import AsyncMonty, CollectString, MontyCrashedError, MontyError, ResourceLimits

DEFAULT_LIMITS: ResourceLimits = {
    'max_duration_secs': 1.0,
    'max_memory': 64 * 1024 * 1024,
    'max_recursion_depth': 500,
}


async def run_one(
    pool: AsyncMonty,
    sem: asyncio.Semaphore,
    index: int,
    code: str,
    request_timeout: float,
    panic_log: IO[str] | None,
) -> str:
    """Execute a single snippet and return a short status tag."""
    async with sem:
        try:
            async with pool.checkout(
                script_name=f'snippet_{index}.py',
                limits=DEFAULT_LIMITS,
            ) as session:
                await asyncio.wait_for(
                    session.feed_run(code, print_callback=CollectString()),
                    timeout=request_timeout,
                )
            return 'completed'
        except asyncio.TimeoutError:
            return 'timeout'
        except MontyCrashedError as exc:
            # `timed_out=True` is the watchdog killing a slow worker, not a real panic.
            if exc.timed_out:
                return 'MontyCrashedError:timeout'
            if panic_log is not None:
                panic_log.write(
                    json.dumps(
                        {
                            'index': index,
                            'exit_status': exc.exit_status,
                            'message': str(exc),
                            'code': code,
                        }
                    )
                    + '\n'
                )
                panic_log.flush()
            print(f'[snippet {index}] PANIC exit={exc.exit_status}: {exc}', flush=True)
            return 'MontyCrashedError:panic'
        except MontyError as exc:
            return type(exc).__name__
        except Exception as exc:
            print(f'[snippet {index}] {type(exc).__name__}: {exc}', flush=True)
            return f'runner_error:{type(exc).__name__}'


async def main(snippets: list[str], max_processes: int, request_timeout: float, panic_log_path: Path | None) -> None:
    sem = asyncio.Semaphore(max_processes)
    counts: Counter[str] = Counter()
    total = len(snippets)
    started = time.monotonic()
    done = 0

    panic_log = panic_log_path.open('w', encoding='utf-8') if panic_log_path is not None else None

    try:
        async with AsyncMonty(max_processes=max_processes, request_timeout=request_timeout) as pool:

            async def task(i: int, code: str) -> None:
                nonlocal done
                status = await run_one(pool, sem, i, code, request_timeout, panic_log)
                counts[status] += 1
                done += 1
                if done % 1000 == 0 or done == total:
                    rate = done / (time.monotonic() - started)
                    print(f'{done}/{total} ({rate:.1f}/s)', flush=True)

            await asyncio.gather(*(task(i, c) for i, c in enumerate(snippets)))
    finally:
        if panic_log is not None:
            panic_log.close()

    print(f'\nfinished in {time.monotonic() - started:.1f}s')
    for status, n in counts.most_common():
        print(f'  {status}: {n}')


def cli() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        '--input',
        default='all_code.jsonl',
        help='Path to JSONL file with one {"code": "..."} object per line.',
    )
    parser.add_argument(
        '--max-processes',
        type=int,
        default=os.cpu_count() or 1,
        help='Pool size; defaults to host CPU count.',
    )
    parser.add_argument('--request-timeout', type=float, default=5.0)
    parser.add_argument(
        '--panic-log',
        default='subprocess_panic_cases.jsonl',
        help='Write panic-causing snippets to this JSONL file (empty string disables).',
    )
    args = parser.parse_args()

    snippets: list[str] = []
    with Path(args.input).open('rb') as f:
        for line in f:
            if not line.strip():
                continue
            code = jiter.from_json(line)['code']
            if isinstance(code, str) and code.strip():
                snippets.append(code)
    print(f'running {len(snippets)} snippets across {args.max_processes} workers', flush=True)

    panic_log_path = Path(args.panic_log) if args.panic_log else None
    asyncio.run(main(snippets, args.max_processes, args.request_timeout, panic_log_path))
    return 0


if __name__ == '__main__':
    sys.exit(cli())
