# Test that hashing deeply nested containers raises RecursionError instead
# of crashing with a Rust stack overflow.

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
