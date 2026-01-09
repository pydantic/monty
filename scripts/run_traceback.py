"""
Run a Python file and return formatted traceback for testing.

This script uses runpy.run_path() to execute a file, ensuring full traceback
information (including caret lines) is preserved. The file path in the traceback
is replaced with 'test_file.py'.
"""

import os
import re
import runpy
import sys
import traceback
from threading import Lock

from iter_test_methods import ITER_MODE_GLOBALS

lock = Lock()


def run_file_and_get_traceback(
    file_path: str, recursion_limit: int | None = None, iter_mode: bool = False
) -> str | None:
    """
    Execute a Python file and return the formatted traceback if an exception occurs.

    The traceback will have the basename as the filename for the executed code,
    with caret lines (`~~~~~`) properly shown for all frames.

    Args:
        file_path: Path to the Python file to execute.
        recursion_limit: Recursion limit for execution. CPython adds ~5 frames
            of overhead for runpy, so the effective limit for user code is
            approximately recursion_limit - 5.
        iter_mode: If True, inject external function implementations into globals
            for iter mode tests (tests that use external functions like add_ints).

    Returns:
        Formatted traceback string with '^' replaced by '~', or None if no exception.
    """
    # Get absolute path for consistent replacement
    abs_path = os.path.abspath(file_path)
    file_name = os.path.basename(abs_path)

    with lock:
        # Set recursion limit for testing.
        previous_recursion_limit = sys.getrecursionlimit()
        if recursion_limit is not None:
            sys.setrecursionlimit(recursion_limit + 5)

        # Prepare init_globals for iter mode tests
        init_globals = dict(ITER_MODE_GLOBALS) if iter_mode else None

        try:
            # Execute via runpy - this preserves full traceback info
            runpy.run_path(abs_path, init_globals=init_globals, run_name='__main__')
            return None  # No exception
        except SystemExit:
            return None  # sys.exit() is not an error
        except BaseException as e:
            # Format the traceback
            stack = traceback.format_exception(type(e), e, e.__traceback__)

            result_frames: list[str] = []
            skip_until_test_file = True

            for frame in stack:
                if skip_until_test_file:
                    # Keep the "Traceback (most recent call last):" header
                    if frame.startswith('Traceback'):
                        result_frames.append(frame)
                    # Skip until we see our test file
                    if frame.startswith(f'  File "{abs_path}"'):
                        skip_until_test_file = False
                        result_frames.append(frame.replace(abs_path, file_name))
                else:
                    if iter_mode:
                        # In iter mode, skip frames from helper modules
                        if 'iter_test_methods.py", ' in frame:
                            continue
                        # python's doing something weird and show the file as <string> for dataclass exceptions
                        if frame.startswith('  File "<string>"'):
                            continue
                    result_frames.append(frame.replace(abs_path, file_name))

            # Restore a high limit for traceback formatting
            sys.setrecursionlimit(previous_recursion_limit)
            lines = (''.join(result_frames)).splitlines()
            return '\n'.join(map(normalize_debug_range, lines)).rstrip()


def format_full_traceback(e: Exception):
    stack = traceback.format_exception(type(e), e, e.__traceback__)

    lines = (''.join(stack)).splitlines()
    return '\n'.join(map(normalize_debug_range, lines)).rstrip()


def normalize_debug_range(line: str) -> str:
    line = line.replace('dataclasses.FrozenInstanceError:', 'FrozenInstanceError:')
    if re.fullmatch(r' +[\~\^]+', line):
        return line.replace('^', '~')
    else:
        return line


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print(f'Usage: {sys.argv[0]} <file.py>', file=sys.stderr)
        sys.exit(1)

    file_path = sys.argv[1]
    if not os.path.exists(file_path):
        print(f'Error: File not found: {file_path}', file=sys.stderr)
        sys.exit(1)

    result = run_file_and_get_traceback(file_path)
    if result:
        print(result)
    else:
        print('No exception raised')
