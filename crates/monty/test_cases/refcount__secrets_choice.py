import secrets

bad = iter([1])
try:
    secrets.choice(bad)
except TypeError:
    pass
else:
    assert False, 'choice requires a sized sequence'
[2]
# ref-counts={'secrets': 1, 'bad': 1}
