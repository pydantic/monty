x = {}
try:
    x.get()
except TypeError as exc:
    assert str(exc) in (
        "get() missing 1 required positional argument: 'key'",
        'get expected at least 1 argument, got 0',
    )
else:
    assert False, 'expected TypeError'
