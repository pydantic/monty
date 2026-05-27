# Test that deeply nested lists don't crash during repr().
# `sys.setrecursionlimit(10)` caps both Monty and CPython at depth 10, so any
# input deeper than that exercises the truncation / RecursionError paths.

import sys

sys.setrecursionlimit(10)

x = []
for _ in range(20):
    x = [x]

result = repr(x)
assert isinstance(result, str), 'repr should return a string'
assert result.startswith('['), 'repr should start with ['
assert result.endswith(']') or '...' in result, 'repr should end with ] or contain ...'

# Deeply nested one-element tuples must hit the same depth guard, otherwise
# `repr()` recurses unbounded and overflows the host Rust stack.
t = (0,)
for _ in range(20):
    t = (t,)

result2 = repr(t)
assert isinstance(result2, str), 'tuple repr should return a string'
assert result2.startswith('('), 'tuple repr should start with ('
assert result2.endswith(')') or '...' in result2, 'tuple repr should end with ) or contain ...'
