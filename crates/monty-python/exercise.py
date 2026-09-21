"""
Exercise script for PGO data collection.

Runs all test cases through Monty worker sessions with type checking enabled,
exercising the host-side pipeline (conversions, protocol dispatch, callbacks)
for profiling.
"""

import time
from pathlib import Path
from typing import Literal

import pydantic_monty


def discard_print(_stream: Literal['stdout', 'stderr'], _text: str) -> None:
    pass


def main():
    test_cases = Path(__file__).parent.parent / 'monty' / 'test_cases'
    run, run_success, type_errors = 0, 0, 0
    crashed: list[str] = []
    start = time.perf_counter()

    # The type checker runs unguarded in the worker, so a case it cannot
    # finish must be cut off by the pool rather than stalling the whole run.
    with pydantic_monty.Monty(request_timeout=10) as pool:
        for py_file in test_cases.glob('*.py'):
            code = py_file.read_text(encoding='utf-8')

            run += 1
            try:
                # Exercise type checking and execution in the worker
                with pool.checkout(type_check=True) as session:
                    try:
                        session.feed_run(code, print_callback=discard_print)
                    except pydantic_monty.MontyTypingError:
                        # Many test cases have type errors — run unchecked
                        type_errors += 1
                        session.feed_run(code, print_callback=discard_print, skip_type_check=True)
                run_success += 1
            except pydantic_monty.MontyCrashedError:
                crashed.append(py_file.name)
            except pydantic_monty.MontyError:
                # ignore syntax errors or errors while running the code
                pass
            except Exception as e:
                raise RuntimeError(f'Error running {py_file.name}: {e}') from e

    t = time.perf_counter() - start
    print(f'Executed {run} test cases in {t:.2f} seconds, {run_success} succeeded, {type_errors} had type errors')
    if crashed:
        print(f'{len(crashed)} crashed or timed out: {", ".join(crashed)}')


if __name__ == '__main__':
    main()
