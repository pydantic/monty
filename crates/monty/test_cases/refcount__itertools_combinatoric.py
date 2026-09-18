# Refcount and GC-trace coverage for the combinatoric iterators, `groupby` and
# `chain.from_iterable`.
#
# Every case holds objects NOTHING else names, so the strict unreachable walk
# has to go through each iterator's `for_each_child_id` to reach them — a
# fixture that names them separately passes even with the hook removed.
import itertools

# The pool is the only edge these three have, and it is collected at
# construction, so an untraced pool strands every item.
combos = itertools.combinations([[1], [2], [3]], 2)
replaced = itertools.combinations_with_replacement([[1], [2]], 2)
permuted = itertools.permutations([[1], [2]])

# A yielded tuple names its items too, so while BOTH the iterator and the tuple
# live each pool item has two edges. The iterator has to stay bound for that:
# an unbound one is released as the statement ends, taking its pool with it.
yielding = itertools.combinations([[1], [2]], 2)
yielded = next(yielding)

# `product` holds a pool per argument, so a hook walking only the first strands
# the second's items.
product_live = itertools.product([[1]], [[2]])

# `repeat` reuses the pools rather than copying them, so the same items are
# reached once however many result slots there are.
repeated = itertools.product([[1], [2]], repeat=3)

# The freeing paths: `py_dec_ref_ids` only runs on release, so each of these
# must be dropped rather than merely held.
gone_combos = itertools.combinations([[1], [2]], 2)
next(gone_combos)
gone_combos = None
gone_permuted = itertools.permutations([[1], [2]], 1)
next(gone_permuted)
gone_permuted = None
gone_product = itertools.product([[1]], [[2]])
next(gone_product)
gone_product = None

# A pool item outliving both the iterator and the tuple that yielded it: only
# this binding is left, so an over-decrementing hook frees it early.
survivor = next(itertools.combinations([[1], [2]], 2))[0]
assert survivor == [1]

# An iterator inside a cycle: the pool holds the list, and the list holds the
# product, so only tracing through the pool collects either.
cyclic = []
cyclic.append(itertools.product([cyclic]))


# `groupby` has four edges — source, key function, the key read ahead and the
# item read ahead — and the last two are only populated once it has stepped.
# The key function is a closure over a list nothing else names, so the key edge
# has to be traced to reach it; `len` would be a builtin with no heap entry.
def make_length_key():
    offset = [0]
    return lambda item: len(item) + offset[0]


groupers = itertools.groupby([[1], [1], [2]], make_length_key())
grouped_key, grouped_group = next(groupers)

# The freeing path for that edge: a dropped `groupby` must release its key
# function as well as its source.
gone_keyed = itertools.groupby([[1], [2]], make_length_key())
next(gone_keyed)
gone_keyed = None

# The grouper owns its parent AND its target key, so a `groupby` reachable only
# through the group it yielded must stay alive.
orphan_group = next(itertools.groupby([[5], [5]], len))[1]
assert list(orphan_group) == [[5], [5]]

# `groupby` keeps its source even when spent, as CPython holds `lz->it` to
# destruction — so this count stays at 2 where a `chain`'s falls to 1.
keys_source = iter([[1], [2, 2]])
drained_groupby = itertools.groupby(keys_source, len)
assert [(k, list(g)) for k, g in drained_groupby] == [(1, [[1]]), (2, [[2, 2]])]

# The freeing path for both types, with the grouper released before its parent.
gone_groupby = itertools.groupby([[1], [2]], len)
gone_grouper = next(gone_groupby)[1]
next(gone_grouper)
gone_grouper = None
gone_groupby = None

# `chain.from_iterable` owns the outer iterator, and the inner one it is
# draining — neither named here.
flattened = itertools.chain.from_iterable([[[1]], [[2]]])
next(flattened)

# A chain that ends releases what it can no longer reach THERE AND THEN rather
# than at destruction, so this outer iterator's count falls to 1 while the
# spent chain stays bound.
flat_source = iter([[1], [2]])
spent_flat = itertools.chain.from_iterable(flat_source)
assert list(spent_flat) == [1, 2]

# A failing inner source ends the chain too, so the outer iterator it was
# drawing from is released on the error path.
bad_source = iter([[1], 5, [2]])
failing_flat = itertools.chain.from_iterable(bad_source)
next(failing_flat)
try:
    next(failing_flat)
    assert False, 'expected the bad inner source to be rejected'
except TypeError:
    pass


# An outer source that steps the chain from inside its own `__next__` leaves a
# live inner iterator behind, which the round that was already resolving one
# must not overwrite without releasing.
class Reentrant:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == 1:
            next(reentrant_flat)
            return [[1]]
        if self.calls == 2:
            return [[2], [3]]
        raise StopIteration


reentrant_flat = itertools.chain.from_iterable(Reentrant())
assert list(reentrant_flat) == [[1]]

# A `tee` group: the buffer owns the source and everything read from it, and
# each `_tee` owns the buffer. Nothing here is named except through them.
teed = itertools.tee([[1], [2], [3]])
next(teed[0])

# Buffered items are released once the slowest consumer has passed them, so
# draining both halves leaves the buffer holding nothing.
drained_tee = itertools.tee([[1], [2]])
assert [list(x) for x in drained_tee] == [[[1], [2]], [[1], [2]]]

# The freeing path for both types. The buffer outlives the tuple and the other
# consumer, going only with the last `_tee` that holds it — which has to still
# work after they are gone.
gone_tee = itertools.tee([[1], [2]])
next(gone_tee[0])
surviving_tee = gone_tee[1]
gone_tee = None
assert list(surviving_tee) == [[1], [2]]
surviving_tee = None

gone_flat = itertools.chain.from_iterable([[[1]]])
next(gone_flat)
gone_flat = None

len('done')
# ref-counts={'itertools': 1, 'Reentrant': 1, 'reentrant_flat': 1, 'teed': 1, 'drained_tee': 1, 'combos': 1, 'yielding': 1, 'replaced': 1, 'permuted': 1, 'yielded': 1, 'product_live': 1, 'repeated': 1, 'survivor': 1, 'cyclic': 2, 'groupers': 2, 'grouped_group': 1, 'orphan_group': 1, 'keys_source': 2, 'drained_groupby': 1, 'flattened': 1, 'flat_source': 1, 'spent_flat': 1, 'bad_source': 1, 'failing_flat': 1}
