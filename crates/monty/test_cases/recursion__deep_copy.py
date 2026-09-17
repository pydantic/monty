# Test that deep-copying nested containers doesn't crash (stack overflow).
# `copy.deepcopy` recurses in Rust on Monty and in Python on CPython, so the
# limit is lowered here to align both — CPython reaches it at half the nesting,
# spending two frames per level where Monty charges one.
import copy
import sys

sys.setrecursionlimit(50)

# === Deep list copy ===
x = []
for _ in range(10):
    x = [x]

y = copy.deepcopy(x)
depth = 0
while y:
    y = y[0]
    depth += 1
assert depth == 10

# === Deep dict copy ===
d = {}
for _ in range(10):
    d = {'inner': d}

d_copy = copy.deepcopy(d)
depth = 0
while d_copy:
    d_copy = d_copy['inner']
    depth += 1
assert depth == 10

# === Past the limit: RecursionError on both, not a crash ===
deep = []
for _ in range(100):
    deep = [deep]

try:
    copy.deepcopy(deep)
    raise AssertionError('expected RecursionError once nesting exceeds the limit')
except RecursionError as exc:
    assert str(exc) == 'maximum recursion depth exceeded'
