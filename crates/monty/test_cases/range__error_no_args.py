try:
    range()
except TypeError as exc:
    assert str(exc) in (
        "range() missing 1 required positional argument: 'start'",
        'range expected at least 1 argument, got 0',
    )
else:
    assert False, 'expected TypeError'
