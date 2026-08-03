# Refcount coverage for the `__index__` dispatch path.
#
# The dunder's return value is owned by the coercion, which must release it once
# narrowed to an `i64`. Returning a `LongInt` (wider than a machine int, so it is
# heap-allocated rather than inline) makes a missed release observable: nothing
# else names the returned object.


class WideIndex:
    def __index__(self):
        return 10**30 // 10**30 + 1


class Boom:
    def __index__(self):
        raise ValueError('boom')


wide = WideIndex()

# Each of these allocates a fresh return value and must release it.
assert [10, 20, 30][wide] == 30
assert 'abcdef'[wide] == 'c'
assert 'abcdef'[wide:] == 'cdef'
assert list(range(wide)) == [0, 1]
assert 'x'.center(wide) == 'x '

# The raising path leaves the coercion through an early return, where a missed
# release is easiest to introduce. The temporary receiver must be released too,
# so a leak leaves an object nothing names.
erroring = Boom()
try:
    [1, 2, 3][Boom()]
except ValueError:
    pass

len('done')
# ref-counts={'wide': 1, 'WideIndex': 2, 'Boom': 2, 'erroring': 1}
