# Re-iterating an iterator wraps it in a delegating iterator. Chains resolve
# iteratively, so depth costs no native stack; the bound only stops a cyclic
# chain spinning. Monty-specific: CPython builds no chain at all.
o = iter([1, 2, 3])
for _ in range(200):
    o = iter(iter(o))
assert next(o) == 1
assert list(o) == [2, 3]
