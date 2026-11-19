# /// monty
# call-external = true
# ///
# Native consumers of a user `__next__` that calls an external function: `for`, `list()`, `tuple()`, `sum()`.
# Every external call here suspends inside a native continuation, and the harness dumps and reloads there.


class Counter:
    """Yields `add_ints(i, offset)` for `i` in `1..=stop`, calling the host on every item."""

    def __init__(self, stop, offset=100):
        self.n = 0
        self.stop = stop
        self.offset = offset

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > self.stop:
            raise StopIteration
        return add_ints(self.n, self.offset)


class Failing:
    """Yields one host value, then raises `ValueError` from a host call."""

    def __init__(self):
        self.n = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n == 1:
            return add_ints(1, 1)
        raise_error('ValueError', 'boom')


class Plain:
    """A user iterator that never suspends, for consumers behind a synchronous boundary."""

    def __init__(self, stop):
        self.n = 0
        self.stop = stop

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > self.stop:
            raise StopIteration
        return self.n


# === for loops ===
out = []
for x in Counter(3):
    out.append(x)
assert out == [101, 102, 103]

out = []
for x in Counter(0):
    out.append(x)
assert out == []

out = []
for x in Counter(5):
    if x == 103:
        break
    out.append(x)
assert out == [101, 102]

out = []
for x in Counter(4):
    if x % 2:
        continue
    out.append(x)
assert out == [102, 104]

for x in Counter(2):
    pass
else:
    out.append('else')
assert out == [102, 104, 'else']

# nested loops, each suspending
pairs = []
for a in Counter(2):
    for b in Counter(2, 10):
        pairs.append((a, b))
assert pairs == [(101, 11), (101, 12), (102, 11), (102, 12)]

# comprehensions iterate with `ForIter` too
assert [x * 2 for x in Counter(2)] == [202, 204]
assert {x for x in Counter(2)} == {101, 102}
assert {x: x + 1 for x in Counter(1)} == {101: 102}


def loop_in_function(stop):
    total = 0
    for x in Counter(stop):
        total += x
    return total


assert loop_in_function(3) == 306

# === list() and tuple() ===
assert list(Counter(3)) == [101, 102, 103]
assert list(Counter(0)) == []
assert tuple(Counter(2)) == (101, 102)
assert tuple(Counter(0)) == ()
assert list(Counter(2)) + list(Counter(1, 5)) == [101, 102, 6]

# the iterator is shared: a partly consumed iterator collects the rest
it = Counter(4)
assert add_ints(next(it), 0) == 101
assert list(it) == [102, 103, 104]
assert list(it) == []

# === sum() ===
assert sum(Counter(3)) == 306
assert sum(Counter(0)) == 0
assert sum(Counter(2), 1000) == 1203
assert sum(Counter(2), start=-3) == 200

try:
    sum(Counter(2), [])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'can only concatenate list (not "int") to list'

# === errors from __next__ propagate, StopIteration does not ===
seen = []
try:
    for x in Failing():
        seen.append(x)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'
assert seen == [2]

for consume in (list, tuple, sum):
    try:
        consume(Failing())
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'boom'


class RaisesOwn:
    """Raises a specific exception object after a suspension, to check its identity survives."""

    error = KeyError('mine')

    def __iter__(self):
        return self

    def __next__(self):
        add_ints(0, 0)
        raise RaisesOwn.error


for consume in (list, sum):
    try:
        consume(RaisesOwn())
        assert False, 'expected KeyError'
    except KeyError as exc:
        assert exc is RaisesOwn.error
try:
    for x in RaisesOwn():
        pass
    assert False, 'expected KeyError'
except KeyError as exc:
    assert exc is RaisesOwn.error


class CatchesInside:
    """Handles a host error inside `__next__`, then stops."""

    def __init__(self):
        self.done = False

    def __iter__(self):
        return self

    def __next__(self):
        if self.done:
            raise StopIteration
        self.done = True
        try:
            raise_error('RuntimeError', 'inner')
        except RuntimeError as exc:
            return str(exc)


assert list(CatchesInside()) == ['inner']
assert [x for x in CatchesInside()] == ['inner']

# === continuations nested in each other's callbacks ===


class Outer:
    """Each item sums a fresh suspending iterator, so one continuation waits inside another."""

    def __init__(self, stop):
        self.n = 0
        self.stop = stop

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > self.stop:
            raise StopIteration
        inner = []
        for x in Counter(self.n, 0):
            inner.append(x)
        return sum(Counter(self.n, 0)) + len(list(Counter(self.n))) * 1000 + len(inner)


assert list(Outer(3)) == [1002, 2005, 3009]
assert sum(Outer(2)) == 3007
assert [x for x in Outer(2)] == [1002, 2005]

# === synchronous boundaries still drive continuations ===
# `sorted` calls its key in Rust; the key's `sum()` and `list()` complete there without suspending.
assert sorted([3, 1, 2], key=lambda k: sum(Plain(k))) == [1, 2, 3]
assert sorted([2, 1], key=lambda k: list(Plain(k))) == [1, 2]
assert list(map(tuple, [Plain(2), Plain(0)])) == [(1, 2), ()]

# === finally blocks inside __next__ run around the suspension ===


class WithFinally:
    def __init__(self, stop):
        self.n = 0
        self.stop = stop
        self.log = []

    def __iter__(self):
        return self

    def __next__(self):
        try:
            self.n += 1
            if self.n > self.stop:
                raise StopIteration
            return add_ints(self.n, 0)
        finally:
            self.log.append(self.n)


w = WithFinally(2)
assert list(w) == [1, 2]
assert w.log == [1, 2, 3]

# === break and return out of a suspended for loop release the iterator ===


def first_over(limit):
    for x in Counter(10):
        if x > limit:
            return x
    return None


assert first_over(102) == 103
assert first_over(200) is None
