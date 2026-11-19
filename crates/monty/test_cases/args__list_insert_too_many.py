import sys

x = []
try:
    x.insert(1, 2, 3)
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == 'insert() takes at most 3 arguments (4 given)'
    else:
        assert str(exc) == 'insert expected 2 arguments, got 3'
else:
    assert False, 'expected TypeError'
