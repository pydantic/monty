# An `lru_cache` wrapper owns its function plus every cached key and value, so a
# missing arm in `py_dec_ref_ids` / `for_each_child_id` only shows up under
# `ref-count-return` / `memory-model-checks`.
#
# `key` ends at 3: its binding, the tuple key the cache stores, and the result
# tuple `held` — which is itself at 2, being both a binding and the cache's
# stored value. `cyclic` ends at 2: its binding and the value cached inside the
# wrapper the list itself holds.
import functools


def wrapped(a, b=0):
    return (a, b)


cached = functools.cache(wrapped)

# A multi-argument call is keyed by a tuple the cache owns, and the result it
# stores holds the arguments in turn.
key = (1,)
held = cached(key, 2)
assert cached(key, 2) is held

# An evicted entry must release both halves: this one is pushed out by the next
# call, leaving `evicted` referenced only by its binding.
small = functools.lru_cache(maxsize=1)(wrapped)
evicted = (2,)
small(evicted, 0)
small((3,), 0)

# The cycle collector must reach a cached value: the list holds the wrapper, and
# the wrapper's stored result is the list.
cyclic = []
holder = functools.cache(lambda: cyclic)
cyclic.append(holder)
holder()
holder = None

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# the cache leaves the stored key and value alive with no referrer.
dropped = functools.cache(wrapped)
dropped((4, 5), 6)
dropped = None

# A re-entrant `__eq__` can insert the very key an in-flight store is about to
# write, so the insertion collides and `Dict::set` hands back the old value —
# whose reference the cache no longer holds. `shared` ends at 3: its binding and
# the two entries the two-entry cache keeps.
shared = ('s',)


def constant(x):
    return shared


colliding = functools.lru_cache(maxsize=2)(constant)
eq_calls = [0]


class Reentrant:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return 3

    def __eq__(self, other):
        eq_calls[0] += 1
        # The eighth comparison is the one inside the insertion's own probe;
        # reaching that branch depends on the dict's probe order, so a change
        # there means picking the count again rather than dropping the case.
        if eq_calls[0] == 8:
            colliding(Reentrant(2))
        return self.v == other.v


colliding(Reentrant(0))
colliding(Reentrant(1))
colliding(Reentrant(2))

len(held)
# ref-counts={'functools': 1, 'wrapped': 3, 'cached': 1, 'key': 3, 'held': 2, 'small': 1, 'evicted': 1, 'cyclic': 2, 'shared': 3, 'colliding': 1, 'eq_calls': 1, 'Reentrant': 3}
