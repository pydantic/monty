# Test that hashing deeply nested tuples doesn't crash (stack overflow).
# The py_hash path recurses in Rust without a DepthGuard, so deeply
# nested immutable containers can overflow the Rust call stack.
# CPython handles this fine (its tuple hash is iterative in C).
# Once fixed, Monty should either succeed or raise RecursionError.

# === Deep tuple hash ===
x = (1,)
for _ in range(10000):
    x = (x,)

try:
    h = hash(x)
    assert isinstance(h, int), 'hash should return an int'
except RecursionError:
    pass  # acceptable if depth guard triggers

# === Deep frozenset hash ===
y = frozenset({1})
for _ in range(10000):
    y = frozenset({y})

try:
    h = hash(y)
    assert isinstance(h, int), 'hash should return an int'
except RecursionError:
    pass  # acceptable if depth guard triggers

# === Deep tuple as dict key (triggers hash) ===
z = (1,)
for _ in range(10000):
    z = (z,)

d = {}
try:
    d[z] = 'value'
except RecursionError:
    pass  # acceptable if depth guard triggers

# === Deep tuple as set element (triggers hash) ===
w = (1,)
for _ in range(10000):
    w = (w,)

s = set()
try:
    s.add(w)
except RecursionError:
    pass  # acceptable if depth guard triggers
