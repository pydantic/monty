item = []
source = [item]

unstarted = (x for x in source)
suspended = (x for x in source)
yielded = next(suspended)

exhausted = (x for x in source)
assert list(exhausted) == [item]


# Both normal and exceptional completion must release the saved iterator/target stack.
def fail(value):
    raise ValueError('boom')


failed = (fail(x) for x in source)
try:
    next(failed)
    assert False, 'expected generator body failure'
except ValueError as exc:
    assert str(exc) == 'boom'

# ref-counts={'suspended': 1, 'source': 3, 'exhausted': 1, 'failed': 1, 'yielded': 4, 'item': 4, 'unstarted': 1}
