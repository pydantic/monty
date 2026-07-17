# === tuple.index() ===
t = (1, 2, 3, 2)
assert t.index(2) == 1
assert t.index(3) == 2
assert t.index(2, 2) == 3
assert t.index(2, 1, 4) == 1

# Regression: `-index` on i64::MIN used to panic when normalising start/end
_I64_MIN = -(2**63)
assert t.index(1, _I64_MIN) == 0
assert t.index(2, _I64_MIN, 4) == 1

t = ('a', 'b', 'c')
assert t.index('b') == 1

# === tuple.count() ===
t = (1, 2, 2, 3, 2)
assert t.count(2) == 3
assert t.count(1) == 1
assert t.count(4) == 0
assert ().count(1) == 0

t = ('a', 'b', 'a')
assert t.count('a') == 2
