import sys

try:
    repr()
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == "repr() missing 1 required positional argument: 'obj'"
    else:
        assert str(exc) == 'repr() takes exactly one argument (0 given)'
else:
    assert False, 'expected TypeError'
