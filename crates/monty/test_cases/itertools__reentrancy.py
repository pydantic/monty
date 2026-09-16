# Every adaptor that drives a source, stepped from inside that source's own
# `__next__`. The state each one is part-way through updating changes under it,
# which is where a snapshot taken before the call gets used after it — either
# overwriting a value the nested call installed, or folding into a stale one.
import itertools


def reenter(build, hook_at=1, steps=6, take=8):
    """Drain an adaptor whose source steps it once, from inside `__next__`.

    The nested step advances the same counter, so the pull that made it returns
    a later item than it would have. `hook_at` picks which pull nests: the
    first one lands while an adaptor is still priming its state, a later one
    while it holds state from the round before.
    """
    holder = [None]
    fired = [False]

    class Source:
        def __init__(self):
            self.n = 0

        def __iter__(self):
            return self

        def __next__(self):
            self.n += 1
            if not fired[0] and self.n == hook_at and holder[0] is not None:
                fired[0] = True
                next(holder[0], None)
            if self.n > steps:
                raise StopIteration
            return [self.n]

    adaptor = build(Source())
    holder[0] = adaptor
    return [repr(x) for x in itertools.islice(adaptor, take)]


def reenter_exhausting(build, take=8):
    """Drain an adaptor whose source EMPTIES it from inside the first pull.

    The nested step runs the source dry, then the pull that made it hands back
    an item anyway — so the outer pass resumes into state already torn down.
    """
    holder = [None]

    class Source:
        def __init__(self):
            self.n = 0

        def __iter__(self):
            return self

        def __next__(self):
            self.n += 1
            if self.n == 1:
                next(holder[0], None)
                return [self.n]
            raise StopIteration

    adaptor = build(Source())
    holder[0] = adaptor
    return [repr(x) for x in itertools.islice(adaptor, take)]


# === the nested step on the priming pull ===
# It consumes the first item, so every adaptor starts from the second one. What
# each does with the round it was already part-way through is what differs.
assert reenter(itertools.pairwise) == ['([3], [4])', '([4], [5])', '([5], [6])']
assert reenter(itertools.accumulate) == ['[2, 2]', '[2, 2, 3]', '[2, 2, 3, 4]', '[2, 2, 3, 4, 5]', '[2, 2, 3, 4, 5, 6]']
assert reenter(lambda s: itertools.chain(s, [[9]])) == ['[2]', '[3]', '[4]', '[5]', '[6]', '[9]']
assert reenter(itertools.cycle, steps=3) == ['[2]', '[3]', '[2]', '[2]', '[3]', '[2]', '[2]', '[3]']
assert reenter(lambda s: itertools.islice(s, 0, None)) == ['[2]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.batched(s, 2)) == ['([3], [4])', '([5], [6])']
assert reenter(lambda s: itertools.compress(s, itertools.repeat(1))) == ['[2]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.takewhile(lambda x: True, s)) == ['[2]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.dropwhile(lambda x: False, s)) == ['[2]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.filterfalse(lambda x: False, s)) == ['[2]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.starmap(lambda x: x, s)) == ['2', '3', '4', '5', '6']
assert reenter(lambda s: itertools.zip_longest(s, [9, 8])) == [
    '([2], 8)',
    '([3], None)',
    '([4], None)',
    '([5], None)',
    '([6], None)',
]


# === the nested step on a later pull ===
# The window the priming table cannot reach: the adaptor already holds state
# from the round before, and the nested step moves it on mid-round. `pairwise`
# is where that shows — it pairs the left half it captured BEFORE the pull, so
# the first pair stays `([1], [3])` rather than following `previous` forward.
assert reenter(itertools.pairwise, hook_at=2) == ['([1], [3])', '([3], [4])', '([4], [5])', '([5], [6])']
assert reenter(itertools.accumulate, hook_at=2) == [
    '[1]',
    '[1, 3, 3]',
    '[1, 3, 3, 4]',
    '[1, 3, 3, 4, 5]',
    '[1, 3, 3, 4, 5, 6]',
]
assert reenter(lambda s: itertools.chain(s, [[9]]), hook_at=2) == ['[1]', '[3]', '[4]', '[5]', '[6]', '[9]']
assert reenter(itertools.cycle, hook_at=2, steps=3) == ['[1]', '[3]', '[1]', '[3]', '[3]', '[1]', '[3]', '[3]']
assert reenter(lambda s: itertools.islice(s, 0, None), hook_at=2) == ['[1]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.batched(s, 2), hook_at=2) == ['([1], [4])', '([5], [6])']
assert reenter(lambda s: itertools.compress(s, itertools.repeat(1)), hook_at=2) == ['[1]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.takewhile(lambda x: True, s), hook_at=2) == ['[1]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.dropwhile(lambda x: False, s), hook_at=2) == ['[1]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.filterfalse(lambda x: False, s), hook_at=2) == ['[1]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.starmap(lambda x: x, s), hook_at=2) == ['1', '3', '4', '5', '6']
assert reenter(lambda s: itertools.zip_longest(s, [9, 8]), hook_at=2) == [
    '([1], 9)',
    '([3], None)',
    '([4], None)',
    '([5], None)',
    '([6], None)',
]


# === the nested step empties the adaptor ===
# Every adaptor but `pairwise` hands back the item the spent pull returned;
# `pairwise` has no left half to pair it with, so it stops with nothing.
# `batched` is missing here because the case segfaults CPython 3.14 — Monty
# yields `['([2],)']`, but there is no reference answer to assert against.
assert reenter_exhausting(itertools.pairwise) == []
assert reenter_exhausting(itertools.accumulate) == ['[2]']
assert reenter_exhausting(lambda s: itertools.chain(s, [[9]])) == ['[2]']
assert reenter_exhausting(itertools.cycle) == ['[2]', '[2]', '[2]', '[2]', '[2]', '[2]', '[2]', '[2]']
assert reenter_exhausting(lambda s: itertools.islice(s, 0, None)) == ['[2]']
assert reenter_exhausting(lambda s: itertools.compress(s, itertools.repeat(1))) == ['[2]']
assert reenter_exhausting(lambda s: itertools.takewhile(lambda x: True, s)) == ['[2]']
assert reenter_exhausting(lambda s: itertools.dropwhile(lambda x: False, s)) == ['[2]']
assert reenter_exhausting(lambda s: itertools.filterfalse(lambda x: False, s)) == ['[2]']
assert reenter_exhausting(lambda s: itertools.starmap(lambda x: x, s)) == ['2']
assert reenter_exhausting(lambda s: itertools.zip_longest(s, [9, 8])) == ['([2], 8)']


# === chain, re-entered through `__iter__` ===
# Not reachable from the tables above: this window opens while chain RESOLVES an
# argument, and that `__iter__` can call `next()` on the SAME chain. CPython
# tests its source only at the top of `chain_next`, so an argument resolved in a
# pass that a re-entrant call ended still yields one item before the chain stops,
# and a source that call installed is simply overwritten.
class ReentrantIter:
    """Calls `next()` on the chain it is an argument of, from `__iter__`."""

    def __init__(self):
        self.inner = 'unset'

    def __iter__(self):
        try:
            self.inner = next(reentrant)
        except StopIteration:
            self.inner = 'stopped'
        return iter([3])


# The re-entrant call installs a source of its own, which the outer pass then
# replaces: 9 is yielded to nobody and 8 is never reached.
overwriting = ReentrantIter()
reentrant = itertools.chain([1], overwriting, [9, 8])
assert list(reentrant) == [1, 3]
assert overwriting.inner == 9

# The same window, with the re-entrant call ENDING the chain instead. The pass
# already past the top of the loop still yields its item.
ending = ReentrantIter()
reentrant = itertools.chain([1], ending)
assert list(reentrant) == [1, 3]
assert ending.inner == 'stopped'


# === what the nested step itself saw ===
# The tables record what the OUTER drain yields; these record what the nested
# call got back, which is the other half of agreeing with CPython about which
# state each pass reads.
class ReentrantCounter:
    """Steps the adaptor named by `target` from inside one `__next__`."""

    def __init__(self, hook_at, target):
        self.calls = 0
        self.hook_at = hook_at
        self.target = target
        self.inner = 'unset'

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == self.hook_at:
            try:
                self.inner = next(self.target())
            except StopIteration:
                self.inner = 'stopped'
        if self.calls > 6:
            raise StopIteration
        return self.calls


# `pairwise` pairs the item it captured BEFORE the pull, so a re-entrant call
# that advances the left half does not change what this pass yields — CPython
# holds its own reference to `old` across the pull.
pair_src = ReentrantCounter(2, lambda: pair_wise)
pair_wise = itertools.pairwise(pair_src)
assert list(pair_wise) == [(1, 3), (3, 4), (4, 5), (5, 6)]
assert pair_src.inner == (1, 3)

# Hooking the first pull instead, which lands while `previous` is being primed.
prime_src = ReentrantCounter(1, lambda: prime_wise)
prime_wise = itertools.pairwise(prime_src)
assert list(prime_wise) == [(3, 4), (4, 5), (5, 6)]
assert prime_src.inner == (2, 3)

# `accumulate` folds into the total as it stands AFTER the pull, so a
# re-entrant call's total is the one the next item is added to — CPython reads
# `lz->total` at that point, not before.
acc_src = ReentrantCounter(2, lambda: acc)
acc = itertools.accumulate(acc_src)
assert list(acc) == [1, 7, 11, 16, 22]
assert acc_src.inner == 4

# The same, hooked one pull later.
acc_late_src = ReentrantCounter(3, lambda: acc_late)
acc_late = itertools.accumulate(acc_late_src)
assert list(acc_late) == [1, 3, 11, 16, 22]
assert acc_late_src.inner == 7


# === the adaptors this branch adds ===
# `chain.from_iterable` drives its outer iterator through the same window as
# `chain`'s arguments, so it is stepped the same way.
assert reenter(itertools.chain.from_iterable) == ['2', '3', '4', '5', '6']
assert reenter(itertools.chain.from_iterable, hook_at=2) == ['1', '3', '4', '5', '6']

# The nested step opens the only group there is, leaving the outer drain with a
# spent parent.
assert reenter(lambda s: itertools.groupby(s, len)) == []

# `tee` is the one that refuses: a source that steps any iterator of the group
# from inside the read that fills its buffer would drive that read again.
try:
    reenter(lambda s: itertools.tee(s)[0])
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'cannot re-enter the tee iterator'
