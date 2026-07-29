# Reference counting for the collections dict-backed types.
#
# The defaultdict `default_factory` is the one new refcount risk the collections
# work introduced (a heap Value the dict owns, threaded through the drop and
# cycle-collection paths). A class object is a heap-tracked callable (unlike a
# lambda), so using one as the factory lets us pin that the dict holds exactly
# one reference to it, and that a missing-key access runs the factory and stores
# the produced instance without leaking. Counter arithmetic is checked to build
# a fresh heap dict that does not alias its operands.

from collections import Counter, defaultdict, deque, namedtuple


class Box:
    pass


class BadName:
    def __str__(self):
        raise ValueError('boom')


# `dd` holds one reference to `Box` as its default_factory. A missing-key access
# runs the factory once and inserts the resulting instance under 'k'; `item`
# aliases that stored instance. The instance in turn holds a reference to its
# `Box` class.
dd = defaultdict(Box)
item = dd['k']

# Counter arithmetic: `total` is a brand-new heap Counter, not an alias of
# either operand, so `c1` / `c2` keep only their own variable bindings.
c1 = Counter('aab')
c2 = Counter('abb')
total = c1 + c2

# A deque holding a heap value also bound to a variable: `shared` is referenced
# by its own binding and by the deque's single element.
shared = [1, 2, 3]
dq = deque([shared])

# Error paths must release the arguments they were handed. `deque()` validates
# maxlen before consuming the iterable, and `index()` binds `stop` before
# checking `start`'s type — in both cases the already-bound argument has to be
# dropped on the way out, or a caught exception silently pins it forever.
# (Only the exception *type* is asserted: Monty's index() wording is a
# documented divergence, so the message can't be dual-run.)
maxlen_src = [1, 2, 3]
try:
    deque(maxlen_src, -1)
    assert False, 'expected a negative maxlen to raise'
except ValueError as e:
    assert str(e) == 'maxlen must be non-negative', 'negative maxlen message'

index_stop = [4, 5]
index_dq = deque([1, 2])
try:
    index_dq.index(1, [], index_stop)
    assert False, 'expected a bad start type to raise'
except TypeError:
    pass

# Counter algebra must free the freshly allocated result Counter when the
# operation raises partway through, and must not leak an operand snapshot it
# took but never folded in. A non-numeric count has no `+`/`-`/ordering, so each
# of these raises mid-operation; a leaked result dict orphans on the heap (the
# strict accounting check trips) and a leaked snapshot over-counts its key.

# Binary `+` after the result is already populated: `pop_key`'s left count folds
# in, then `+ 'x'` raises — the populated result dict must be freed, not leaked.
pop_key = Box()
pop_l = Counter()
pop_l[pop_key] = 1
pop_r = Counter()
pop_r[pop_key] = 'x'
try:
    pop_l + pop_r
    assert False, 'expected Counter + on a non-numeric count to raise'
except TypeError:
    pass

# Binary `+` failing on the FIRST fold: the right operand's snapshot is already
# taken and must not leak when the left fold raises.
first_key = Box()
first_l = Counter()
first_l[first_key] = 'x'
first_r = Counter()
first_r[first_key] = 1
try:
    first_l + first_r
    assert False, 'expected Counter + on a non-numeric count to raise'
except TypeError:
    pass

# `&` compares the counts of a shared key; an unorderable pair raises after the
# key has been cloned out of the left snapshot.
and_key = Box()
and_l = Counter()
and_l[and_key] = 'x'
and_r = Counter()
and_r[and_key] = 1
try:
    and_l & and_r
    assert False, 'expected Counter & on unorderable counts to raise'
except TypeError:
    pass

# Unary negation of a non-numeric count raises after the result is allocated.
neg_key = Box()
neg_src = Counter()
neg_src[neg_key] = 'x'
try:
    -neg_src
    assert False, 'expected unary - on a non-numeric count to raise'
except TypeError:
    pass

# Multiset comparison walks the union of both key sets and must drain the
# un-compared remainder on the two early exits. The trailing count is the shared
# heap big-int `cmp_tail_count`, so a leaked pair-clone shows up in its refcount.

# Short-circuit exit: `<=` fails on 'early' (2 > 1) and breaks, leaving 'tail'
# un-compared; its count clones must be released, not leaked.
cmp_tail_count = 2**70
cmp_le_l = Counter()
cmp_le_l['early'] = 2
cmp_le_l['tail'] = cmp_tail_count
cmp_le_r = Counter()
cmp_le_r['early'] = 1
cmp_le_r['tail'] = cmp_tail_count
assert not (cmp_le_l <= cmp_le_r)

# Error exit: comparing 'early' raises (str vs int is unorderable), leaving the
# 'tail' pair queued; the error path must drain it too.
cmp_err_l = Counter()
cmp_err_l['early'] = 'x'
cmp_err_l['tail'] = cmp_tail_count
cmp_err_r = Counter()
cmp_err_r['early'] = 1
cmp_err_r['tail'] = cmp_tail_count
try:
    cmp_err_l <= cmp_err_r
    assert False, 'expected <= on a non-numeric count to raise'
except TypeError:
    pass

# namedtuple field-name coercion: a field whose `__str__` raises mid-iteration
# must release the values already pulled from the iterable and the un-iterated
# remainder, or those cloned values leak.
bad_field = BadName()
tail_field = [1, 2]
try:
    namedtuple('Pt', ['ok', bad_field, tail_field])
    assert False, 'expected a raising field name to propagate'
except ValueError:
    pass

# Counting an iterable with an unhashable element raises when that element is
# used as a key; the elements already counted and the half-built counter must be
# released. `unhashable_key` is a hashable heap object counted just before the
# failure — a leaked clone would inflate its refcount past its lone binding.
unhashable_key = Box()
try:
    Counter([unhashable_key, [1]])
    assert False, 'expected an unhashable element to raise'
except TypeError:
    pass

# A namedtuple class owns its heap-ref default values, so its refcount-teardown
# walk must descend into a NamedTupleClass. Build the class holding a clone of
# `nt_default`, then drop the only class binding: freeing the class must release
# that clone, leaving `nt_default` at just its own binding. A teardown walker
# that skips the class's defaults would leak the clone, keeping `nt_default` at 2
# and tripping the strict accounting check.
nt_default = [7, 8]
nt_class = namedtuple('NtClass', ['a'], defaults=[nt_default])
nt_class = None  # drop the sole class reference, triggering its teardown

Box
# ref-counts={'Box': 9, 'BadName': 2, 'dd': 1, 'item': 2, 'c1': 1, 'c2': 1, 'total': 1, 'shared': 2, 'dq': 1, 'maxlen_src': 1, 'index_stop': 1, 'index_dq': 1, 'pop_key': 3, 'pop_l': 1, 'pop_r': 1, 'first_key': 3, 'first_l': 1, 'first_r': 1, 'and_key': 3, 'and_l': 1, 'and_r': 1, 'neg_key': 2, 'neg_src': 1, 'cmp_tail_count': 5, 'cmp_le_l': 1, 'cmp_le_r': 1, 'cmp_err_l': 1, 'cmp_err_r': 1, 'bad_field': 1, 'tail_field': 1, 'unhashable_key': 1, 'nt_default': 1}
