"""
Exercise script for PGO data collection.

Runs all test cases through Monty with type checking enabled,
exercising the full interpreter pipeline for profiling.
"""

import time
from pathlib import Path

import pydantic_monty


def main():
    test_cases = Path(__file__).parent.parent / 'monty' / 'test_cases'
    run, run_success, type_errors = 0, 0, 0
    start = time.perf_counter()

    for py_file in test_cases.glob('*.py'):
        code = py_file.read_text(encoding='utf-8')

        # Exercise parsing and type checking
        try:
            try:
                m = pydantic_monty.Monty(code, type_check=True)
            except pydantic_monty.MontyTypingError:
                # Many test cases have type errors
                m = pydantic_monty.Monty(code)
                type_errors += 1

            # Exercise execution
            run += 1
            m.run(print_callback=lambda _, __: None)
            run_success += 1
        except pydantic_monty.MontyError:
            # ignore syntax errors or errors while running the code
            pass
        except Exception as e:
            raise RuntimeError(f'Error running {py_file.name}: {e}') from e

    t = time.perf_counter() - start
    print(f'Executed {run} test cases in {t:.2f} seconds, {run_success} succeeded, {type_errors} had type errors')


if __name__ == '__main__':
    main()
