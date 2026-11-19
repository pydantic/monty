x = {}
try:
    x.pop(1, 2, 3)
except TypeError as exc:
    assert str(exc) in ('pop() takes at most 2 arguments (3 given)', 'pop expected at most 2 arguments, got 3')
else:
    assert False, 'expected TypeError'
