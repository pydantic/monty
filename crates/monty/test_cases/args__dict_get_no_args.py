import sys

x = {}
try:
    x.get()
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == "get() missing 1 required positional argument: 'key'"
    else:
        assert str(exc) == 'get expected at least 1 argument, got 0'
else:
    assert False, 'expected TypeError'
