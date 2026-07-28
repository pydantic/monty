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

len(out)
# ref-counts={'itertools': 1, 'obj': 5, 'r': 1, 'out': 1, 'big': 2, 'counter': 1, 'first': 2, 'cyclic': 2}
