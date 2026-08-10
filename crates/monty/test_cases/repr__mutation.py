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


# === dict: keys deleted mid-repr still print all original entries ===
class KeyPopper:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return self.n

    def __eq__(self, other):
        return self is other

    def __repr__(self):
        d.pop(next(iter(d)), None)
        return f'K{self.n}'


d = {KeyPopper(1): 1, KeyPopper(2): 2, KeyPopper(3): 3}
assert repr(d) == '{K1: 1, K2: 2, K3: 3}'


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
