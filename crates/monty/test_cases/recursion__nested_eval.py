# Recursive synchronous callbacks re-enter the interpreter via
# `evaluate_function`; Monty must raise `RecursionError` instead of overflowing
# the native Rust stack.

import sys


def assert_recursion_message(exc, context):
    msg = str(exc)
    if sys.platform == 'monty':
        assert msg == 'maximum recursion depth exceeded', f'unexpected {context} recursion message: {msg}'
    else:
        # CPython may hit its recursion counter, or (on smaller C stacks like the
        # datatest worker / macOS / Windows CI) a native stack-overflow guard. That
        # guard's message reports the kB used and appends a context-specific suffix
        # (`) while calling a Python object`, ... or nothing), all containing ` kB)`.
        assert msg == 'maximum recursion depth exceeded' or (
            msg.startswith('Stack overflow (used ') and ' kB)' in msg
        ), f'unexpected {context} recursion message: {msg}'


# === Recursive map() ===
def f_map(x):
    return list(map(f_map, [x]))


try:
    f_map(1)
    raise AssertionError('expected RecursionError from unbounded map() self-recursion')
except RecursionError as exc:
    assert_recursion_message(exc, 'map')


# === Recursive filter() ===
def f_filter(x):
    return list(filter(f_filter, [x]))


try:
    f_filter(1)
    raise AssertionError('expected RecursionError from unbounded filter() self-recursion')
except RecursionError as exc:
    assert_recursion_message(exc, 'filter')


# === Recursive sorted(key=...) ===
def f_sorted(x):
    return sorted([x], key=f_sorted)


try:
    f_sorted(1)
    raise AssertionError('expected RecursionError from unbounded sorted(key=...) self-recursion')
except RecursionError as exc:
    assert_recursion_message(exc, 'sorted')


# === Positive case: comfortably under the cap, still correct ===
def double_via_map(x):
    if x <= 0:
        return [0]
    return list(map(lambda v: v, double_via_map(x - 1)))


result = double_via_map(5)
assert result == [0], f'expected [0], got {result}'
