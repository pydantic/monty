# Test nested tuple unpacking

# === Basic nested unpacking ===
data = ((1, 2), 'x')
(a, b), c = data
assert a == 1, 'nested unpack first inner'
assert b == 2, 'nested unpack second inner'
assert c == 'x', 'nested unpack outer'

# === Deeply nested ===
((a, b), (c, d)) = ((1, 2), (3, 4))
assert a == 1, 'deep nested first'
assert b == 2, 'deep nested second'
assert c == 3, 'deep nested third'
assert d == 4, 'deep nested fourth'

# === Mixed depths ===
(a, (b, c)) = (1, (2, 3))
assert a == 1, 'mixed depth outer'
assert b == 2, 'mixed depth inner first'
assert c == 3, 'mixed depth inner second'

# === Three levels deep ===
(a, (b, (c, d))) = (1, (2, (3, 4)))
assert a == 1, 'three level outer'
assert b == 2, 'three level mid'
assert c == 3, 'three level inner first'
assert d == 4, 'three level inner second'

# === In for loops ===
items = [((1, 2), 'a'), ((3, 4), 'b')]
sums = []
letters = []
for (a, b), c in items:
    sums.append(a + b)
    letters.append(c)
assert sums == [3, 7], 'for loop nested unpack sums'
assert letters == ['a', 'b'], 'for loop nested unpack letters'

# === In comprehensions ===
items = [((1, 2), 'a'), ((3, 4), 'b')]
result = [a + b for (a, b), c in items]
assert result == [3, 7], 'comprehension nested unpack'

# === Deep nested in comprehension ===
items = [((1, 2), (3, 4)), ((5, 6), (7, 8))]
result = [a + b + c + d for (a, b), (c, d) in items]
assert result == [10, 26], 'comprehension deep nested unpack'
