import sys

try:
    len()
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == "len() missing 1 required positional argument: 'obj'"
    else:
        assert str(exc) == 'len() takes exactly one argument (0 given)'
else:
    assert False, 'expected TypeError'
