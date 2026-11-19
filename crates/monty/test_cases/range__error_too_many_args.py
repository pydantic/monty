import sys

try:
    range(1, 2, 3, 4)
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == 'range() takes at most 3 arguments (4 given)'
    else:
        assert str(exc) == 'range expected at most 3 arguments, got 4'
else:
    assert False, 'expected TypeError'
