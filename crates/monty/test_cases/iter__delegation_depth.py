# Re-iterating an iterator wraps it in a delegating iterator. Chains are
# resolved iteratively, so depth costs no native stack — but a bound still
# applies so a cyclic chain cannot spin forever. Monty-specific: CPython
# returns the iterator unchanged and builds no chain at all.
#
# Kept well under the 1000 limit so this stays a normal-operation test.
o = iter([1, 2, 3])
for _ in range(200):
    o = iter(iter(o))
assert next(o) == 1, 'a deep delegation chain still advances the terminal iterator'
assert list(o) == [2, 3], 'and shares its position'
