# === Heap types may have the same id if lifetimes do not overlap ===
# See https://docs.python.org/3/library/functions.html#id
assert id([]) == id([])
assert id({}) == id({})
assert id((1, 2)) == id((1, 2))
assert id([1, 2]) == id([1, 2])
