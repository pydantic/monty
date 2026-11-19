try:
    repr()
except TypeError as exc:
    assert str(exc) in (
        "repr() missing 1 required positional argument: 'obj'",
        'repr() takes exactly one argument (0 given)',
    )
else:
    assert False, 'expected TypeError'
