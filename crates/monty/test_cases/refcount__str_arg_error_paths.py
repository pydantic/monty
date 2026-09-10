# Refcount coverage for `str` integer-argument error paths.
#
# `str.expandtabs` owns its `tabsize` argument and must release it even when the
# C `int` range check rejects it -- that early return is where a missed release
# is easiest to introduce. A `LongInt` argument is heap-allocated (wider than a
# machine int), so a leak is observable rather than hidden in an inline value.


class BigIndex:
    def __index__(self):
        return 10**30


big = 10**30

try:
    '\thello'.expandtabs(big)
    assert False, 'expected an out-of-range tabsize to overflow'
except OverflowError:
    pass

# The same rejection reached through `__index__`, where the instance and the
# value its dunder returns are both temporaries that must be released.
try:
    '\thello'.expandtabs(BigIndex())
    assert False, 'expected an out-of-range __index__ tabsize to overflow'
except OverflowError:
    pass

len('done')
# ref-counts={'big': 1, 'BigIndex': 1}
