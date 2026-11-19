x = []
try:
    x.insert(1, 2, 3)
except TypeError as exc:
    assert str(exc) in ('insert() takes at most 3 arguments (4 given)', 'insert expected 2 arguments, got 3')
else:
    assert False, 'expected TypeError'
