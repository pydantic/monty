# Test nested tuple unpacking

# === Basic nested unpacking ===
data = ((1, 2), 'x')
(a, b), c = data
assert a == 1
assert b == 2
assert c == 'x'

# === Deeply nested ===
((a, b), (c, d)) = ((1, 2), (3, 4))
assert a == 1
assert b == 2
assert c == 3
assert d == 4

# === Mixed depths ===
(a, (b, c)) = (1, (2, 3))
assert a == 1
assert b == 2
assert c == 3

# === Three levels deep ===
(a, (b, (c, d))) = (1, (2, (3, 4)))
assert a == 1
assert b == 2
assert c == 3
assert d == 4

# === In for loops ===
items = [((1, 2), 'a'), ((3, 4), 'b')]
sums = []
letters = []
for (a, b), c in items:
    sums.append(a + b)
    letters.append(c)
assert sums == [3, 7]
assert letters == ['a', 'b']

# === In comprehensions ===
items = [((1, 2), 'a'), ((3, 4), 'b')]
result = [a + b for (a, b), c in items]
assert result == [3, 7]

# === Deep nested in comprehension ===
items = [((1, 2), (3, 4)), ((5, 6), (7, 8))]
result = [a + b + c + d for (a, b), (c, d) in items]
assert result == [10, 26]
