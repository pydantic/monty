import sys

try:
    len(1, 2)
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == 'len() takes at most 1 argument (2 given)'
    else:
        assert str(exc) == 'len() takes exactly one argument (2 given)'
else:
    assert False, 'expected TypeError'
