# Two different self-referential lists compared for equality.
# CPython raises RecursionError; Monty must not panic.
a = [1, 2, 3]
b = [1, 2, 3]
a.append(a)
b.append(b)
try:
    a == b
    assert False, 'should have raised RecursionError'
except RecursionError:
    pass
