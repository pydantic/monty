try:
    len(1, 2)
except TypeError as exc:
    assert str(exc) in ('len() takes at most 1 argument (2 given)', 'len() takes exactly one argument (2 given)')
else:
    assert False, 'expected TypeError'
