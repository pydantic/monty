shared = []
source = {}
for _ in range(100):
    returned = source.setdefault('keep', shared)
    source.update(source, extra=shared)
    try:
        source.setdefault([], [shared])
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "cannot use 'list' as a dict key (unhashable type: 'list')"
assert returned is shared


class RaisingHash:
    def __hash__(self):
        raise ValueError('hash failed')


bad = RaisingHash()
try:
    source.setdefault(bad, [shared])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'hash failed'


class RaisingEquality:
    def __hash__(self):
        return 7

    def __eq__(self, other):
        raise ValueError('equality failed')


eq_source = {RaisingEquality(): shared}
try:
    eq_source.setdefault(7, [shared])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'equality failed'
eq_source.clear()

# Removing the last live entries must release their children despite retained tombstones.
drained = {i: shared for i in range(128)}
for i in range(128):
    drained.pop(i)
assert list(drained.values()) == []
# ref-counts={'shared': 4, 'source': 1, 'returned': 4, 'RaisingHash': 2, 'bad': 1, 'RaisingEquality': 1, 'eq_source': 1, 'drained': 1}
