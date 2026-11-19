try:
    id(1, 2)
except TypeError as exc:
    assert str(exc) in ('id() takes at most 1 argument (2 given)', 'id() takes exactly one argument (2 given)')
else:
    assert False, 'expected TypeError'
