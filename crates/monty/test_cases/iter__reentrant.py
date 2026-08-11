# Re-entrancy guard for the heap-resident `advance` path: its `Callable` arm
# must drop the heap borrow before calling back into Python, or a nested
# `next(it)` on the SAME iterator aliases that heap cell. Under
# `memory-model-checks` the overlap panics, so a regression fails loudly.

# === callable_iterator re-entering itself ===
# `f` calls `next(it)` on its own iterator while `it` is mid-advance.
it = None
depth = [0]


def f():
    depth[0] += 1
    d = depth[0]
    if d >= 3:
        return 0  # sentinel -> stop
    if d == 1:
        return next(it) + 100  # re-enter `it` mid-advance
    return d


it = iter(f, 0)
assert list(it) == [102]

# === user iterator re-entering itself ===
# `C.__next__` calls `next(it2)` on the very object being advanced, so the same
# heap cell is reached twice. `py_next` copies `self_id` out and holds no borrow
# across the call, which is what makes this safe.
it2 = None


class C:
    def __init__(self):
        self.d = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.d += 1
        if self.d >= 3:
            raise StopIteration
        if self.d == 1:
            return next(it2) + 100  # re-enter `it2` mid-advance
        return self.d


it2 = iter(C())
assert list(it2) == [102]


# === a user __next__ that mutates the instance while it is being iterated ===
# `__next__` writes to `self.seen`, which takes a mutable borrow of the same
# heap entry the iteration is driving.
class Recorder:
    def __init__(self):
        self.i = 0
        self.seen = []

    def __iter__(self):
        return self

    def __next__(self):
        if self.i >= 3:
            raise StopIteration
        self.i += 1
        self.seen.append(self.i)
        return self.i


rec = Recorder()
assert list(rec) == [1, 2, 3]
assert rec.seen == [1, 2, 3]
