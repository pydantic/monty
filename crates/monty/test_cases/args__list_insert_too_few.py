import sys

x = []
try:
    x.insert(1)
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == "insert() missing 1 required positional argument: 'object'"
    else:
        assert str(exc) == 'insert expected 2 arguments, got 1'
else:
    assert False, 'expected TypeError'
