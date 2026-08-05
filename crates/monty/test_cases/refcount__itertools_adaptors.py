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

len('done')
# ref-counts={'itertools': 1, 'live': 1, 'primed': 1, 'cyclic': 2, 'paired': 1, 'sliced': 1, 'chained': 1, 'cycled': 1, 'replaying': 1, 'Boom': 2, 'erroring': 1, 'spent_source': 1, 'spent_pairwise': 1, 'stopped_source': 1, 'stopped_islice': 1, 'drained_source': 1, 'drained_islice': 1}
