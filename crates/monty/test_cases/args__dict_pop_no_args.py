import sys

x = {}
try:
    x.pop()
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == "pop() missing 1 required positional argument: 'key'"
    else:
        assert str(exc) == 'pop expected at least 1 argument, got 0'
else:
    assert False, 'expected TypeError'
