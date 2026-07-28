# Every itertools type ships a refcount fixture; only `ref-count-return` /
# `memory-model-checks` catches a missing `py_dec_ref_ids` arm.
#
# `obj` ends at 5 because `repeat` never releases its object (CPython keeps it
# too): its binding, the spent iterator, and three identical items in `out`.
# `first` IS `big` — the same heap LongInt — so both report 2.
import itertools

obj = [1, 2]
r = itertools.repeat(obj, 3)
out = list(r)

big = 2**70
counter = itertools.count(big)
first = next(counter)

# The cycle collector must reach `repeat`'s object: this list holds the only
# reference to an iterator that in turn holds the list.
cyclic = []
cyclic.append(itertools.repeat(cyclic, 1))

# The cases above name everything their iterators hold, so the strict walk
# reaches it all without the trace hooks. These hold objects nothing else names,
# forcing the walk through `for_each_child_id`.
held = itertools.repeat([1, 2], 2)
# Both operands unnamed, so a hook walking only `current` strands `step`.
stepped = itertools.count(2**70, 2**70)

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# `object` leaves the list alive with no referrer.
dropped = itertools.repeat([3, 4], 1)
dropped = None

len(out)
# ref-counts={'itertools': 1, 'obj': 5, 'r': 1, 'out': 1, 'big': 2, 'counter': 1, 'first': 2, 'cyclic': 2, 'held': 1, 'stepped': 1}
