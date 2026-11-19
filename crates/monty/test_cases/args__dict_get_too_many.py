x = {}
try:
    x.get(1, 2, 3)
except TypeError as exc:
    assert str(exc) in ('get() takes at most 2 arguments (3 given)', 'get expected at most 2 arguments, got 3')
else:
    assert False, 'expected TypeError'
