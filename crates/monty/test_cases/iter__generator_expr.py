# === Basic generator expression ===
result = list(x * 2 for x in range(5))
assert result == [0, 2, 4, 6, 8], 'basic generator expression'

# === With condition ===
result = list(x for x in range(10) if x % 2 == 0)
assert result == [0, 2, 4, 6, 8], 'generator with condition'

# === Nested generators ===
result = list(x + y for x in range(3) for y in range(2))
assert result == [0, 1, 1, 2, 2, 3], 'nested generator'

# === Generator in function call ===
result = sum(x for x in range(5))
assert result == 10, 'generator in sum()'

# === Generator with unpacking ===
pairs = [(1, 2), (3, 4)]
result = list(a + b for a, b in pairs)
assert result == [3, 7], 'generator with unpacking'
