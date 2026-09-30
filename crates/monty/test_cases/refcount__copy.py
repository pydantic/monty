# Reference counting for `copy`: a deep-copy pass holds the half-built copy, the
# item being copied, and the memo pinning every source it has visited.
import copy
from collections import deque
from functools import partial

inner = ['keep']

# A deep copy that fails partway must release the half-built list, what it had
# copied into it, and the memo — leaving the sources untouched.
holder = [inner, copy]
try:
    copy.deepcopy(holder)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "cannot pickle 'module' object"

# A shallow copy fails before it allocates anything.
try:
    copy.copy(copy)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "cannot pickle 'module' object"

# A successful pass must drop the memo, so its sources end at their own counts.
source = [inner]
result = copy.deepcopy(source)
assert result == [['keep']]
assert result[0] is not inner

# A hook that raises partway through a fill must release the half-built copy
# and everything already copied into it, whichever container the walk was
# filling. `Boom` hashes cleanly while the sources are built, then raises when
# the copy of it is inserted into the copy of its container.
armed = [False]


class Boom:
    def __hash__(self):
        if armed[0]:
            raise ValueError('boom')
        return 1


class Holds:
    def __init__(self, inner):
        self.inner = inner

    def peek(self):
        return self.inner


boom_set = {Boom()}
cases = [
    [boom_set],
    {'k': boom_set},
    (boom_set, 1),
    Holds(boom_set),
    deque([boom_set]),
    frozenset([Boom()]),
    # A bound method releases the receiver clone it took before deep-copying it.
    Holds(boom_set).peek,
    # A partial copies its callable before the copy exists, so this raises with
    # nothing half-built; the two below raise with the copy already memoized.
    partial(Holds(boom_set).peek),
    partial(len, boom_set),
    partial(len, key=boom_set),
]
armed[0] = True
for case in cases:
    try:
        copy.deepcopy(case)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'boom'
armed[0] = False

# A shallow copy re-inserts every pair into the new dict, so a key that raises
# while being hashed abandons the pairs the loop had not reached; those are
# owned by the copier and must be released, not left counted. The `except` is
# tolerant because only Monty gets here: `dict.copy()` re-hashes where CPython's
# copies the table, a divergence `copy` inherits rather than introduces.
boom_dict = {Boom(): 'first', 'kept': inner}
armed[0] = True
boom_dict_copy = None
try:
    boom_dict_copy = copy.copy(boom_dict)
except ValueError as exc:
    assert str(exc) == 'boom'
armed[0] = False
# Exactly two outcomes are correct here, and a bare `except` would accept a
# third: CPython copies the hash table and returns an equal dict, Monty
# re-hashes and raises. A copy that came back unequal would otherwise pass.
assert boom_dict_copy is None or boom_dict_copy == boom_dict

# `inner` is held by itself, `holder`, `source` and `boom_dict`.
# The module itself is held by its name and by `holder`.
# `boom_set` is held by its name and by the nine cases that reach it; `Boom` by
# its name and by the three instances alive; `case` still holds the last case.
# The marker must be the last line and one line: the fixture parser reads only
# `lines.last()`, so a wrapped `ref-counts=` is silently not checked at all.
# ref-counts={'inner': 4, 'holder': 1, 'source': 1, 'result': 1, 'copy': 2, 'armed': 1, 'Boom': 4, 'Holds': 4, 'boom_set': 10, 'cases': 1, 'case': 2, 'boom_dict': 1}
