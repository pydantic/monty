item = ('a heap-backed frozenset element', 7)
source = frozenset([item])
for _ in range(100):
    alias = source.copy()
assert alias is source
assert next(iter(alias)) is item

# Rejected arguments release their references before the identity shortcut.
try:
    source.copy([item])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'frozenset.copy() takes no arguments (1 given)'
# ref-counts={'item': 2, 'source': 2, 'alias': 2}
