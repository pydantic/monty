# Verifies that `sys.setrecursionlimit` tightens the recursion ceiling on both
# Monty (via the `test-hooks` feature) and CPython, so fixtures can drive
# RecursionError deterministically without depending on the host default.

import sys

sys.setrecursionlimit(10)


def recurse(n):
    if n > 0:
        return recurse(n - 1)
    return 0


# === Within the configured limit (10): succeeds on both ===
assert recurse(5) == 0

# === Exceeds the limit: both interpreters raise ===
try:
    recurse(100)
    raise AssertionError('expected RecursionError once depth exceeds 10')
except RecursionError as exc:
    assert str(exc) == 'maximum recursion depth exceeded', f'unexpected message: {exc}'
