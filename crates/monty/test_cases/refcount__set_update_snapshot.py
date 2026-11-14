shared = ('heap-backed set member',)


class RaisingEquality:
    def __hash__(self):
        return hash(shared)

    def __eq__(self, other):
        raise ValueError('equality failed')


target = {RaisingEquality()}
mutable = {shared, ('another heap-backed member',)}
immutable = frozenset(mutable)
for source in [mutable, immutable]:
    for _ in range(100):
        try:
            target.update(source)
            assert False, 'expected ValueError'
        except ValueError as exc:
            assert str(exc) == 'equality failed'
    assert shared in source

target = mutable = immutable = source = RaisingEquality = None
# ref-counts={'shared': 1}
