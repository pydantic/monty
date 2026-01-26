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
import tempfile
import traceback
from threading import Lock

from iter_test_methods import ITER_MODE_GLOBALS

lock = Lock()


def run_file_and_get_traceback(
    fixture_file_path: str,
    recursion_limit: int | None = None,
    iter_mode: bool = False,
    async_mode: bool = False,
) -> str | None:
    """
    Execute a Python file and return the formatted traceback if an exception occurs.

    The traceback will have the basename as the filename for the executed code,
    with caret lines (`~~~~~`) properly shown for all frames.

    Args:
        fixture_file_path: Path to the Python file to execute.
        recursion_limit: Recursion limit for execution. CPython adds ~5 frames
            of overhead for runpy, so the effective limit for user code is
            approximately recursion_limit - 5.
        iter_mode: If True, inject external function implementations into globals
            for iter mode tests (tests that use external functions like add_ints).
        async_mode: If True, wrap code in an async context for tests with
            top-level await that Monty supports but CPython doesn't.

    Returns:
        Formatted traceback string with '^' replaced by '~', or None if no exception.
    """
    # Get absolute path for consistent replacement
    abs_path = os.path.abspath(fixture_file_path)
    file_name = os.path.basename(fixture_file_path)

    # Async line offset: 1 lines for "async def __test_main():\n"
    line_offset = 0

    with open(abs_path) as f:
        code = f.read()

    if async_mode:
        # Wrap code in async context: indent everything by 4 spaces and add wrapper
        indented = '\n'.join([f'    {line}' if line else '' for line in code.split('\n')])

        code = f'async def __test_main():\n{indented}\nimport asyncio as __asy\n__asy.run(__test_main())'
        # Async line offset: 1 lines for "async def __test_main():\n"
        line_offset = 1

    with lock:
        # Set recursion limit for testing.
        previous_recursion_limit = sys.getrecursionlimit()
        if recursion_limit is not None:
            sys.setrecursionlimit(recursion_limit + 5)

        # Prepare init_globals for iter mode tests
        init_globals = dict(ITER_MODE_GLOBALS) if iter_mode else None

        with tempfile.NamedTemporaryFile(mode='w', suffix='.py') as tmp_file:
            tmp_file.write(code)
            tmp_file.flush()
            file_path = tmp_file.name

            try:
                runpy.run_path(file_path, init_globals=init_globals, run_name='__main__')
            except SystemExit:
                pass  # don't error on ctrl+c
            except BaseException as e:
                # Format the traceback
                stack = traceback.format_exception(type(e), e, e.__traceback__)

                result_frames: list[str] = []
                found_user_code = False

                for frame in stack:
                    # Keep the "Traceback (most recent call last):" header
                    if frame.startswith('Traceback'):
                        result_frames.append(frame)
                        continue
                    elif '__asy.run(__test_main())' in frame:
                        # Skip the asyncio.run(__test_main()) wrapper frame
                        continue
                    elif '/asyncio/' in frame:
                        # Skip asyncio internal frames
                        continue
                    elif iter_mode:
                        # In iter mode, skip frames from helper modules
                        if 'iter_test_methods.py", ' in frame:
                            continue
                        # python's doing something weird and show the file as <string> for dataclass exceptions
                        if frame.startswith('  File "<string>"'):
                            continue

                    # Skip until we see our test file
                    if not found_user_code and frame.startswith(f'  File "{file_path}"'):
                        found_user_code = True

                    if found_user_code:
                        if async_mode:
                            if adjusted_frame := _adjust_async_frame(frame, file_path, file_name, line_offset):
                                result_frames.append(adjusted_frame)
                        else:
                            result_frames.append(frame.replace(file_path, file_name))

                # Restore a high limit for traceback formatting
                sys.setrecursionlimit(previous_recursion_limit)
                lines = (''.join(result_frames)).splitlines()
                return '\n'.join(map(normalize_debug_range, lines)).rstrip()


def _adjust_async_frame(frame: str, tmp_path: str, file_name: str, line_offset: int) -> str | None:
    """
    Adjust a traceback frame from the async wrapper to show original line numbers.

    Returns the adjusted frame, or None if the frame should be skipped.
    """
    # Parse the frame to extract and adjust the line number
    # Format: '  File "path", line N, in func\n    code\n    ~~~~\n'
    frame = frame.replace(tmp_path, file_name)

    # Replace __test_main with <module> since it represents module-level code
    frame = frame.replace('in __test_main', 'in <module>')

    # Find and adjust line number using regex
    match = re.search(r'line (\d+)', frame)
    if match:
        old_line = int(match.group(1))
        new_line = old_line - line_offset
        if new_line < 1:
            return None  # Skip frames from wrapper code
        frame = frame.replace(f'line {old_line}', f'line {new_line}')

    return frame


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
