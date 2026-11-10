value = []
default = []
key = ('a heap-backed dictionary key',)
d = {key: value}


class Key:
    def __hash__(self):
        return 0

    def __eq__(self, other):
        raise ValueError('comparison failed')


stored = Key()
probe = Key()
colliding = {stored: value}
for _ in range(100):
    assert d.get(key, default) is value
    assert d.get('missing', default) is default
    assert d.get('missing') is None
    try:
        d.get([], default)
        assert False, 'expected TypeError'
    except TypeError:
        pass
    try:
        colliding.get(probe, default)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'comparison failed'
# ref-counts={'value': 3, 'default': 1, 'key': 2, 'd': 1, 'Key': 3, 'stored': 2, 'probe': 1, 'colliding': 1}
