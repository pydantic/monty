# A partial chain deep enough to exhaust the native re-entry budget bails out of
# `Partial::py_call` after the bound parts have been lifted off the heap but
# before the call takes ownership of them. That branch has to hand back both the
# lifted parts and the call's own arguments; otherwise every RecursionError
# leaks a reference per level.
import functools
import sys

arg = [1, 2]
bound = {'k': 1}


def target(*a, **k):
    return a


def chain(depth):
    cur = target
    for _ in range(depth):

        class Holder:
            wrapped = functools.partial(cur, bound)

        cur = Holder().wrapped
    return cur


if sys.platform == 'monty':
    deep = chain(30)
    for _ in range(3):
        try:
            deep(arg)
            assert False, 'expected the deep partial chain to raise'
        except RecursionError:
            pass
else:
    deep = chain(30)
    for _ in range(3):
        deep(arg)

len(arg)
# ref-counts={'functools': 1, 'sys': 1, 'arg': 1, 'bound': 31, 'deep': 1}
