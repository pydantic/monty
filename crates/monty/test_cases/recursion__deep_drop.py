# Test that dropping deeply nested containers doesn't crash (stack overflow).
# Heap::dec_ref recurses in Rust when freeing child references, so deeply
# nested containers can overflow the Rust call stack during cleanup.
# CPython handles this fine (its dealloc uses an iterative trashcan mechanism).
# Once fixed (iterative dec_ref), this should work without crashing.

# === Deep list drop ===
x = [1]
for _ in range(10000):
    x = [x]
x = None  # triggers recursive dec_ref chain
assert True, 'survived deep list drop'

# === Deep tuple drop ===
y = (1,)
for _ in range(10000):
    y = (y,)
y = None  # triggers recursive dec_ref chain
assert True, 'survived deep tuple drop'

# === Deep dict drop ===
z = {'a': 1}
for _ in range(10000):
    z = {'a': z}
z = None  # triggers recursive dec_ref chain
assert True, 'survived deep dict drop'
