# Regression: `json.dumps()` of a deeply nested non-cyclic structure must raise
# RecursionError on Monty rather than overflow the native Rust stack. CPython's
# `_json.c` is iterative and does not honor `setrecursionlimit` here, so it
# happily serializes the same input — we accept that outcome on CPython but
# require RecursionError on Monty.
#
# Each container type is covered separately because the three serialization
# branches (list, tuple, dict) hold their recursion tokens in different ways
# (list / tuple via the live `ListIter` / `TupleIter`; dict via a token
# acquired explicitly after `collect_dict_entries` releases the `DictIter`).

import json
import sys

sys.setrecursionlimit(20)

monty = sys.platform == 'monty'


# === Deep list ===
x = [1]
for _ in range(50):
    x = [x]
try:
    json.dumps(x)
    assert not monty, 'monty should raise RecursionError on deep list'
except RecursionError:
    pass

# === Deep tuple ===
y = (1,)
for _ in range(50):
    y = (y,)
try:
    json.dumps(y)
    assert not monty, 'monty should raise RecursionError on deep tuple'
except RecursionError:
    pass

# === Deep dict ===
d = {'x': 1}
for _ in range(50):
    d = {'x': d}
try:
    json.dumps(d)
    assert not monty, 'monty should raise RecursionError on deep dict'
except RecursionError:
    pass

# === Shallow nesting must still serialize cleanly under the same limit ===
# Confirms the depth guard isn't trigger-happy — shouldn't reject all nesting.
shallow = [1, [2, [3, [4, [5]]]]]
assert json.dumps(shallow) == '[1, [2, [3, [4, [5]]]]]'
