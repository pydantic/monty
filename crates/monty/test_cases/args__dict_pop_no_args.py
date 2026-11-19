x = {}
try:
    x.pop()
except TypeError as exc:
    assert str(exc) in (
        "pop() missing 1 required positional argument: 'key'",
        'pop expected at least 1 argument, got 0',
    )
else:
    assert False, 'expected TypeError'
