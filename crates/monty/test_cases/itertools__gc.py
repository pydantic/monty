# Cycle-collector interaction for the itertools iterators: these force a
# collection, so `for_each_child_id` — the arm that silently under-traces when
# missing — actually runs, which the refcount fixture alone never does.
# `gc.collect()` returns different counts on CPython and Monty, so it isn't asserted.
import gc

import itertools


def repeat_cycle():
    # The repeat holds the list, the list holds the repeat: unreachable once
    # this returns, and only collectable by tracing through the iterator.
    items = []
    items.append(itertools.repeat(items, 1))
    return len(items)


def count_cycle():
    # `count` is not itself GC-tracked (it only holds numbers) but is reachable
    # as a CHILD of a tracked cycle, so the collector still traverses it. Both
    # start AND step are big ints, so a hook walking only `current` is caught.
    items = []
    items.append(itertools.count(2**70, 2**70))
    items.append(items)
    return len(items)


assert repeat_cycle() == 1
assert count_cycle() == 2
gc.collect()

# Iterators still work after a collection.
survivor = itertools.repeat('x', 2)
gc.collect()
assert list(survivor) == ['x', 'x']

# Freeing an iterator outright must release what it holds (the `py_dec_ref_ids`
# path, distinct from the tracing one above).
payload = [1, 2]
doomed = itertools.repeat(payload, 2)
doomed = None
counter = itertools.count(2**70)
counter = None
assert payload == [1, 2]
assert doomed is None
assert counter is None

# Size hints: bounded `repeat` reports its remaining count to the collection
# builders, while the infinite forms must report nothing usable.
assert list(itertools.repeat(7, 3)) == [7, 7, 7]
assert set(itertools.repeat(7, 3)) == {7}
assert sorted(set(itertools.repeat('a', 2))) == ['a']
partial = itertools.repeat(5, 4)
next(partial)
assert list(partial) == [5, 5, 5]
assert list(itertools.repeat(1, 0)) == []
# `zip` stops at the shortest input, so it is the one builder that can safely
# take an infinite adaptor — `map`/`filter`/`enumerate` are eager in Monty and
# would never return (see limitations/itertools.md).
assert list(zip(itertools.count(10), 'ab')) == [(10, 'a'), (11, 'b')]
assert list(zip(itertools.repeat('z'), [1, 2])) == [('z', 1), ('z', 2)]
gc.collect()
