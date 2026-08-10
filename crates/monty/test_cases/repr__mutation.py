import sys
from collections import Counter, deque


# === list: repr sees live length, so mid-repr pops truncate the output ===
class ListPopper:
    def __repr__(self):
        xs.pop()
        return 'X'


xs = [ListPopper(), ListPopper(), ListPopper()]
assert repr(xs) == '[X, X]'


# === list: mid-repr appends are picked up (each element appends once) ===
class ListGrower:
    def __init__(self):
        self.done = False

    def __repr__(self):
        if not self.done:
            self.done = True
            ys.append('tail')
        return 'X'


ys = [ListGrower(), ListGrower()]
assert repr(ys) == "[X, X, 'tail', 'tail']"


# === deque: repr formats a snapshot, so mid-repr pops change nothing ===
class DequePopper:
    def __repr__(self):
        dq.pop()
        return 'X'


dq = deque([DequePopper(), DequePopper(), DequePopper()])
assert repr(dq) == 'deque([X, X, X])'


# === set: repr formats a snapshot, so mid-repr discards change nothing ===
class SetPopper:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return self.n

    def __repr__(self):
        st.discard(next(iter(st)))
        return f'S{self.n}'


st = {SetPopper(1), SetPopper(2), SetPopper(3)}
assert repr(st) == '{S1, S2, S3}'


# === set: elements added mid-repr are not shown (snapshot on both) ===
class SetAdder:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return self.n

    def __repr__(self):
        st2.add(self.n * 100)
        return f'A{self.n}'


st2 = {SetAdder(1), SetAdder(2)}
assert repr(st2) == '{A1, A2}'


# === dict: keys inserted mid-repr are appended and printed ===
class KeyAdder:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return self.n

    def __eq__(self, other):
        return self is other

    def __repr__(self):
        if self.n == 1:
            d['new'] = 99
        return f'K{self.n}'


d = {KeyAdder(1): 1, KeyAdder(2): 2}
assert repr(d) == "{K1: 1, K2: 2, 'new': 99}"


# === dict: a deletion after the affected entries have printed is harmless ===
# (early deletions diverge — see the sys.platform sections below)
class KeyPopper:
    def __init__(self, n, mutate=False):
        self.n = n
        self.mutate = mutate

    def __hash__(self):
        return self.n

    def __eq__(self, other):
        return self is other

    def __repr__(self):
        if self.mutate:
            d2.pop(next(iter(d2)), None)
        return f'K{self.n}'


d2 = {KeyPopper(1): 1, KeyPopper(2): 2, KeyPopper(3, mutate=True): 3}
assert repr(d2) == '{K1: 1, K2: 2, K3: 3}'


# === dict: deleting a NOT-yet-printed entry mid-repr diverges ===
# Monty's dense storage shifts later entries down where CPython leaves a
# tombstone, so the entry that moves into an already-printed slot is skipped
# (see limitations/builtins.md). These also prove the live bounds re-check
# cannot panic when the dict shrinks under the repr cursor.
class ShiftPopper:
    def __init__(self, n, mutate=False):
        self.n = n
        self.mutate = mutate

    def __hash__(self):
        return self.n

    def __eq__(self, other):
        return self is other

    def __repr__(self):
        if self.mutate:
            target.pop(next(iter(target)), None)
        return f'K{self.n}'


# the FIRST key deletes itself while printing: K2 shifts into the printed slot
target = {ShiftPopper(1, mutate=True): 1, ShiftPopper(2): 2, ShiftPopper(3): 3}
if sys.platform == 'monty':
    assert repr(target) == '{K1: 1, K3: 3}'
else:
    assert repr(target) == '{K1: 1, K2: 2, K3: 3}'

# a LATER key deletes the already-printed first entry: the tail shifts past
# the repr cursor and is skipped
target = {ShiftPopper(1): 1, ShiftPopper(2, mutate=True): 2, ShiftPopper(3): 3}
if sys.platform == 'monty':
    assert repr(target) == '{K1: 1, K2: 2}'
else:
    assert repr(target) == '{K1: 1, K2: 2, K3: 3}'


# === dict: a VALUE repr mutating the dict ===
# The mutation runs between the key printing and the value printing: the
# entry's pair was already copied, so the value still prints even though its
# entry is gone; the shrink is only seen at the next entry (divergent, as
# above — CPython's tombstone keeps the middle entry).
class ValuePopper:
    def __init__(self, n, mutate=False):
        self.n = n
        self.mutate = mutate

    def __repr__(self):
        if self.mutate:
            target.pop(next(iter(target)), None)
        return f'V{self.n}'


target = {'a': ValuePopper(1, mutate=True), 'b': ValuePopper(2), 'c': ValuePopper(3)}
if sys.platform == 'monty':
    assert repr(target) == "{'a': V1, 'c': V3}"
else:
    assert repr(target) == "{'a': V1, 'b': V2, 'c': V3}"

# the LAST value deleting an already-printed entry changes nothing on either
# engine — everything affected has already printed
target = {'a': ValuePopper(1), 'b': ValuePopper(2), 'c': ValuePopper(3, mutate=True)}
assert repr(target) == "{'a': V1, 'b': V2, 'c': V3}"


# === Counter: repr orders and prints a snapshot despite mid-repr pops ===
class CountPopper:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return self.n

    def __eq__(self, other):
        return self is other

    def __repr__(self):
        c.pop(next(iter(c)), None)
        return f'C{self.n}'


c = Counter({CountPopper(1): 5, CountPopper(2): 3})
assert repr(c) == 'Counter({C1: 5, C2: 3})'
