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
    file_path: str,
    recursion_limit: int | None = None,
    iter_mode: bool = False,
    async_mode: bool = False,
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
        async_mode: If True, wrap code in an async context for tests with
            top-level await that Monty supports but CPython doesn't.

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

        # For async mode, wrap the code and execute from a temp file
        if async_mode:
            return _run_async_file_and_get_traceback(abs_path, file_name, init_globals, previous_recursion_limit)

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


def _run_async_file_and_get_traceback(
    abs_path: str,
    file_name: str,
    init_globals: dict[str, object] | None,
    previous_recursion_limit: int,
) -> str | None:
    """
    Execute an async test file by wrapping its code in an async context.

    This handles tests with top-level await that Monty supports but CPython doesn't.
    The code is wrapped in an async function and run via asyncio.run().
    Line numbers in tracebacks are adjusted to match the original file.
    """
    with open(abs_path) as f:
        code = f.read()

    # Wrap code in async context: indent everything by 4 spaces and add wrapper
    indented = '\n'.join([f'    {line}' if line else '' for line in code.split('\n')])

    wrapped = f'import asyncio\nasync def __test_main():\n{indented}\nasyncio.run(__test_main())'

    # Write to temp file so tracebacks show proper file paths
    tmp_fd, tmp_path = tempfile.mkstemp(suffix='.py')
    try:
        with os.fdopen(tmp_fd, 'w') as tmp_file:
            tmp_file.write(wrapped)

        try:
            runpy.run_path(tmp_path, init_globals=init_globals, run_name='__main__')
            return None  # No exception
        except SystemExit:
            return None  # sys.exit() is not an error
        except BaseException as e:
            # Format the traceback
            stack = traceback.format_exception(type(e), e, e.__traceback__)

            result_frames: list[str] = []
            found_user_code = False
            # Line offset: 2 lines for "import asyncio\nasync def __test_main():\n"
            line_offset = 2

            for frame in stack:
                # Keep the "Traceback (most recent call last):" header
                if frame.startswith('Traceback'):
                    result_frames.append(frame)
                    continue

                # Skip frames until we find user code in our temp file
                if not found_user_code:
                    # Skip the asyncio.run(__test_main()) wrapper frame
                    if 'asyncio.run(__test_main())' in frame:
                        continue
                    # Skip asyncio internal frames
                    if '/asyncio/' in frame or '\\asyncio\\' in frame:
                        continue
                    # Found a frame from our temp file that's actual user code
                    if frame.startswith(f'  File "{tmp_path}"'):
                        found_user_code = True
                        adjusted_frame = _adjust_async_frame(frame, tmp_path, file_name, line_offset)
                        if adjusted_frame:
                            result_frames.append(adjusted_frame)
                    continue

                # Process remaining frames
                # Skip asyncio internal frames
                if '/asyncio/' in frame or '\\asyncio\\' in frame:
                    continue
                if frame.startswith(f'  File "{tmp_path}"'):
                    adjusted_frame = _adjust_async_frame(frame, tmp_path, file_name, line_offset)
                    if adjusted_frame:
                        result_frames.append(adjusted_frame)
                else:
                    result_frames.append(frame)

            sys.setrecursionlimit(previous_recursion_limit)
            lines = (''.join(result_frames)).splitlines()
            return '\n'.join(map(normalize_debug_range, lines)).rstrip()
    finally:
        os.unlink(tmp_path)


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
