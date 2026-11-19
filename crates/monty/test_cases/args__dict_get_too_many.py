import sys

x = {}
try:
    x.get(1, 2, 3)
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == 'get() takes at most 2 arguments (3 given)'
    else:
        assert str(exc) == 'get expected at most 2 arguments, got 3'
else:
    assert False, 'expected TypeError'
