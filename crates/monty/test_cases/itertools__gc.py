# Cycle-collector interaction: these drive a collection over cycles the
# iterators take part in, so the collector traverses these types. Smoke coverage
# only — an under-tracing `for_each_child_id` leaks silently and nothing here
# notices; `refcount__itertools_count_repeat.py` is what verifies the hooks.
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
    # `count` is not itself GC-tracked (it only holds numbers) but sits inside a
    # tracked cycle, so the collector walks past it while condemning the list.
    items = []
    items.append(itertools.count(2**70, 2**70))
    items.append(items)
    return len(items)


def chain_cycle():
    # A chain ends by RELEASING its arguments, and here an argument is the list
    # that holds the chain: the release runs while the chain is mid-`next`, so
    # it must cope with dropping objects that refer back to it.
    items = []
    items.append(itertools.chain(items))
    drained = list(items[0])
    return len(drained)


def product_cycle():
    # The pool holds the list and the list holds the product, so the cycle
    # runs through a collected pool rather than through a live source.
    items = []
    items.append(itertools.product([items]))
    return len(items)


def groupby_cycle():
    # `groupby` holds its source, its key function, and the key and item it
    # read ahead — here the key function is a closure over the list that holds
    # the `groupby`, so collecting it means tracing that edge.
    items = []
    items.append(itertools.groupby([1, 1], lambda x: len(items)))
    next(items[0])
    return len(items)


def grouper_cycle():
    # A group holds its parent, so the cycle closes through two iterators: the
    # list holds the group, the group holds the `groupby`, and the `groupby`'s
    # source holds the list.
    items = []
    grouped = itertools.groupby([items])
    items.append(next(grouped)[1])
    return len(items)


assert repeat_cycle() == 1
assert count_cycle() == 2
assert chain_cycle() == 1
assert product_cycle() == 1
assert groupby_cycle() == 1
assert grouper_cycle() == 1
gc.collect()

# Iterators still work after a collection.
survivor = itertools.repeat('x', 2)
gc.collect()
assert list(survivor) == ['x', 'x']

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
