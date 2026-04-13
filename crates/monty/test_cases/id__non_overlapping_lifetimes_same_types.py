# === Heap types may have the same id if lifetimes do not overlap ===
# See https://docs.python.org/3/library/functions.html#id
assert id([]) == id([]), 'empty list may have same id'
assert id({}) == id({}), 'empty dict may have same id'
assert id((1, 2)) == id((1, 2)), 'non-empty tuple may have same id'
assert id([1, 2]) == id([1, 2]), 'non-empty list may have same id'
