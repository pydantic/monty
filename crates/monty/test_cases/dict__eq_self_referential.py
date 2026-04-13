# Two different self-referential dicts compared for equality.
# CPython raises RecursionError; Monty must not panic.
d = {}
d['self'] = d
e = {}
e['self'] = e
try:
    d == e
    assert False, 'should have raised RecursionError'
except RecursionError:
    pass
