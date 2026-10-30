import secrets

bad = iter([1])
try:
    secrets.choice(bad)
except TypeError:
    pass
else:
    assert False, 'choice requires a sized sequence'
# ref-counts={'bad': 1}
