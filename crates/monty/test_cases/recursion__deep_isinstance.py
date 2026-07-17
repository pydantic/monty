# gc-interval=200

# Test that isinstance() with deeply-nested classinfo tuples raises
# RecursionError instead of overflowing the Rust stack.

# === Deeply nested classinfo tuple ===
classinfo = (int,)
for _ in range(10000):
    classinfo = (classinfo,)

try:
    result = isinstance(1, classinfo)
    assert result == True
except RecursionError:
    pass  # acceptable when depth guard triggers

# === Mismatch through deep nesting ===
classinfo2 = (str,)
for _ in range(10000):
    classinfo2 = (classinfo2,)

try:
    result2 = isinstance(1, classinfo2)
    assert result2 == False
except RecursionError:
    pass  # acceptable when depth guard triggers
