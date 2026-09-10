# Cycle-collector interaction: a `partial` holds its arguments directly, so a
# cycle through one is only collectable by tracing `for_each_child_id`. Smoke
# coverage only — under-tracing leaks silently and nothing here notices;
# `refcount__functools_partial.py` is what verifies the hooks.
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


assert arg_cycle() == 1
assert keyword_cycle() == 1
assert func_cycle() == 1
gc.collect()

# Still reachable after the collection, so nothing above was condemned early.
live = []
live.append(functools.partial(target, live))
gc.collect()
assert live[0]()[0][0] is live
