# Every adaptor that drives a source, stepped from inside that source's own
# `__next__`. The state each one is part-way through updating changes under it,
# which is where a snapshot taken before the call gets used after it — either
# overwriting a value the nested call installed, or folding into a stale one.
import itertools


def reenter(build, steps=6, take=8):
    """Drain an adaptor whose source steps it once, from inside `__next__`."""
    holder = [None]
    fired = [False]

    class Source:
        def __init__(self):
            self.n = 0

        def __iter__(self):
            return self

        def __next__(self):
            self.n += 1
            if not fired[0] and holder[0] is not None:
                fired[0] = True
                next(holder[0], None)
            if self.n > steps:
                raise StopIteration
            return [self.n]

    adaptor = build(Source())
    holder[0] = adaptor
    return [repr(x) for x in itertools.islice(adaptor, take)]


# The nested step consumes the first item, so every adaptor starts from the
# second one. What each does with the round it was already part-way through is
# the part that differs.
assert reenter(itertools.pairwise) == ['([3], [4])', '([4], [5])', '([5], [6])']
assert reenter(itertools.cycle, steps=3) == ['[2]', '[3]', '[2]', '[2]', '[3]', '[2]', '[2]', '[3]']
assert reenter(lambda s: itertools.islice(s, 0, None)) == ['[2]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.batched(s, 2)) == ['([3], [4])', '([5], [6])']
assert reenter(lambda s: itertools.compress(s, itertools.repeat(1))) == ['[2]', '[3]', '[4]', '[5]', '[6]']
assert reenter(lambda s: itertools.chain(s, [[9]])) == ['[2]', '[3]', '[4]', '[5]', '[6]', '[9]']
assert reenter(itertools.chain.from_iterable) == ['2', '3', '4', '5', '6']
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

# `accumulate` folds into the total the nested step installed, not into the one
# that was there when the round began.
assert reenter(itertools.accumulate) == [
    '[2, 2]',
    '[2, 2, 3]',
    '[2, 2, 3, 4]',
    '[2, 2, 3, 4, 5]',
    '[2, 2, 3, 4, 5, 6]',
]

# The nested step opens the only group there is, leaving the outer drain with
# a spent parent.
assert reenter(lambda s: itertools.groupby(s, len)) == []
