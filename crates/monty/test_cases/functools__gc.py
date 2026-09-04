# Cycle-collector interaction: a `partial` holds its arguments and an
# `lru_cache` its stored keys and values directly, so a cycle through either is
# only collectable by tracing `for_each_child_id`. Smoke coverage only —
# under-tracing leaks silently and nothing here notices;
# `refcount__functools_partial.py` and `refcount__functools_lru_cache.py` are
# what verify the hooks.
# `gc.collect()` returns different counts on CPython and Monty, so it isn't asserted.
import gc

import functools


def target(*args, **kwargs):
    return (args, kwargs)


def arg_cycle():
    # The partial holds the list, the list holds the partial: unreachable once
    # this returns, and only collectable by tracing through the partial.
    items = []
    items.append(functools.partial(target, items))
    return len(items)


def keyword_cycle():
    # Same, through a bound keyword value rather than a positional.
    items = []
    items.append(functools.partial(target, k=items))
    return len(items)


def func_cycle():
    # The wrapped callable is traced too: here the partial's `func` is a closure
    # over the list that holds the partial.
    items = []

    def closure():
        return len(items)

    items.append(functools.partial(closure))
    return len(items)


def cached_value_cycle():
    # The wrapper's stored result is the list that holds the wrapper, so the
    # cycle runs through the cache rather than through an argument.
    items = []
    holder = functools.cache(lambda: items)
    items.append(holder)
    holder()
    return len(items)


class Node:
    def __init__(self, holder):
        self.holder = holder


def cached_key_cycle():
    # Same, through a key rather than a value: the stored key holds the node,
    # and the node holds the wrapper the key lives in.
    holder = functools.cache(lambda arg: 1)
    return holder(Node(holder))


assert arg_cycle() == 1
assert keyword_cycle() == 1
assert func_cycle() == 1
assert cached_value_cycle() == 1
assert cached_key_cycle() == 1
gc.collect()

# Still reachable after the collection, so nothing above was condemned early.
live = []
live.append(functools.partial(target, live))
live_cache = functools.cache(lambda: live)
live_cache()
gc.collect()
assert live[0]()[0][0] is live
assert live_cache() is live
