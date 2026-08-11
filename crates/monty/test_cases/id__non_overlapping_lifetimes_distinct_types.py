# xfail=cpython
# === Heap types may have the same id if lifetimes do not overlap ===
# See https://docs.python.org/3/library/functions.html#id
# for Cpython it happens to be the case that for different types they end
# up being allocated in different memory locations, but this is not guaranteed by the language spec
assert id([]) == id([])
assert id([]) == id({})
assert id([]) == id((1,))
assert id((1, 2)) == id((1, 2))
assert id([1, 2]) == id([1, 2])
