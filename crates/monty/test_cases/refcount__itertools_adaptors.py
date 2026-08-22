# Refcount and GC-trace coverage for the source-wrapping `itertools` adaptors.
#
# Every case holds objects NOTHING else names, so the strict unreachable walk
# has to go through each adaptor's `for_each_child_id` to reach them — a fixture
# that names them separately passes even with the hook removed.
import itertools

# `source` is reachable only through the adaptor.
live = itertools.pairwise([1, 2, 3])

# Mid-iteration the adaptor also holds `previous`, a second owned ref that is
# not the source; the list below is reachable only via that field.
primed = itertools.pairwise([[1], [2], [3]])
next(primed)

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# either field leaves its object alive with no referrer.
dropped = itertools.pairwise([[9], [8]])
next(dropped)
dropped = None

# An adaptor inside a cycle: the list holds the only reference to a pairwise
# that in turn holds the list, so only tracing through it can collect either.
cyclic = []
cyclic.append(itertools.pairwise(cyclic))

# compress owns two iterators; only tracing both reaches either list.
paired = itertools.compress([[1], [2]], [1, 0])

# islice owns one, and skipping discards items rather than buffering them.
sliced = itertools.islice([[1], [2], [3]], 1, 3)
next(sliced)

# The freeing path for each adaptor. These must be DROPPED, not merely held:
# `py_dec_ref_ids` only runs when the adaptor is released, so a live binding
# exercises `for_each_child_id` alone and a missing release goes unnoticed.
gone_compress = itertools.compress([[1], [2]], [1, 1])
gone_compress = None
gone_islice = itertools.islice([[1], [2]], 1)
gone_islice = None

# chain keeps every unresolved argument plus the live one; cycle keeps the
# source AND its saved buffer, so both need every element traced.
chained = itertools.chain([[1]], [[2]])
next(chained)
cycled = itertools.cycle([[1], [2]])
next(cycled)

# cycle's saved buffer is only the SOLE owner once the source is spent: until
# then its items stay reachable through the source iterator, so a fixture that
# stops early cannot see a missing `saved` trace. Three steps over two items
# exhausts the source and starts the replay.
replaying = itertools.cycle([[1], [2]])
next(replaying)
next(replaying)
next(replaying)

gone_chain = itertools.chain([[1]], [[2]])
next(gone_chain)
gone_chain = None
gone_cycle = itertools.cycle([[1], [2]])
next(gone_cycle)
gone_cycle = None


# An exception mid-iteration leaves `next` through a `?` early return, the path
# where a missed cleanup is easiest to introduce. The adaptor stays live and
# usable afterwards, so anything it holds must still be accounted for.
class Boom:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('boom')


erroring = itertools.pairwise(Boom())
try:
    next(erroring)
except ValueError:
    pass


# Exhausting an adaptor releases its source THERE AND THEN, not at destruction,
# so whatever the source itself holds is reclaimed as soon as it is spent. Each
# source is named separately, so its count is 1 only if the spent adaptor let go
# of it — a retained source would leave 2. The adaptors stay bound so that it is
# the release, not their destruction, being measured.
spent_source = iter([1, 2])
spent_pairwise = itertools.pairwise(spent_source)
list(spent_pairwise)

# islice has two spending paths and only one runs here: reaching `stop` before
# the source ends, which must release without waiting for a StopIteration.
stopped_source = iter([1, 2, 3])
stopped_islice = itertools.islice(stopped_source, 1)
list(stopped_islice)

# The other islice path: `stop` is never reached, so the source runs out first.
drained_source = iter([1, 2])
drained_islice = itertools.islice(drained_source, 5)
list(drained_islice)

# chain holds its arguments UNRESOLVED, so what an ended chain must release is
# the ARGUMENT itself, not just the iterator it resolved from it. Both ways a
# chain ends have to release: draining the last argument...
chain_drained_source = [1, 2]
chain_drained = itertools.chain(chain_drained_source)
list(chain_drained)

# ...and an argument that fails `iter()`, which ends the chain for good. The
# arguments after the bad one are unreachable, so pinning them keeps objects
# alive that nothing can ever yield.
chain_unreached_source = [3, 4]
chain_failed = itertools.chain([1], 5, chain_unreached_source)
next(chain_failed)
try:
    next(chain_failed)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"


# The predicate-driven adaptors own a CALLABLE as well as a source, so each has
# a second trace edge. A closure is used deliberately: a plain `def` is an
# immediate `Value`, not a heap ref, so it would exercise no hook at all.
def make_shorter_than(limit):
    bound = list(range(limit))

    def shorter(x):
        return len(x) < len(bound)

    return shorter


def make_adder():
    bound = [1]

    def add(a, b=0):
        return a + b + len(bound)

    return add


def make_boom():
    bound = [1]

    def boom(*args):
        raise ValueError('boom' + str(len(bound)))

    return boom


# Each closure is passed inline and never named, so the adaptor's callable
# field is its only referrer; the items are lists for the same reason.
take_live = itertools.takewhile(make_shorter_than(3), [[1], [2]])
next(take_live)
drop_live = itertools.dropwhile(make_shorter_than(0), [[1], [2]])
next(drop_live)
filter_live = itertools.filterfalse(make_shorter_than(0), [[1], [2]])
next(filter_live)
star_live = itertools.starmap(make_adder(), [(1,), (2,)])
next(star_live)

# filterfalse with a None predicate leaves only the source edge, so a hook that
# traces the callable twice still fails to reach these.
filter_none = itertools.filterfalse(None, [[1], []])

# The freeing paths: `py_dec_ref_ids` runs only on release, so each of these
# must be dropped rather than merely held.
gone_take = itertools.takewhile(make_shorter_than(3), [[1], [2]])
next(gone_take)
gone_take = None
gone_drop = itertools.dropwhile(make_shorter_than(0), [[1], [2]])
next(gone_drop)
gone_drop = None
gone_filter = itertools.filterfalse(make_shorter_than(0), [[1], [2]])
next(gone_filter)
gone_filter = None
gone_star = itertools.starmap(make_adder(), [(1,)])
next(gone_star)
gone_star = None

# A rejected item is dropped rather than yielded — the guard path inside `next`.
rejected = itertools.takewhile(make_shorter_than(0), [[1], [2]])
assert list(rejected) == []

# A callable that raises leaves `next` through a `?` while the guard still
# holds the item being tested, and for starmap the arguments already collected.
pred_erroring = itertools.takewhile(make_boom(), [[1], [2]])
try:
    next(pred_erroring)
except ValueError:
    pass

star_erroring = itertools.starmap(make_boom(), [(1, 2)])
try:
    next(star_erroring)
except ValueError:
    pass


# Spending an adaptor releases what it can no longer reach, THERE AND THEN
# rather than at destruction — as `pairwise` and `islice` do above. Each source
# and callable is named separately, so a count of 1 means the spent adaptor let
# go of it and 2 means it is still held. The adaptors stay bound so it is the
# release being measured, not their destruction.
take_pred = make_shorter_than(0)
take_source = iter([[1], [2]])
latched_take = itertools.takewhile(take_pred, take_source)
assert list(latched_take) == []

# `dropwhile` releases neither: the predicate goes uncalled after the first
# rejection but stays owned to destruction, as CPython holds `lz->func`, and
# it never latches, so every later `next` drives the source again.
drop_pred = make_shorter_than(1)
drop_source = iter([[], [1]])
past_drop = itertools.dropwhile(drop_pred, drop_source)
assert next(past_drop) == [1]

len('done')
# ref-counts={'itertools': 1, 'live': 1, 'primed': 1, 'cyclic': 2, 'paired': 1, 'sliced': 1, 'chained': 1, 'cycled': 1, 'replaying': 1, 'Boom': 2, 'erroring': 1, 'spent_source': 1, 'spent_pairwise': 1, 'stopped_source': 1, 'stopped_islice': 1, 'drained_source': 1, 'drained_islice': 1, 'chain_drained_source': 1, 'chain_drained': 1, 'chain_unreached_source': 1, 'chain_failed': 1, 'take_live': 1, 'drop_live': 1, 'filter_live': 1, 'star_live': 1, 'filter_none': 1, 'rejected': 1, 'pred_erroring': 1, 'star_erroring': 1, 'take_pred': 1, 'take_source': 1, 'latched_take': 1, 'drop_pred': 2, 'drop_source': 2, 'past_drop': 1}
