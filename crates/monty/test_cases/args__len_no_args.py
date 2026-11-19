try:
    len()
except TypeError as exc:
    assert str(exc) in (
        "len() missing 1 required positional argument: 'obj'",
        'len() takes exactly one argument (0 given)',
    )
else:
    assert False, 'expected TypeError'
