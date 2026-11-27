import sys

try:
    range()
except TypeError as exc:
    if sys.platform == 'monty':
        assert str(exc) == "range() missing 1 required positional argument: 'start'"
    else:
        assert str(exc) == 'range expected at least 1 argument, got 0'
else:
    assert False, 'expected TypeError'
