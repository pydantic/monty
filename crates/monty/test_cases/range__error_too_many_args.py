try:
    range(1, 2, 3, 4)
except TypeError as exc:
    assert str(exc) in ('range() takes at most 3 arguments (4 given)', 'range expected at most 3 arguments, got 4')
else:
    assert False, 'expected TypeError'
