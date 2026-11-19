import sys

x = {}
try:
    x.pop(1, 2, 3)
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == 'pop() takes at most 2 arguments (3 given)'
    else:
        assert str(exc) == 'pop expected at most 2 arguments, got 3'
else:
    assert False, 'expected TypeError'
