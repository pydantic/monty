# === Basic for loop ===
result = []
for i in range(5):
    result.append(i)
assert result == [0, 1, 2, 3, 4], 'basic for loop'

# === Tuple unpacking in for loop ===
pairs = [(1, 2), (3, 4), (5, 6)]
sums = []
for a, b in pairs:
    sums.append(a + b)
assert sums == [3, 7, 11], 'for loop with pair unpacking'

# === Triple unpacking ===
triples = [(1, 2, 3), (4, 5, 6)]
products = []
for a, b, c in triples:
    products.append(a * b * c)
assert products == [6, 120], 'for loop with triple unpacking'

# === Nested tuple unpacking ===
nested = [((1, 2), 3), ((4, 5), 6)]
results = []
for (a, b), c in nested:
    results.append(a + b + c)
assert results == [6, 15], 'for loop with nested unpacking'

# === Deep nested unpacking ===
deep = [((1, 2), (3, 4)), ((5, 6), (7, 8))]
sums = []
for (a, b), (c, d) in deep:
    sums.append(a + b + c + d)
assert sums == [10, 26], 'for loop with deep nested unpacking'

# === Mixed depth unpacking ===
mixed = [(1, (2, 3)), (4, (5, 6))]
results = []
for a, (b, c) in mixed:
    results.append(a + b + c)
assert results == [6, 15], 'for loop with mixed depth unpacking'

# === Unpacking with else clause ===
pairs = [(1, 2), (3, 4)]
total = 0
for a, b in pairs:
    total += a + b
else:
    total += 100
assert total == 110, 'for loop unpacking with else clause'

# === Enumerate with unpacking ===
items = ['a', 'b', 'c']
result = []
for i, val in enumerate(items):
    result.append((i, val))
assert result == [(0, 'a'), (1, 'b'), (2, 'c')], 'enumerate with unpacking'

# === Dict items unpacking ===
d = {'x': 1, 'y': 2}
keys = []
vals = []
for k, v in d.items():
    keys.append(k)
    vals.append(v)
assert sorted(keys) == ['x', 'y'], 'dict items unpacking keys'
assert sorted(vals) == [1, 2], 'dict items unpacking values'

# === Subscript target as for-loop variable (issue #408) ===
# Subscript target stores into the existing container on each iteration; the
# final iteration's value persists after the loop.
box = [None]
for box[0] in (1, 2, 3):
    pass
assert box[0] == 3, 'for-loop subscript target retains last value'

# Subscript target with computed index — index is re-evaluated each iteration.
# If `i` were cached at loop entry both stores would target a[0], yielding
# [20, 0]; re-evaluation lets the body's mutation of `i` shift the second store
# to a[1].
a = [0, 0]
i = 0
for a[i] in (10, 20):
    i = 1
assert a == [10, 20], 'for-loop subscript target re-evaluates index each iteration'

# Subscript target combined with body that reads from the same container
trace = []
a = [0]
for a[0] in (1, 2, 3):
    trace.append(a[0])
assert trace == [1, 2, 3], 'for-loop subscript target visible inside body'

# Subscript target inside a nested tuple target
a = [0]
for a[0], b in [(1, 'x'), (2, 'y')]:
    pass
assert a == [2], 'for-loop nested: subscript updated'
assert b == 'y', 'for-loop nested: name bound'
