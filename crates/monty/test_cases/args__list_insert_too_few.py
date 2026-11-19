x = []
try:
    x.insert(1)
except TypeError as exc:
    assert str(exc) in (
        "insert() missing 1 required positional argument: 'object'",
        'insert expected 2 arguments, got 1',
    )
else:
    assert False, 'expected TypeError'
