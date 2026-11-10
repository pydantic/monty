import _operator
import operator

# === `operator` re-exports the very same objects ===
for name in (
    'add',
    'sub',
    'neg',
    'getitem',
    'contains',
    'iadd',
    'iconcat',
    'setitem',
    'call',
    'itemgetter',
    'attrgetter',
    'methodcaller',
):
    assert getattr(operator, name) is getattr(_operator, name)

assert operator.add is _operator.add
assert operator.itemgetter is _operator.itemgetter

# === `_operator` provides the functions and the callables ===
assert _operator.add(1, 2) == 3
assert _operator.itemgetter(1)([5, 6]) == 6


class Box:
    def __init__(self) -> None:
        self.size = 3


assert _operator.attrgetter('size')(Box()) == 3
assert _operator.methodcaller('upper')('a') == 'A'
assert sorted([(2, 'b'), (1, 'a')], key=_operator.itemgetter(0)) == [(1, 'a'), (2, 'b')]

# === reprs name `operator`, as CPython does ===
assert repr(_operator.itemgetter('a')) == "operator.itemgetter('a')"

# === errors use the `_operator` prefix ===
try:
    _operator.neg()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '_operator.neg() takes exactly one argument (0 given)'
