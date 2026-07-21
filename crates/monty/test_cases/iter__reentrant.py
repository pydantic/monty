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
