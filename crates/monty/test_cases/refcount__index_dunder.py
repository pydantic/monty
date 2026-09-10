# Refcount coverage for the `__index__` dispatch path.
#
# The dunder's return value is owned by the coercion, which must release it once
# narrowed to an `i64`. Nothing else names that value, so a missed release shows
# up as an unreachable heap object rather than as a wrong count.


class NarrowIndex:
    # The wide intermediates are released inside the dunder; the returned `2`
    # fits a machine int, so it is inline and its release is a no-op.
    def __index__(self):
        return 10**30 // 10**30 + 1


half = 10**15


class WideIndex:
    # Beyond `i64`, so the value cannot be inline (`into_value` only demotes what
    # fits). Multiplied at runtime rather than written as a literal, which the
    # compiler would fold and intern — an interned `LongInt` is not refcounted,
    # so a missed release would leave nothing to observe. The coercion is handed
    # this heap `LongInt` and must release it on the overflow path.
    def __index__(self):
        return half * half


class Boom:
    def __index__(self):
        raise ValueError('boom')


narrow = NarrowIndex()

# Each of these allocates a fresh return value and must release it.
assert [10, 20, 30][narrow] == 30
assert 'abcdef'[narrow] == 'c'
assert 'abcdef'[narrow:] == 'cdef'
assert list(range(narrow)) == [0, 1]
assert 'x'.center(narrow) == 'x '

# Indexing rejects the oversized return, so the release happens on the way out
# through the error.
wide = WideIndex()
try:
    [1, 2, 3][wide]
    assert False, 'expected an out-of-range index to raise'
except IndexError:
    pass

# A slice bound takes the same coercion but saturates instead of raising, so the
# release happens on the success path too.
assert [1, 2, 3][wide:] == []

# The raising path leaves the coercion through an early return, where a missed
# release is easiest to introduce. The temporary receiver must be released too,
# so a leak leaves an object nothing names.
erroring = Boom()
try:
    [1, 2, 3][Boom()]
except ValueError:
    pass

len('done')
# ref-counts={'narrow': 1, 'NarrowIndex': 2, 'WideIndex': 2, 'wide': 1, 'Boom': 2, 'erroring': 1}
