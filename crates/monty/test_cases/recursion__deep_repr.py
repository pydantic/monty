# Test that deeply nested lists don't crash during repr()
# Monty truncates with "..." at depth limit, CPython handles in C code
x = []
for _ in range(200):
    x = [x]

# Should not crash - either returns full repr or truncated with "..."
result = repr(x)
assert isinstance(result, str), 'repr should return a string'
assert result.startswith('['), 'repr should start with ['
# Either full repr or truncated
assert result.endswith(']') or '...' in result, 'repr should end with ] or contain ...'
