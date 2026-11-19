import bisect
from functools import partial

# Heap-owned sequences, search values, keys and comparison intermediates.
rows = [[str(i) * 20, i] for i in range(8)]
key = partial(lambda offset, row: [row[1] + offset], 10)
assert bisect.bisect_left(rows, [13], key=key) == 3
assert bisect.bisect_right(rows, [13], key=key) == 4
new = ['new' * 20, 3]
bisect.insort_left(rows, new, key=key)
assert rows[3] is new
bisect.insort_right(rows, new, key=key)
assert rows[5] is new

# Repeated failures must release every temporary, including key results.
for i in range(20):
    try:
        bisect.bisect_left(rows, ['incomparable' * 20], key=key)
        assert False, 'expected failure'
    except TypeError as exc:
        assert str(exc) == "'<' not supported between instances of 'int' and 'str'"

    def fail(row):
        temporary = ['temporary' * 20]
        raise ValueError(temporary[0])

    for f in [bisect.bisect_left, bisect.insort_right]:
        try:
            f(rows, new, key=fail)
            assert False, 'expected failure'
        except ValueError as exc:
            assert str(exc) == 'temporary' * 20

# A callback can remove elements; subsequent indexing raises safely.
items = [[1], [2], [3]]


def clear_items(item):
    items.clear()
    return item


try:
    bisect.bisect_left(items, [2], key=clear_items)
    assert False, 'expected failure'
except IndexError as exc:
    assert str(exc) == 'list index out of range'


class WideIndex:
    def __index__(self):
        base = 10**15
        return base * base


for bound, message in [
    ('lo', 'Python int too large to convert to C ssize_t'),
    ('hi', "cannot fit 'WideIndex' into an index-sized integer"),
]:
    try:
        bisect.bisect_left(rows, new, **{bound: WideIndex()})
        assert False, 'expected failure'
    except OverflowError as exc:
        assert str(exc) == message
